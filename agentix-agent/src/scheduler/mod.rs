pub mod gmail_scan;
pub mod notification;

use crate::{
    config::AgentConfig,
    db::AgentDb,
    store::{GmailCursorStore, NotificationStore, SessionStore, TodoStore},
    tools::ntfy::NtfyClient,
};
use agentix_tts::TtsEngine;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct AppState {
    pub db: AgentDb,
    pub config: AgentConfig,
    pub ntfy: NtfyClient,
    pub google_auth: Option<Arc<crate::google::auth::GoogleAuth>>,
    pub prometheus_handle: metrics_exporter_prometheus::PrometheusHandle,
    pub tts_engine: Option<Arc<TtsEngine>>,
}

impl AppState {
    pub fn todo_store(&self) -> TodoStore {
        TodoStore(self.db.clone())
    }

    pub fn notif_store(&self) -> NotificationStore {
        NotificationStore(self.db.clone())
    }

    pub fn session_store(&self) -> SessionStore {
        SessionStore(self.db.clone())
    }

    pub fn gmail_cursor_store(&self) -> GmailCursorStore {
        GmailCursorStore(self.db.clone())
    }
}

pub fn spawn(state: Arc<AppState>, token: CancellationToken) {
    let s = Arc::clone(&state);
    let t = token.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = notification::run_notification_loop(
                s.todo_store(),
                s.notif_store(),
                s.ntfy.clone(),
                s.config.scheduler.clone(),
            ) => {}
            _ = t.cancelled() => {}
        }
    });

    if let Some(auth) = state.google_auth.as_ref().cloned() {
        let gmail_client = Arc::new(crate::google::gmail::GmailClient::new(auth));
        let s2 = Arc::clone(&state);
        let t2 = token;
        tokio::spawn(async move {
            tokio::select! {
                _ = gmail_scan::run_gmail_scan_loop(
                    gmail_client,
                    s2.todo_store(),
                    s2.notif_store(),
                    s2.gmail_cursor_store(),
                    s2.config.scheduler.gmail_scan_interval_secs,
                ) => {}
                _ = t2.cancelled() => {}
            }
        });
    }
}
