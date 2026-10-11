mod escalation;
mod health;
mod ollama_manage;
mod proxy;
mod speech_handler;
mod transcription_handler;
mod voices_handler;

use crate::config::Config;
use agentix_router::Router as ModelRouter;
use anyhow::Context as _;
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use std::{path::PathBuf, sync::Arc};
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub model_router: Arc<ModelRouter>,
    pub config: Config,
    pub http: reqwest::Client,
    pub llama_socket: PathBuf,
    pub whisper_socket: PathBuf,
    pub piper_bin: PathBuf,
    pub piper_voices_dir: PathBuf,
}

pub fn router(model_router: Arc<ModelRouter>, config: Config) -> anyhow::Result<Router> {
    let http = reqwest::Client::builder()
        .user_agent("agentix-daemon/0.1")
        .timeout(std::time::Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build HTTP client")?;

    let state = AppState {
        model_router,
        llama_socket: config.llama_socket.clone(),
        whisper_socket: config.whisper_socket.clone(),
        piper_bin: config.piper_bin.clone(),
        piper_voices_dir: config.piper_voices_dir.clone(),
        config,
        http,
    };

    Ok(Router::new()
        .route("/health", get(health::handler))
        .route("/v1/models", get(models_handler))
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/v1/agentix/escalations", post(escalation::handler))
        .route("/v1/agentix/models/readiness", post(readiness_handler))
        .route("/v1/agentix/models", get(catalog_handler))
        .route("/v1/agentix/models/tokenize", post(tokenize_handler))
        .route("/v1/responses", post(responses_proxy_handler))
        .route("/v1/embeddings", post(embeddings_handler))
        // Anthropic-native endpoint (for clients using the Anthropic SDK directly)
        .route("/v1/messages", post(messages_handler))
        // Ollama-compatible embedding endpoint (used by ingest/mcp-server)
        .route("/api/embed", post(ollama_embed_handler))
        // Audio transcription (OpenAI-compatible) — proxied to agentix-whisper
        .route(
            "/v1/audio/transcriptions",
            post(transcription_handler::handler),
        )
        // Text-to-speech (OpenAI-compatible) — Piper TTS subprocess
        .route("/v1/audio/speech", post(speech_handler::handler))
        // Piper voice model management
        .route("/v1/audio/voices", get(voices_handler::list_handler))
        .route("/v1/audio/voices", post(voices_handler::pull_handler))
        // Ollama-compatible model management endpoints — proxied to agentix-llama
        .route("/api/pull", post(ollama_manage::pull_handler))
        .route("/api/delete", delete(ollama_manage::delete_handler))
        .route("/api/tags", get(ollama_manage::tags_handler))
        .route("/api/show", post(ollama_manage::show_handler))
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state))
}

