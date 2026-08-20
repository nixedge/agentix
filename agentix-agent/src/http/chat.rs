use axum::{
    extract::{Multipart, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

use crate::{scheduler::AppState, tools};

#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
}

#[derive(Serialize)]
pub struct ChatResponse {
    pub reply: String,
    pub session_id: String,
}

fn extract_session(headers: &HeaderMap) -> Uuid {
    headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(Uuid::new_v4)
}

pub async fn chat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, Json<serde_json::Value>)> {
    let session_id = extract_session(&headers);
    let session_store = state.session_store();

    let mut messages = session_store
        .load(session_id)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    messages.push(json!({ "role": "user", "content": req.message }));

    let agent =
        tools::build_agent_loop(&state.config.gateway_url, &state.config.local_model, &state);
    let output = agent
        .run(&req.message)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    messages.push(json!({ "role": "assistant", "content": output.answer }));

    session_store
        .save(session_id, &messages)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    metrics::counter!("agentix_chat_requests_total").increment(1);

    Ok(Json(ChatResponse {
        reply: output.answer,
        session_id: session_id.to_string(),
    }))
}

pub async fn chat_voice(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let session_id = extract_session(&headers);

    let mut audio_bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad_request(e.to_string()))?
    {
        if field.name() == Some("audio") {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| bad_request(e.to_string()))?;
            audio_bytes = Some(bytes.to_vec());
        }
    }

    let audio = audio_bytes.ok_or_else(|| bad_request("missing audio field".to_owned()))?;

    let transcript = forward_to_whisper(&state.config.gateway_url, audio)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    let agent =
        tools::build_agent_loop(&state.config.gateway_url, &state.config.local_model, &state);
    let output = agent
        .run(&transcript)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    let session_store = state.session_store();
    let mut messages = session_store
        .load(session_id)
        .await
        .map_err(|e| internal_error(e.to_string()))?;
    messages.push(json!({ "role": "user", "content": transcript }));
    messages.push(json!({ "role": "assistant", "content": output.answer }));
    session_store
        .save(session_id, &messages)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    metrics::counter!("agentix_chat_requests_total").increment(1);

    Ok(Json(json!({
        "transcript": transcript,
        "reply": output.answer,
        "session_id": session_id.to_string(),
    })))
}

async fn forward_to_whisper(gateway_url: &str, audio: Vec<u8>) -> anyhow::Result<String> {
    let url = format!(
        "{}/v1/audio/transcriptions",
        gateway_url.trim_end_matches('/')
    );
    let part = reqwest::multipart::Part::bytes(audio)
        .file_name("audio.webm")
        .mime_str("audio/webm")?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", "whisper-1");
    let client = reqwest::Client::new();
    let resp = client.post(&url).multipart(form).send().await?;
    anyhow::ensure!(
        resp.status().is_success(),
        "whisper endpoint returned {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await?;
    body["text"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("no text in whisper response"))
}

fn internal_error(msg: String) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
}

fn bad_request(msg: String) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg })))
}
