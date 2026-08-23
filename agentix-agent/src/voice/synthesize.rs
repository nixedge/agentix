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
    let engine = match state.tts_engine.as_ref() {
        Some(e) => Arc::clone(e),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "TTS not configured"})),
            )
                .into_response();
        }
    };

    let _ = req.voice; // voice selection deferred until backend supports it
    let _ = req.rate;

    match engine.synthesize(&req.text).await {
        Ok(samples) => match engine.to_wav(&samples) {
            Ok(wav) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "audio/wav")
                .body(axum::body::Body::from(wav))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("WAV encoding failed: {e}")})),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("synthesis failed: {e}")})),
        )
            .into_response(),
    }
}
