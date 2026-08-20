use crate::db::AgentDb;
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationKind {
    TodoOverdue,
    EventReminder,
    GmailAction,
}

impl NotificationKind {
    pub fn as_str(&self) -> &str {
        match self {
            NotificationKind::TodoOverdue => "todo_overdue",
            NotificationKind::EventReminder => "event_reminder",
            NotificationKind::GmailAction => "gmail_action",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRecord {
    pub id: Uuid,
    pub kind: NotificationKind,
    pub reference_id: String,
    pub title: String,
    pub body: String,
    pub sent_at: Option<DateTime<Utc>>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub retry_count: u32,
}

#[derive(Clone)]
pub struct NotificationStore(pub AgentDb);

impl NotificationStore {
    fn key(kind: &NotificationKind, reference_id: &str) -> Vec<u8> {
        format!("{}/{}", kind.as_str(), reference_id).into_bytes()
    }

    pub async fn already_sent(
        &self,
        reference_id: &str,
        kind: &NotificationKind,
    ) -> anyhow::Result<bool> {
        let tree = self.0.notifications.clone();
        let key = Self::key(kind, reference_id);
        tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            match tree.get(&key).context("sled get")? {
                None => Ok(false),
                Some(bytes) => {
                    let record: NotificationRecord =
                        serde_json::from_slice(&bytes).context("deserialize notification")?;
                    Ok(record.sent_at.is_some())
                }
            }
        })
        .await
        .context("spawn_blocking")?
    }

    pub async fn record_attempt(
        &self,
        kind: NotificationKind,
        reference_id: String,
        title: String,
        body: String,
    ) -> anyhow::Result<Uuid> {
        let tree = self.0.notifications.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Uuid> {
            let key = Self::key(&kind, &reference_id);
            let record = match tree.get(&key).context("sled get")? {
                Some(bytes) => {
                    let mut existing: NotificationRecord =
                        serde_json::from_slice(&bytes).context("deserialize notification")?;
                    existing.retry_count += 1;
                    existing.last_attempt_at = Some(Utc::now());
                    existing
                }
                None => NotificationRecord {
                    id: Uuid::new_v4(),
                    kind,
                    reference_id,
                    title,
                    body,
                    sent_at: None,
                    last_attempt_at: Some(Utc::now()),
                    retry_count: 0,
                },
            };
            let id = record.id;
            tree.insert(key, serde_json::to_vec(&record).context("serialize notification")?)
                .context("sled insert")?;
            Ok(id)
        })
        .await
        .context("spawn_blocking")?
    }

    pub async fn mark_sent(
        &self,
        kind: &NotificationKind,
        reference_id: &str,
    ) -> anyhow::Result<()> {
        let tree = self.0.notifications.clone();
        let key = Self::key(kind, reference_id);
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let bytes = tree
                .get(&key)
                .context("sled get")?
                .context("notification record not found")?;
            let mut record: NotificationRecord =
                serde_json::from_slice(&bytes).context("deserialize notification")?;
            record.sent_at = Some(Utc::now());
            tree.insert(key, serde_json::to_vec(&record).context("serialize notification")?)
                .context("sled insert")?;
            Ok(())
        })
        .await
        .context("spawn_blocking")?
    }

    pub async fn mark_failed(
        &self,
        kind: &NotificationKind,
        reference_id: &str,
    ) -> anyhow::Result<()> {
        let tree = self.0.notifications.clone();
        let key = Self::key(kind, reference_id);
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let bytes = tree
                .get(&key)
                .context("sled get")?
                .context("notification record not found")?;
            let mut record: NotificationRecord =
                serde_json::from_slice(&bytes).context("deserialize notification")?;
            record.retry_count += 1;
            if record.retry_count >= 5 {
                tracing::error!(
                    id = %record.id,
                    reference_id = %record.reference_id,
                    "notification exceeded max retries, giving up"
                );
                return Ok(());
            }
            tree.insert(key, serde_json::to_vec(&record).context("serialize notification")?)
                .context("sled insert")?;
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
    async fn already_sent_false_then_true_after_mark_sent() {
        let store = NotificationStore(test_db());
        let kind = NotificationKind::TodoOverdue;
        let ref_id = "todo-123";

        let is_sent = store
            .already_sent(ref_id, &kind)
            .await
            .expect("already_sent");
        assert!(!is_sent);

        store
            .record_attempt(
                NotificationKind::TodoOverdue,
                ref_id.to_owned(),
                "Task Overdue".to_owned(),
                "Your task is overdue".to_owned(),
            )
            .await
            .expect("record_attempt");

        store
            .mark_sent(&kind, ref_id)
            .await
            .expect("mark_sent");

        let is_sent = store
            .already_sent(ref_id, &kind)
            .await
            .expect("already_sent after mark_sent");
        assert!(is_sent);
    }

    #[tokio::test]
    async fn mark_failed_at_max_retries_returns_ok() {
        let store = NotificationStore(test_db());
        let kind = NotificationKind::EventReminder;
        let ref_id = "event-456";

        store
            .record_attempt(
                NotificationKind::EventReminder,
                ref_id.to_owned(),
                "Event Reminder".to_owned(),
                "Event is coming up".to_owned(),
            )
            .await
            .expect("record_attempt");

        for _ in 0..5 {
            store
                .mark_failed(&kind, ref_id)
                .await
                .expect("mark_failed should not error even at max retries");
        }
    }
}