async fn local_body(state: &AppState, body: &[u8]) -> Result<Vec<u8>, Response> {
    let mut value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| escalation::failure(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let model = value["model"]
        .as_str()
        .ok_or_else(|| escalation::failure(StatusCode::BAD_REQUEST, "missing_model"))?;
    let installed = local_ids(state).await;
    let model = state
        .model_router
        .local_model(model, &installed)
        .map_err(|code| {
            escalation::failure(
                if code == "ambiguous_local_model" {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::FORBIDDEN
                },
                code,
            )
        })?;
    value["model"] = serde_json::Value::String(model);
    serde_json::to_vec(&value)
        .map_err(|_| escalation::failure(StatusCode::BAD_REQUEST, "invalid_request"))
}

async fn local_ids(state: &AppState) -> Vec<String> {
    let request = proxy::forward(
        &state.llama_socket,
        axum::http::Method::GET,
        "/v1/models",
        HeaderMap::new(),
        Body::empty(),
    );
    let response = match tokio::time::timeout(std::time::Duration::from_secs(5), request).await {
        Ok(response) if response.status().is_success() => response,
        _ => return vec![],
    };
    let Ok(bytes) = axum::body::to_bytes(response.into_body(), 1024 * 1024).await else {
        return vec![];
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return vec![];
    };
    value["data"]
        .as_array()
        .map(|data| {
            data.iter()
                .filter_map(|model| model["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

async fn local_forward(
    state: &AppState,
    mut headers: HeaderMap,
    body: &[u8],
    path: &str,
) -> Response {
    let body = match local_body(state, body).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    headers.remove("content-length");
    headers.remove("authorization");
    proxy::forward(
        &state.llama_socket,
        axum::http::Method::POST,
        path,
        headers,
        Body::from(body),
    )
    .await
}

async fn chat_completions_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    local_forward(&state, headers, &body, "/v1/chat/completions").await
}
async fn embeddings_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    local_embed(&state, headers, &body, "/v1/embeddings").await
}
async fn ollama_embed_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    local_embed(&state, headers, &body, "/api/embed").await
}

async fn local_embed(
    state: &AppState,
    mut headers: HeaderMap,
    body: &[u8],
    path: &str,
) -> Response {
    let body = match local_body(state, body).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    headers.remove("content-length");
    headers.remove("authorization");
    let response = proxy::forward(
        &state.llama_socket,
        axum::http::Method::POST,
        path,
        headers,
        Body::from(body.clone()),
    )
    .await;
    if response.status() != StatusCode::SERVICE_UNAVAILABLE {
        return response;
    }
    match state
        .http
        .post(format!(
            "{}{path}",
            state.config.ollama_base_url.trim_end_matches('/')
        ))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let stream = response.bytes_stream();
            (
                status,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                Body::from_stream(stream),
            )
                .into_response()
        }
        Err(_) => escalation::failure(StatusCode::BAD_GATEWAY, "local_embedding_unavailable"),
    }
}
async fn messages_handler(
    State(state): State<AppState>,
    _headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    match local_body(&state, &body).await {
        Err(response) => response,
        Ok(_) => escalation::failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "local_messages_unsupported",
        ),
    }
}
async fn responses_proxy_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    local_forward(&state, headers, &body, "/v1/responses").await
}
async fn readiness_handler(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    if agentix_api::wire::decode_request::<agentix_api::models::ReadinessRequest>(&body).is_err() {
        return escalation::failure(StatusCode::BAD_REQUEST, "invalid_readiness_request");
    }
    proxy::forward(
        &state.llama_socket,
        axum::http::Method::POST,
        "/control/readiness",
        HeaderMap::new(),
        Body::from(body),
    )
    .await
}
async fn catalog_handler(State(state): State<AppState>, uri: axum::http::Uri) -> Response {
    let path = match uri.query() {
        Some(query) => format!("/control/models?{query}"),
        None => "/control/models".into(),
    };
    proxy::forward(
        &state.llama_socket,
        axum::http::Method::GET,
        &path,
        HeaderMap::new(),
        Body::empty(),
    )
    .await
}
async fn tokenize_handler(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    if agentix_api::wire::decode_request::<agentix_api::models::TokenizeRequest>(&body).is_err() {
        return escalation::failure(StatusCode::BAD_REQUEST, "invalid_tokenize_request");
    }
    proxy::forward(
        &state.llama_socket,
        axum::http::Method::POST,
        "/control/tokenize",
        HeaderMap::new(),
        Body::from(body),
    )
    .await
}

async fn models_handler(State(state): State<AppState>) -> impl IntoResponse {
    let mut models = vec![];

    // Ask the llama backend for its local models (best-effort)
    let llama_resp = proxy::forward(
        &state.llama_socket,
        axum::http::Method::GET,
        "/v1/models",
        HeaderMap::new(),
        Body::empty(),
    )
    .await;

    if llama_resp.status().is_success() {
        let body_bytes = axum::body::to_bytes(llama_resp.into_body(), 1 << 20).await;
        if let Ok(bytes) = body_bytes {
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(data) = val["data"].as_array() {
                    models.extend_from_slice(data);
                }
            }
        }
    }

    axum::Json(serde_json::json!({ "object": "list", "data": models }))
}
