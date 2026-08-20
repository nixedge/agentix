use crate::db::AgentDb;
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sled::transaction::TransactionError;
use sled::Transactional;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TodoSource {
    Manual,
    Gmail,
    Calendar,
}

impl TodoSource {
    fn as_str(&self) -> &'static str {
        match self {
            TodoSource::Manual => "manual",
            TodoSource::Gmail => "gmail",
            TodoSource::Calendar => "calendar",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: Uuid,
    pub title: String,
    pub due_at: Option<DateTime<Utc>>,
    pub completed: bool,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub source: TodoSource,
    pub source_id: Option<String>,
    pub notes: Option<String>,
}

#[derive(Clone)]
pub struct TodoStore(pub AgentDb);

impl TodoStore {
    pub async fn insert(
        &self,
        title: String,
        due_at: Option<DateTime<Utc>>,
        source: TodoSource,
        source_id: Option<String>,
        notes: Option<String>,
    ) -> anyhow::Result<Todo> {
        let todos_tree = self.0.todos.clone();
        let by_source_tree = self.0.todos_by_source.clone();

        let title = title.trim().to_owned();
        anyhow::ensure!(!title.is_empty(), "todo title must not be empty");
        anyhow::ensure!(title.len() <= 500, "todo title too long (max 500 bytes)");

        tokio::task::spawn_blocking(move || -> anyhow::Result<Todo> {
            let todo = Todo {
                id: Uuid::new_v4(),
                title,
                due_at,
                completed: false,
                created_at: Utc::now(),
                completed_at: None,
                source: source.clone(),
                source_id: source_id.clone(),
                notes,
            };

            if let (Some(sid), src) = (&source_id, &source) {
                if *src != TodoSource::Manual {
                    let source_key = format!("{}/{}", src.as_str(), sid);
                    let id_bytes: Vec<u8> = todo.id.as_bytes().to_vec();
                    let json_bytes = serde_json::to_vec(&todo).context("serialize todo")?;

                    let result: Result<Option<()>, TransactionError<()>> =
                        (&todos_tree, &by_source_tree).transaction(|trees| {
                            let (todos_tx, by_source_tx) = trees;
                            if by_source_tx.get(source_key.as_bytes())?.is_some() {
                                return Ok(None);
                            }
                            todos_tx.insert(id_bytes.as_slice(), json_bytes.as_slice())?;
                            by_source_tx.insert(source_key.as_bytes(), id_bytes.as_slice())?;
                            Ok(Some(()))
                        });

                    match result {
                        Ok(Some(())) => return Ok(todo),
                        Ok(None) => {
                            anyhow::bail!("duplicate: todo already exists for {source_key}")
                        }
                        Err(TransactionError::Storage(e)) => {
                            return Err(anyhow::anyhow!(e).context("sled transaction"))
                        }
                        Err(TransactionError::Abort(())) => unreachable!(),
                    }
                }
            }

            let id_bytes: Vec<u8> = todo.id.as_bytes().to_vec();
            let json_bytes = serde_json::to_vec(&todo).context("serialize todo")?;
            todos_tree
                .insert(id_bytes.as_slice(), json_bytes.as_slice())
                .context("sled insert")?;
            Ok(todo)
        })
        .await
        .context("spawn_blocking")?
    }

    pub async fn list(
        &self,
        include_completed: bool,
        search: Option<String>,
        limit: u32,
    ) -> anyhow::Result<Vec<Todo>> {
        let tree = self.0.todos.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Todo>> {
            let query = search.as_deref().map(|s| s.to_lowercase());
            let mut todos: Vec<Todo> = tree
                .iter()
                .values()
                .filter_map(|v| v.ok())
                .filter_map(|v| serde_json::from_slice::<Todo>(&v).ok())
                .filter(|t| include_completed || !t.completed)
                .filter(|t| {
                    query
                        .as_deref()
                        .map(|q| t.title.to_lowercase().contains(q))
                        .unwrap_or(true)
                })
                .collect();

            todos.sort_by(|a, b| match (&a.due_at, &b.due_at) {
                (Some(da), Some(db)) => da.cmp(db),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.created_at.cmp(&b.created_at),
            });

            todos.truncate(limit as usize);
            Ok(todos)
        })
        .await
        .context("spawn_blocking")?
    }

