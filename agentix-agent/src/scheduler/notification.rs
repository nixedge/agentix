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

async fn check_overdue_todos(
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
