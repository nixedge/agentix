pub mod chat;
pub mod health;
pub mod todos;
pub mod ui;

use axum::{routing::get, routing::patch, routing::post, Router};
use std::sync::Arc;

use crate::scheduler::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(ui::index))
        .route("/health", get(health::health))
        .route("/chat", post(chat::chat))
        .route("/chat/voice", post(chat::chat_voice))
        .route("/todos", get(todos::list_todos))
        .route("/todos", post(todos::create_todo))
        .route("/todos/:id", patch(todos::update_todo))
        .with_state(state)
}
