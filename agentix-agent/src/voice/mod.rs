pub mod synthesize;
pub mod transcribe;

use crate::scheduler::AppState;
use axum::{routing::post, Router};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/transcribe", post(transcribe::handler))
        .route("/synthesize", post(synthesize::handler))
}
