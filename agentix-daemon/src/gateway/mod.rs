mod anthropic;
mod health;
mod ollama_manage;
mod openai_proxy;
mod proxy;
mod speech_handler;
mod transcription_handler;
mod voices_handler;

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
    pub qwen_socket: PathBuf,
    pub qwen_model: String,
    pub whisper_socket: PathBuf,
    pub piper_bin: PathBuf,
    pub piper_voices_dir: PathBuf,
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
        qwen_socket: config.qwen_socket.clone(),
        qwen_model: config.qwen_model.clone(),
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

    if is_qwen_model(&req.model, &state.qwen_model) {
        return qwen_chat_completion(&state, headers, body).await;
    }

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

fn is_qwen_model(requested_model: &str, configured_model: &str) -> bool {
    requested_model == configured_model
}

async fn qwen_chat_completion(
    state: &AppState,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let response = proxy::forward(
        &state.qwen_socket,
        axum::http::Method::POST,
        "/v1/chat/completions",
        headers,
        Body::from(body),
    )
    .await;
    if response.status() == StatusCode::SERVICE_UNAVAILABLE {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({
                "error": {
                    "message": "The configured Qwen worker is unavailable",
                    "type": "model_unavailable",
                    "code": "model_unavailable"
                }
            })),
        )
            .into_response();
    }
    response
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
    if is_qwen_model(&model, &state.qwen_model) {
        return unsupported_qwen_endpoint("Qwen worker does not provide embeddings");
    }
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
    if body_requests_qwen(&body, &state.qwen_model) {
        return unsupported_qwen_endpoint("Qwen worker does not provide embeddings");
    }
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
    if body_requests_qwen(&body, &state.qwen_model) {
        return unsupported_qwen_endpoint(
            "Qwen worker does not provide the Anthropic messages API",
        );
    }
    anthropic::proxy_messages(&state, headers, body).await
}

async fn responses_proxy_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if body_requests_qwen(&body, &state.qwen_model) {
        return unsupported_qwen_endpoint("Qwen worker does not provide the responses API");
    }
    proxy::forward(
        &state.llama_socket,
        axum::http::Method::POST,
        "/v1/responses",
        headers,
        Body::from(body),
    )
    .await
}

fn body_requests_qwen(body: &[u8], configured_model: &str) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|request| request["model"].as_str().map(str::to_string))
        .is_some_and(|model| is_qwen_model(&model, configured_model))
}

fn unsupported_qwen_endpoint(message: &str) -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        axum::Json(serde_json::json!({
            "error": {"message": message, "type": "unsupported_capability", "code": "unsupported_capability"}
        })),
    )
        .into_response()
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

    let qwen_resp = proxy::forward(
        &state.qwen_socket,
        axum::http::Method::GET,
        "/v1/models",
        HeaderMap::new(),
        Body::empty(),
    )
    .await;
    if qwen_resp.status().is_success() {
        let body_bytes = axum::body::to_bytes(qwen_resp.into_body(), 1 << 20).await;
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

#[cfg(test)]
mod qwen_routing_tests {
    use super::{body_requests_qwen, is_qwen_model, qwen_chat_completion, AppState, ModelRouter};
    use axum::{body::Bytes, http::HeaderMap};
    use std::{error::Error, sync::Arc};

    #[test]
    fn qwen_routing_requires_exact_configured_model_id() {
        assert!(is_qwen_model("qwen3.8-flash-next", "qwen3.8-flash-next"));
        assert!(!is_qwen_model("Qwen3.8-flash-next", "qwen3.8-flash-next"));
        assert!(!is_qwen_model(
            "qwen3.8-flash-next:iq3_s",
            "qwen3.8-flash-next"
        ));
        assert!(!is_qwen_model("qwen2.5-coder", "qwen3.8-flash-next"));
    }

    #[test]
    fn qwen_endpoint_detection_is_exact_and_rejects_malformed_json() {
        assert!(body_requests_qwen(
            br#"{"model":"qwen3.8-flash-next"}"#,
            "qwen3.8-flash-next"
        ));
        assert!(!body_requests_qwen(
            br#"{"model":"qwen3.8-flash-next:iq3_s"}"#,
            "qwen3.8-flash-next"
        ));
        assert!(!body_requests_qwen(b"not json", "qwen3.8-flash-next"));
    }

    #[tokio::test]
    async fn unavailable_qwen_socket_returns_typed_error_without_fallback(
    ) -> Result<(), Box<dyn Error>> {
        let socket = std::env::temp_dir().join(format!(
            "agentix-qwen-unavailable-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&socket);
        let mut config = crate::config::Config::from_env();
        config.qwen_socket = socket.clone();
        config.llama_socket = socket.with_file_name("agentix-qwen-llama-must-not-be-used.sock");
        let state = AppState {
            model_router: Arc::new(ModelRouter::new()),
            qwen_socket: config.qwen_socket.clone(),
            qwen_model: config.qwen_model.clone(),
            llama_socket: config.llama_socket.clone(),
            whisper_socket: config.whisper_socket.clone(),
            piper_bin: config.piper_bin.clone(),
            piper_voices_dir: config.piper_voices_dir.clone(),
            config,
            http: reqwest::Client::new(),
        };
        let response = qwen_chat_completion(
            &state,
            HeaderMap::new(),
            Bytes::from_static(br#"{"model":"qwen3.8-flash-next"}"#),
        )
        .await;
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        let body = axum::body::to_bytes(response.into_body(), 4096).await?;
        let value: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(value["error"]["code"], "model_unavailable");
        assert!(!socket.exists());
        Ok(())
    }
}
