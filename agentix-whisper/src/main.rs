//! Standalone whisper daemon — listens on a Unix socket and serves
//! POST /v1/audio/transcriptions (OpenAI-compatible),
//! POST /api/pull, GET /v1/models, DELETE /api/delete (model management), and
//! POST /control/shutdown (graceful drain-and-exit for VRAM reclaim).
//!
//! Environment variables:
//!   AGENTIX_WHISPER_SOCKET  Unix socket path (default /run/agentix/whisper.sock)
//!   AGENTIX_MODELS_DIR      Model store directory (default /var/lib/agentix/models)
//!   AGENTIX_WHISPER_MODELS  Comma-separated models to pull (if absent) and load at startup

use agentix_infer::{InferConfig, InferEngine};
use agentix_whisper::{decode_audio_to_pcm, WhisperBackend};
use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{oneshot, Mutex};
use tracing::info;

#[derive(Clone)]
struct AppState {
    engine: InferEngine,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agentix_whisper=info".into()),
        )
        .init();

    let socket_path = std::env::var("AGENTIX_WHISPER_SOCKET")
        .unwrap_or_else(|_| "/run/agentix/whisper.sock".to_string());

    let models_dir = std::env::var("AGENTIX_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/lib/agentix/models"));

    let cfg = InferConfig::new(models_dir, None, 1, 0);
    let engine = InferEngine::new(cfg).await?;
    engine.register_backend(Arc::new(WhisperBackend));

    for model in parse_model_list("AGENTIX_WHISPER_MODELS") {
        info!(model = %model, "preloading whisper model");
        match engine.pull(&model).await {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(model = %model, err = %e, "preload pull failed — skipping");
                continue;
            }
        }
        if let Err(e) = engine.warmup(&model).await {
            tracing::warn!(model = %model, err = %e, "preload warmup failed");
        }
    }

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let state = AppState {
        engine,
        shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
    };

    let router = Router::new()
        .route("/v1/audio/transcriptions", post(transcription_handler))
        .route("/v1/models", get(models_handler))
        .route("/api/pull", post(pull_handler))
        .route("/api/delete", delete(delete_handler))
        .route("/control/shutdown", post(shutdown_handler))
        .with_state(state);

    // Remove stale socket from a previous run before binding.
    let _ = std::fs::remove_file(&socket_path);
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    info!(socket = %socket_path, "agentix-whisper listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_rx.await.ok();
        })
        .await?;

    Ok(())
}

async fn models_handler(State(state): State<AppState>) -> Response {
    let models: Vec<serde_json::Value> = state
        .engine
        .list()
        .await
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "id": m.name,
                "object": "model",
                "owned_by": "local",
            })
        })
        .collect();
    Json(serde_json::json!({ "object": "list", "data": models })).into_response()
}

async fn pull_handler(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let name = match serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["name"].as_str().map(str::to_string))
    {
        Some(n) => n,
        None => return (StatusCode::BAD_REQUEST, "missing field: name").into_response(),
    };

    tracing::info!(model = %name, "pull requested");
    match state.engine.pull(&name).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("pull failed: {e}"),
        )
            .into_response(),
    }
}

async fn delete_handler(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let name = match serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["name"].as_str().map(str::to_string))
    {
        Some(n) => n,
        None => return (StatusCode::BAD_REQUEST, "missing field: name").into_response(),
    };

    tracing::info!(model = %name, "delete requested");
    match state.engine.remove(&name).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("delete failed: {e}"),
        )
            .into_response(),
    }
}

async fn shutdown_handler(State(state): State<AppState>) -> Response {
    if let Some(tx) = state.shutdown_tx.lock().await.take() {
        let _ = tx.send(());
        info!("shutdown requested via /control/shutdown");
        StatusCode::OK.into_response()
    } else {
        (StatusCode::GONE, "already shutting down").into_response()
    }
}

async fn transcription_handler(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Response {
    let mut audio_bytes: Option<Vec<u8>> = None;
    let mut model: Option<String> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("multipart parse error: {e}"),
                )
                    .into_response()
            }
        };

        let name = match field.name() {
            Some(n) => n.to_string(),
            None => continue,
        };

        match name.as_str() {
            "file" => match field.bytes().await {
                Ok(b) => audio_bytes = Some(b.to_vec()),
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        format!("failed to read audio field: {e}"),
                    )
                        .into_response()
                }
            },
            "model" => match field.text().await {
                Ok(t) => model = Some(t),
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        format!("failed to read model field: {e}"),
                    )
                        .into_response()
                }
            },
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let audio = match audio_bytes {
        Some(b) => b,
        None => return (StatusCode::BAD_REQUEST, "missing required field: file").into_response(),
    };

    // If client omits the model field, pick the first loaded whisper model.
    let model = match model {
        Some(m) => m,
        None => {
            let loaded = state.engine.list().await;
            match loaded.into_iter().next() {
                Some(m) => m.name,
                None => {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "no whisper model is loaded — pull one first with POST /api/pull",
                    )
                        .into_response()
                }
            }
        }
    };

    if audio.is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, "audio file is empty").into_response();
    }

    tracing::info!(model = %model, audio_bytes = audio.len(), "transcription request");

    let pcm = match decode_audio_to_pcm(audio).await {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("audio decode failed: {e}"),
            )
                .into_response()
        }
    };

    match state.engine.transcribe_pcm(&model, &pcm).await {
        Ok(text) => Json(agentix_api::TranscriptionResponse { text }).into_response(),
        Err(agentix_infer::InferError::ModelNotFound(_)) => (
            StatusCode::NOT_FOUND,
            format!("model '{model}' not found — pull it first with POST /api/pull"),
        )
            .into_response(),
        Err(agentix_infer::InferError::CapabilityMissing(m, _)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("model '{m}' does not support transcription"),
        )
            .into_response(),
        Err(agentix_infer::InferError::Transcription(e)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("transcription failed: {e}"),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("internal error: {e}"),
        )
            .into_response(),
    }
}

fn parse_model_list(var: &str) -> Vec<String> {
    std::env::var(var)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
