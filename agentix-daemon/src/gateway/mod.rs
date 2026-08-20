mod anthropic;
mod health;
mod ollama_manage;
mod openai_proxy;
mod proxy;
mod transcription_handler;

use crate::config::Config;
use agentix_router::{RouteTarget, Router as ModelRouter};
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
}

pub fn router(model_router: Arc<ModelRouter>, config: Config) -> anyhow::Result<Router> {
    let http = reqwest::Client::builder()
        .user_agent("agentix-daemon/0.1")
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .context("failed to build HTTP client")?;

    let state = AppState {
        model_router,
        llama_socket: config.llama_socket.clone(),
        whisper_socket: config.whisper_socket.clone(),
        config,
        http,
    };

    Ok(Router::new()
        .route("/health", get(health::handler))
        .route("/v1/models", get(models_handler))
        .route("/v1/chat/completions", post(chat_completions_handler))
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
        // Ollama-compatible model management endpoints — proxied to agentix-llama
        .route("/api/pull", post(ollama_manage::pull_handler))
        .route("/api/delete", delete(ollama_manage::delete_handler))
        .route("/api/tags", get(ollama_manage::tags_handler))
        .route("/api/show", post(ollama_manage::show_handler))
        .layer(TraceLayer::new_for_http())
        .with_state(state))
}

async fn chat_completions_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let req: agentix_api::ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid request: {e}")).into_response()
        }
    };

    let target = state.model_router.route(&req.model);
    tracing::debug!(model = %req.model, target = ?target, "routing chat completion");

    match target {
        RouteTarget::Anthropic => anthropic::proxy_chat(&state, headers, body).await,
        RouteTarget::OpenAI => openai_proxy::proxy_chat(&state, headers, body).await,
        RouteTarget::OpenRouter => openai_proxy::proxy_openrouter(&state, headers, body).await,
        RouteTarget::Local => {
            proxy::forward(
                &state.llama_socket,
                axum::http::Method::POST,
                "/v1/chat/completions",
                headers,
                Body::from(body),
            )
            .await
        }
    }
}

async fn embeddings_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let model = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["model"].as_str().map(str::to_string))
        .unwrap_or_default();
    tracing::debug!(model = %model, "embeddings request");

    // Try the local llama backend first; fall back to Ollama if unavailable
    let resp = proxy::forward(
        &state.llama_socket,
        axum::http::Method::POST,
        "/v1/embeddings",
        headers.clone(),
        Body::from(body.clone()),
    )
    .await;

    if resp.status() == StatusCode::SERVICE_UNAVAILABLE {
        let url = format!("{}/v1/embeddings", state.config.ollama_base_url);
        tracing::info!(model = %model, ollama_url = %url, "llama socket unavailable, falling back to Ollama");
        return match state
            .http
            .post(&url)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(r) => openai_proxy::relay_response(r).await,
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                format!("embeddings proxy error: {e}"),
            )
                .into_response(),
        };
    }

    resp
}

async fn ollama_embed_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let resp = proxy::forward(
        &state.llama_socket,
        axum::http::Method::POST,
        "/api/embed",
        headers,
        Body::from(body.clone()),
    )
    .await;

    if resp.status() == StatusCode::SERVICE_UNAVAILABLE {
        let url = format!("{}/api/embed", state.config.ollama_base_url);
        return match state
            .http
            .post(&url)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(r) => openai_proxy::relay_response(r).await,
            Err(e) => (StatusCode::BAD_GATEWAY, format!("embed proxy error: {e}")).into_response(),
        };
    }
    resp
}

async fn messages_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    anthropic::proxy_messages(&state, headers, body).await
}

async fn responses_proxy_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    proxy::forward(
        &state.llama_socket,
        axum::http::Method::POST,
        "/v1/responses",
        headers,
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

    if state.config.anthropic_api_key.is_some() {
        models.push(
            serde_json::json!({"id":"claude-opus-4-7","object":"model","owned_by":"anthropic"}),
        );
        models.push(
            serde_json::json!({"id":"claude-sonnet-4-6","object":"model","owned_by":"anthropic"}),
        );
        models.push(serde_json::json!({"id":"claude-haiku-4-5-20251001","object":"model","owned_by":"anthropic"}));
    }
    if state.config.openai_api_key.is_some() {
        models.push(serde_json::json!({"id":"gpt-4o","object":"model","owned_by":"openai"}));
    }
    if state.config.openrouter_api_key.is_some() {
        models.push(
            serde_json::json!({"id":"openrouter/*","object":"model","owned_by":"openrouter"}),
        );
    }

    axum::Json(serde_json::json!({ "object": "list", "data": models }))
}
