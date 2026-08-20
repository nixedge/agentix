pub mod health;

use axum::{Router, routing::get};
use std::sync::Arc;

use crate::scheduler::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .with_state(state)
}
