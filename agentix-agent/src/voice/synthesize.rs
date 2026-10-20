use crate::scheduler::AppState;
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct SynthRequest {
    pub text: String,
    pub voice: Option<String>,
    pub rate: Option<f32>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SynthRequest>,
) -> impl IntoResponse {
    let Some(voice_cfg) = state.config.voice.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "voice not configured"})),
        )
            .into_response();
    };

    let model = req
        .voice
        .as_deref()
        .unwrap_or(&voice_cfg.tts_voice)
        .to_owned();
    let daemon_url = &voice_cfg.daemon_url;

    let _ = req.rate; // speed control deferred until piper supports it

    let speech_resp = match state
        .http
        .post(format!("{daemon_url}/v1/audio/speech"))
        .json(&serde_json::json!({"input": req.text, "model": model}))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": format!("TTS request failed: {e}")})),
            )
                .into_response();
        }
    };

    if !speech_resp.status().is_success() {
        let status = speech_resp.status();
        let body = speech_resp.bytes().await.unwrap_or_default();
        return Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body))
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response());
    }

    let wav = match speech_resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": format!("reading TTS response: {e}")})),
            )
                .into_response();
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "audio/wav")
        .body(axum::body::Body::from(wav))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
