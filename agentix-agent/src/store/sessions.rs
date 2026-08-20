use crate::db::AgentDb;
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
struct SessionRecord {
    messages: Vec<serde_json::Value>,
    updated_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct SessionStore(pub AgentDb);

impl SessionStore {
    pub async fn load(&self, session_id: Uuid) -> anyhow::Result<Vec<serde_json::Value>> {
        let tree = self.0.sessions.clone();
        let key = session_id.as_bytes().to_vec();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<serde_json::Value>> {
            match tree.get(&key).context("sled get")? {
                None => Ok(vec![]),
                Some(bytes) => {
                    let record: SessionRecord =
                        serde_json::from_slice(&bytes).context("deserialize session")?;
                    Ok(record.messages)
                }
            }
        })
        .await
        .context("spawn_blocking")?
    }

    pub async fn save(
        &self,
        session_id: Uuid,
        messages: &[serde_json::Value],
    ) -> anyhow::Result<()> {
        let tree = self.0.sessions.clone();
        let key = session_id.as_bytes().to_vec();
        let record = SessionRecord {
            messages: messages.to_vec(),
            updated_at: Utc::now(),
        };
        let bytes = serde_json::to_vec(&record).context("serialize session")?;
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            tree.insert(key, bytes).context("sled insert")?;
            Ok(())
        })
        .await
        .context("spawn_blocking")?
    }

    pub async fn prune_older_than_days(&self, days: i64) -> anyhow::Result<()> {
        let tree = self.0.sessions.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let cutoff = Utc::now() - chrono::Duration::days(days);
            let mut to_remove: Vec<Vec<u8>> = vec![];
            for item in tree.iter() {
                let (key, val) = item.context("sled iter")?;
                match serde_json::from_slice::<SessionRecord>(&val) {
                    Ok(record) if record.updated_at < cutoff => {
                        to_remove.push(key.to_vec());
                    }
                    _ => {}
                }
            }
            for key in to_remove {
                tree.remove(key).context("sled remove")?;
            }
            Ok(())
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
    async fn prune_removes_old_keeps_recent() {
        let store = SessionStore(test_db());

        let old_id = Uuid::new_v4();
        let recent_id = Uuid::new_v4();

        let messages = vec![serde_json::json!({"role": "user", "content": "hello"})];

        // Save both sessions
        store
            .save(old_id, &messages)
            .await
            .expect("save old session");
        store
            .save(recent_id, &messages)
            .await
            .expect("save recent session");

        // Manually overwrite old session with a very old timestamp via tree directly
        let old_record = serde_json::json!({
            "messages": messages,
            "updated_at": "2020-01-01T00:00:00Z"
        });
        store
            .0
            .sessions
            .insert(
                old_id.as_bytes().to_vec(),
                serde_json::to_vec(&old_record).expect("serialize"),
            )
            .expect("sled insert old");

        // Prune sessions older than 1 day
        store
            .prune_older_than_days(1)
            .await
            .expect("prune");

        let old_msgs = store.load(old_id).await.expect("load old");
        let recent_msgs = store.load(recent_id).await.expect("load recent");

        assert!(old_msgs.is_empty(), "old session should have been pruned");
        assert!(!recent_msgs.is_empty(), "recent session should remain");
    }

    #[tokio::test]
    async fn load_missing_returns_empty_vec() {
        let store = SessionStore(test_db());
        let msgs = store.load(Uuid::new_v4()).await.expect("load missing");
        assert!(msgs.is_empty());
    }
}
