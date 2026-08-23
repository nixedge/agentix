#[cfg(test)]
use crate::db::AgentDb;
use std::{sync::Arc, time::Duration};
use tracing::{error, info};

use crate::{
    google::gmail::GmailClient,
    store::{GmailCursorStore, NotificationStore, TodoStore},
};

pub async fn run_gmail_scan_loop(
    gmail: Arc<GmailClient>,
    todo_store: TodoStore,
    notif_store: NotificationStore,
    cursor_store: GmailCursorStore,
    interval_secs: u64,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        if let Err(e) = scan_once(&gmail, &todo_store, &notif_store, &cursor_store).await {
            error!(error = %e, "gmail scan failed");
        }
    }
}

async fn scan_once(
    gmail: &GmailClient,
    todo_store: &TodoStore,
    notif_store: &NotificationStore,
    cursor_store: &GmailCursorStore,
) -> anyhow::Result<()> {
    let messages = gmail.list_unread(20).await?;
    info!(count = messages.len(), "gmail scan completed");

    for msg in &messages {
        let source_id = msg.id.clone();
        let subject = msg.subject.as_deref().unwrap_or("(no subject)").to_owned();

        let already = todo_store.find_by_source("gmail", &source_id).await?;

        if already.is_none() {
            let title = format!("[gmail] {subject}");
            let notes = msg.snippet.clone();
            todo_store
                .insert(
                    title,
                    None,
                    crate::store::todos::TodoSource::Gmail,
                    Some(source_id.clone()),
                    notes,
                )
                .await?;
        }

        let _ = notif_store;
    }

    let history_id = gmail.get_history_id().await?;
    cursor_store.save(&history_id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{GmailCursorStore, NotificationStore, TodoStore};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_db() -> AgentDb {
        let db = sled::Config::new()
            .temporary(true)
            .open()
            .expect("open temp db");
        AgentDb {
            todos: db.open_tree("todos").expect("todos"),
            todos_by_source: db.open_tree("todos_by_source").expect("by_source"),
            notifications: db.open_tree("notifications").expect("notifications"),
            gmail_cursor: db.open_tree("gmail_cursor").expect("gmail_cursor"),
            sessions: db.open_tree("sessions").expect("sessions"),
        }
    }

    fn message_list_response(ids: &[&str]) -> serde_json::Value {
        let msgs: Vec<serde_json::Value> = ids
            .iter()
            .map(|id| serde_json::json!({ "id": id }))
            .collect();
        serde_json::json!({ "messages": msgs })
    }

    fn message_detail(id: &str, subject: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "threadId": "thread1",
            "snippet": "test snippet",
            "payload": {
                "headers": [
                    { "name": "Subject", "value": subject },
                    { "name": "From", "value": "sender@example.com" }
                ]
            }
        })
    }

    fn profile_response() -> serde_json::Value {
        serde_json::json!({ "historyId": "12345", "emailAddress": "user@example.com" })
    }

    #[tokio::test]
    async fn duplicate_message_creates_only_one_todo() {
        let server = MockServer::start().await;

        // Both scans return the same message ID
        Mock::given(method("GET"))
            .and(path("/users/me/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(message_list_response(&["msg_abc"])),
            )
            .expect(2)
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/users/me/messages/msg_abc"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(message_detail("msg_abc", "Your package arrives tomorrow")),
            )
            .expect(2)
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/users/me/profile"))
            .respond_with(ResponseTemplate::new(200).set_body_json(profile_response()))
            .mount(&server)
            .await;

        let gmail = Arc::new(GmailClient::new_for_test(server.uri(), "fake_token".into()));
        let db = test_db();
        let todo_store = TodoStore(db.clone());
        let notif_store = NotificationStore(db.clone());
        let cursor_store = GmailCursorStore(db.clone());

        // First scan — should create the todo
        scan_once(&gmail, &todo_store, &notif_store, &cursor_store)
            .await
            .expect("first scan");

        // Second scan — same message ID, should NOT create a duplicate
        scan_once(&gmail, &todo_store, &notif_store, &cursor_store)
            .await
            .expect("second scan");

        let todos = todo_store.list(false, None, 100).await.expect("list todos");

        assert_eq!(
            todos.len(),
            1,
            "expected exactly one todo for the same gmail message, got {}",
            todos.len()
        );
        assert!(todos[0].title.contains("package arrives"));
    }
}
