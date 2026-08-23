#[cfg(test)]
use crate::db::AgentDb;
use crate::{
    config::SchedulerConfig,
    store::{notifications::NotificationKind, NotificationStore, TodoStore},
    tools::ntfy::NtfyClient,
};
use std::time::Duration;
use tracing::{error, info};

pub async fn run_notification_loop(
    todo_store: TodoStore,
    notif_store: NotificationStore,
    ntfy: NtfyClient,
    config: SchedulerConfig,
) {
    let mut interval =
        tokio::time::interval(Duration::from_secs(config.notification_interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        if let Err(e) = check_overdue_todos(&todo_store, &notif_store, &ntfy, &config).await {
            error!(error = %e, "overdue todo notification check failed");
        }
    }
}

pub(crate) async fn check_overdue_todos(
    todo_store: &TodoStore,
    notif_store: &NotificationStore,
    ntfy: &NtfyClient,
    config: &SchedulerConfig,
) -> anyhow::Result<()> {
    let overdue = todo_store.overdue_open(config.overdue_grace_secs).await?;
    for todo in overdue {
        let ref_id = todo.id.to_string();
        let kind = NotificationKind::TodoOverdue;

        if notif_store.already_sent(&ref_id, &kind).await? {
            continue;
        }

        let title = format!("Overdue: {}", todo.title);
        let body = match &todo.due_at {
            Some(d) => format!("Due at {}", d.format("%Y-%m-%d %H:%M UTC")),
            None => "Task is overdue".to_owned(),
        };

        notif_store
            .record_attempt(kind.clone(), ref_id.clone(), title.clone(), body.clone())
            .await?;

        match ntfy.send(&title, &body, "high").await {
            Ok(()) => {
                metrics::counter!("agentix_notifications_sent_total", "kind" => "todo_overdue")
                    .increment(1);
                info!(todo_id = %todo.id, "sent overdue notification");
                notif_store.mark_sent(&kind, &ref_id).await?;
            }
            Err(e) => {
                metrics::counter!("agentix_notifications_failed_total", "kind" => "todo_overdue")
                    .increment(1);
                error!(todo_id = %todo.id, error = %e, "failed to send overdue notification");
                notif_store.mark_failed(&kind, &ref_id).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::todos::TodoSource;
    use crate::store::{notifications::NotificationKind, NotificationStore, TodoStore};
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

    fn test_config() -> SchedulerConfig {
        SchedulerConfig {
            notification_interval_secs: 300,
            gmail_scan_interval_secs: 900,
            event_reminder_lead_secs: 1800,
            overdue_grace_secs: 0,
        }
    }

    #[tokio::test]
    async fn only_one_ntfy_post_for_two_overdue_one_already_sent() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/test-topic"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1) // exactly one POST expected
            .mount(&server)
            .await;

        let db = test_db();
        let todo_store = TodoStore(db.clone());
        let notif_store = NotificationStore(db.clone());
        let ntfy = NtfyClient::new(server.uri(), "test-topic".into());
        let cfg = test_config();

        let past = chrono::Utc::now() - chrono::Duration::seconds(3600);

        // Two overdue todos
        let todo1 = todo_store
            .insert(
                "overdue task 1".into(),
                Some(past),
                TodoSource::Manual,
                None,
                None,
            )
            .await
            .expect("insert todo1");
        todo_store
            .insert(
                "overdue task 2".into(),
                Some(past),
                TodoSource::Manual,
                None,
                None,
            )
            .await
            .expect("insert todo2");

        // Mark todo1 as already sent
        notif_store
            .record_attempt(
                NotificationKind::TodoOverdue,
                todo1.id.to_string(),
                "Overdue: overdue task 1".into(),
                "test body".into(),
            )
            .await
            .expect("record_attempt");
        notif_store
            .mark_sent(&NotificationKind::TodoOverdue, &todo1.id.to_string())
            .await
            .expect("mark_sent");

        check_overdue_todos(&todo_store, &notif_store, &ntfy, &cfg)
            .await
            .expect("check_overdue_todos");

        // wiremock verifies .expect(1) on drop
    }
}
