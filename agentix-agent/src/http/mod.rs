pub mod chat;
pub mod health;
pub mod metrics;
pub mod todos;
pub mod ui;

use axum::{
    http::StatusCode,
    routing::{get, patch, post},
    Json, Router,
};
use std::sync::Arc;

use crate::scheduler::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(ui::index))
        .route("/health", get(health::health))
        .route("/metrics", get(metrics::metrics_handler))
        .route("/chat", post(chat::chat))
        .route("/chat/voice", post(chat::chat_voice))
        .route("/todos", get(todos::list_todos))
        .route("/todos", post(todos::create_todo))
        .route("/todos/:id", patch(todos::update_todo))
        .route("/voice/stream", post(voice_stream_stub))
        .with_state(state)
}

async fn voice_stream_stub() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "ESP32 voice stream not yet implemented",
            "seam": "reserved"
        })),
    )
}
