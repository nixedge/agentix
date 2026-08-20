use axum::{extract::State, Json};
use serde_json::json;
use std::sync::Arc;

use crate::scheduler::AppState;

pub async fn health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let gateway_status = check_gateway(&state.config.gateway_url).await;
    let google_auth_status = if state.google_auth.is_some() {
        "ok"
    } else {
        "unconfigured"
    };

    Json(json!({
        "status": "ok",
        "db": "ok",
        "gateway": gateway_status,
        "google_auth": google_auth_status,
    }))
}

async fn check_gateway(gateway_url: &str) -> &'static str {
    let url = format!("{}/health", gateway_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build();

    match client {
        Ok(c) => match c.head(&url).send().await {
            Ok(resp) if resp.status().is_success() => "ok",
            _ => "unreachable",
        },
        Err(_) => "unreachable",
    }
}