    pub async fn complete(&self, id: Uuid) -> anyhow::Result<()> {
        let tree = self.0.todos.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let key: Vec<u8> = id.as_bytes().to_vec();
            let bytes = tree
                .get(&key)
                .context("sled get")?
                .with_context(|| format!("todo {id} not found"))?;
            let mut todo: Todo = serde_json::from_slice(&bytes).context("deserialize todo")?;
            todo.completed = true;
            todo.completed_at = Some(Utc::now());
            tree.insert(key, serde_json::to_vec(&todo).context("serialize todo")?)
                .context("sled insert")?;
            Ok(())
        })
        .await
        .context("spawn_blocking")?
    }

    pub async fn update(
        &self,
        id: Uuid,
        due_at: Option<Option<DateTime<Utc>>>,
        notes: Option<Option<String>>,
    ) -> anyhow::Result<Todo> {
        let tree = self.0.todos.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Todo> {
            let key: Vec<u8> = id.as_bytes().to_vec();
            let bytes = tree
                .get(&key)
                .context("sled get")?
                .with_context(|| format!("todo {id} not found"))?;
            let mut todo: Todo = serde_json::from_slice(&bytes).context("deserialize todo")?;
            if let Some(d) = due_at {
                todo.due_at = d;
            }
            if let Some(n) = notes {
                todo.notes = n;
            }
            tree.insert(key, serde_json::to_vec(&todo).context("serialize")?)
                .context("sled insert")?;
            Ok(todo)
        })
        .await
        .context("spawn_blocking")?
    }

    pub async fn find_by_source(
        &self,
        source: &str,
        source_id: &str,
    ) -> anyhow::Result<Option<Todo>> {
        let todos_tree = self.0.todos.clone();
        let by_source_tree = self.0.todos_by_source.clone();
        let key = format!("{source}/{source_id}");
        tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Todo>> {
            let id_bytes = match by_source_tree.get(key.as_bytes()).context("sled get")? {
                Some(b) => b,
                None => return Ok(None),
            };
            let todo_bytes = match todos_tree.get(&id_bytes).context("sled get")? {
                Some(b) => b,
                None => return Ok(None),
            };
            Ok(Some(
                serde_json::from_slice(&todo_bytes).context("deserialize todo")?,
            ))
        })
        .await
        .context("spawn_blocking")?
    }

    pub async fn overdue_open(&self, grace_secs: i64) -> anyhow::Result<Vec<Todo>> {
        let tree = self.0.todos.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Todo>> {
            let cutoff = Utc::now() - chrono::Duration::seconds(grace_secs);
            Ok(tree
                .iter()
                .values()
                .filter_map(|v| v.ok())
                .filter_map(|v| serde_json::from_slice::<Todo>(&v).ok())
                .filter(|t| !t.completed && t.due_at.map(|d| d < cutoff).unwrap_or(false))
                .collect())
        })
        .await
        .context("spawn_blocking")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AgentDb;

    fn test_db() -> AgentDb {
        let db = sled::Config::new()
            .temporary(true)
            .open()
            .expect("open temp db");
        AgentDb {
            todos: db.open_tree("todos").expect("todos tree"),
            todos_by_source: db.open_tree("todos_by_source").expect("by_source tree"),
            notifications: db.open_tree("notifications").expect("notifications tree"),
            gmail_cursor: db.open_tree("gmail_cursor").expect("gmail_cursor tree"),
            sessions: db.open_tree("sessions").expect("sessions tree"),
        }
    }

    #[tokio::test]
    async fn overdue_open_returns_past_due_todos() {
        let store = TodoStore(test_db());
        let past = Utc::now() - chrono::Duration::seconds(3600);
        store
            .insert(
                "overdue task".into(),
                Some(past),
                TodoSource::Manual,
                None,
                None,
            )
            .await
            .expect("insert");
        let overdue = store.overdue_open(0).await.expect("overdue_open");
        assert_eq!(overdue.len(), 1);
        assert_eq!(overdue[0].title, "overdue task");
    }

    #[tokio::test]
    async fn find_by_source_dedup() {
        let store = TodoStore(test_db());
        store
            .insert(
                "gmail task".into(),
                None,
                TodoSource::Gmail,
                Some("msg123".into()),
                None,
            )
            .await
            .expect("first insert");
        let found = store
            .find_by_source("gmail", "msg123")
            .await
            .expect("find");
        assert!(found.is_some());
        // Second insert with same source_id should fail
        let dup = store
            .insert(
                "gmail task dup".into(),
                None,
                TodoSource::Gmail,
                Some("msg123".into()),
                None,
            )
            .await;
        assert!(dup.is_err());
    }

    #[tokio::test]
    async fn list_search_filter() {
        let store = TodoStore(test_db());
        store
            .insert("buy milk".into(), None, TodoSource::Manual, None, None)
            .await
            .expect("insert");
        store
            .insert("fix bike".into(), None, TodoSource::Manual, None, None)
            .await
            .expect("insert");
        let results = store
            .list(false, Some("milk".into()), 100)
            .await
            .expect("list");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "buy milk");
    }

    #[tokio::test]
    async fn atomic_transaction_prevents_duplicate() {
        let store = TodoStore(test_db());
        store
            .insert(
                "email task".into(),
                None,
                TodoSource::Gmail,
                Some("abc".into()),
                None,
            )
            .await
            .expect("first insert ok");
        let second = store
            .insert(
                "email task again".into(),
                None,
                TodoSource::Gmail,
                Some("abc".into()),
                None,
            )
            .await;
        assert!(second.is_err(), "second insert should fail as duplicate");
    }
}
