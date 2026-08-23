use crate::scheduler::AppState;
use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

pub async fn handler(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let daemon_url = state
        .config
        .voice
        .as_ref()
        .map(|v| v.daemon_url.clone())
        .unwrap_or_else(|| "http://127.0.0.1:8766".to_string());

    // Extract the audio file from the multipart body
    let mut file_bytes: Option<(Vec<u8>, String)> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            let content_type = field.content_type().unwrap_or("audio/wav").to_string();
            match field.bytes().await {
                Ok(b) => {
                    file_bytes = Some((b.to_vec(), content_type));
                    break;
                }
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": format!("reading file field: {e}")})),
                    )
                        .into_response();
                }
            }
        }
    }

    let (bytes, content_type) = match file_bytes {
        Some(f) => f,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing 'file' field in multipart"})),
            )
                .into_response();
        }
    };

    // Forward to agentix-daemon
    let client = reqwest::Client::new();
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name("audio.wav")
        .mime_str(&content_type)
        .unwrap_or_else(|_| reqwest::multipart::Part::bytes(vec![]));
    let form = reqwest::multipart::Form::new().part("file", part);

    match client
        .post(format!("{daemon_url}/v1/audio/transcriptions"))
        .multipart(form)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            match resp.json::<serde_json::Value>().await {
                Ok(body) => (
                    StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK),
                    Json(body),
                )
                    .into_response(),
                Err(e) => (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"error": format!("parsing daemon response: {e}")})),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": format!("daemon request failed: {e}")})),
        )
            .into_response(),
    }
}
