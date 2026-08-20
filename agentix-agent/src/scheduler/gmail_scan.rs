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
