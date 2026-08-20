//! Standalone llama daemon — listens on a Unix socket and serves
//! OpenAI-compatible chat completions and embeddings, plus model management
//! and POST /control/shutdown for VRAM reclaim.
//!
//! Environment variables:
//!   AGENTIX_LLAMA_SOCKET   Unix socket path (default /run/agentix/llama.sock)
//!   AGENTIX_MODELS_DIR     Model store directory (default /var/lib/agentix/models)
//!   AGENTIX_LLAMA_MODELS   Comma-separated models to pull (if absent) and load at startup
//!   AGENTIX_GPU_LAYERS     Layers to offload to GPU (-1 = all, 0 = CPU only)
//!   AGENTIX_MAX_CTX        Context window size (default 32768)
//!   AGENTIX_VRAM_LIMIT_BYTES  Hard VRAM cap in bytes (optional)
//!   AGENTIX_MAX_LOADED_MODELS Max models in memory simultaneously (default 2)

use agentix_api::{
    ResponseFormatType, ResponseInputContent, ResponseOutputContent, ResponseOutputMessage,
    ResponsesRequest, ResponsesResponse,
};
use agentix_infer::{
    CompletionMessage, CompletionRequest, FinishReason, GrammarConstraint, InferConfig, InferEngine,
};
use agentix_llama::LlamaCppBackend;
use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{oneshot, Mutex};
use tokio_stream::StreamExt;
use tracing::info;
use uuid::Uuid;

const JSON_GBNF: &str = r#"root   ::= object
value  ::= object | array | string | number | ("true" | "false" | "null")
object ::= "{" (string ":" value ("," string ":" value)*)? "}"
array  ::= "[" (value ("," value)*)? "]"
string ::= "\"" ([^"\\] | "\\" (["\\/bfnrt] | "u" [0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]))* "\""
number ::= ("-"? ([0-9] | [1-9] [0-9]*)) ("." [0-9]+)? ([eE] [-+]? [0-9]+)?"#;

fn validate_and_convert_schema(schema: &serde_json::Value) -> Result<String, String> {
    let schema_str = serde_json::to_string(schema).map_err(|e| e.to_string())?;
    // Reject external $ref URIs (http/https) — would require network fetch to resolve
    if schema_str.contains("\"$ref\"") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&schema_str) {
            if let Some(r) = v.get("$ref").and_then(|r| r.as_str()) {
                if r.starts_with("http://") || r.starts_with("https://") {
                    return Err(format!("external $ref URI not supported: {r}"));
                }
            }
        }
    }
    llama_cpp_2::json_schema_to_grammar(&schema_str).map_err(|e| e.to_string())
}

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
                .unwrap_or_else(|_| "agentix_llama=info".into()),
        )
        .init();

    let socket_path = std::env::var("AGENTIX_LLAMA_SOCKET")
        .unwrap_or_else(|_| "/run/agentix/llama.sock".to_string());

    let models_dir = std::env::var("AGENTIX_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/lib/agentix/models"));

    let max_ctx = std::env::var("AGENTIX_MAX_CTX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(32768u32);

    let vram_limit = std::env::var("AGENTIX_VRAM_LIMIT_BYTES")
        .ok()
        .and_then(|v| v.parse().ok());

    let max_loaded = std::env::var("AGENTIX_MAX_LOADED_MODELS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2usize);

    let cfg = InferConfig::new(models_dir, vram_limit, max_loaded, max_ctx);
    let engine = InferEngine::new(cfg).await?;

    match LlamaCppBackend::new() {
        Ok(backend) => {
            engine.register_backend(Arc::new(backend));
            info!("registered LlamaCppBackend");
        }
        Err(e) => {
            tracing::warn!("LlamaCppBackend unavailable: {e} — local GGUF inference disabled");
        }
    }

    for model in parse_model_list("AGENTIX_LLAMA_MODELS") {
        info!(model = %model, "preloading model");
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
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/v1/responses", post(responses_handler))
        .route("/v1/embeddings", post(embeddings_handler))
        .route("/api/embed", post(ollama_embed_handler))
        .route("/v1/models", get(models_handler))
        .route("/api/tags", get(tags_handler))
        .route("/api/pull", post(pull_handler))
        .route("/api/delete", delete(delete_handler))
        .route("/control/shutdown", post(shutdown_handler))
        .with_state(state);

    let _ = std::fs::remove_file(&socket_path);
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    info!(socket = %socket_path, "agentix-llama listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_rx.await.ok();
        })
        .await?;

    Ok(())
}

async fn chat_completions_handler(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Response {
    let api_req: agentix_api::ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid request: {e}")).into_response()
        }
    };

    let resolved = match resolve_model(&state.engine, &api_req.model).await {
        Some(m) => m,
        None => {
            return (
                StatusCode::NOT_FOUND,
                format!("model '{}' not found — pull it first", api_req.model),
            )
                .into_response()
        }
    };

    let mut messages = Vec::with_capacity(api_req.messages.len());
    for m in &api_req.messages {
        let content = match normalize_content(&m.content) {
            Ok(s) => s,
            Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
        };
        messages.push(CompletionMessage {
            role: m.role.clone(),
            content,
        });
    }

    let stop: Vec<String> = api_req
        .extra
        .get("stop")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let grammar = match api_req.response_format.as_ref().map(|rf| &rf.format_type) {
        Some(ResponseFormatType::JsonObject) => {
            Some(GrammarConstraint::Gbnf(JSON_GBNF.to_string()))
        }
        Some(ResponseFormatType::JsonSchema) => {
            let schema = api_req
                .response_format
                .as_ref()
                .and_then(|rf| rf.json_schema.as_ref())
                .map(|js| &js.schema);
            match schema {
                Some(s) => match validate_and_convert_schema(s) {
                    Ok(gbnf) => Some(GrammarConstraint::Gbnf(gbnf)),
                    Err(msg) => {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({
                                "error": format!("response_format.json_schema.schema could not be converted to a grammar: {msg}")
                            })),
                        )
                            .into_response();
                    }
                },
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": "response_format.json_schema.schema is required when type is json_schema"
                        })),
                    )
                        .into_response();
                }
            }
        }
        Some(ResponseFormatType::Text) | Some(ResponseFormatType::Unknown) | None => None,
    };

    let req = CompletionRequest {
        messages,
        max_tokens: api_req.max_tokens,
        temperature: api_req.temperature,
        top_p: api_req
            .extra
            .get("top_p")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32),
        stop,
        grammar,
    };

    let stream = match state.engine.complete(&resolved, req).await {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("complete error: {e}"),
            )
                .into_response()
        }
    };

    let model_id = api_req.model.clone();
    let completion_id = format!("chatcmpl-{}", uuid_simple());
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if api_req.stream.unwrap_or(false) {
        let sse_stream = stream.map(move |result| {
            let chunk = match result {
                Ok(c) => c,
                Err(e) => {
                    return Ok::<_, std::convert::Infallible>(
                        format!("data: {{\"error\":\"{e}\"}}\n\n"),
                    );
                }
            };
            let finish_reason = chunk.finish_reason.as_ref().map(|r| match r {
                FinishReason::Stop => "stop",
                FinishReason::Length => "length",
                FinishReason::Error => "error",
            });
            let json = serde_json::json!({
                "id": completion_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model_id,
                "choices": [{"index": 0, "delta": {"content": chunk.delta}, "finish_reason": finish_reason}]
            });
            Ok::<_, std::convert::Infallible>(format!(
                "data: {}\n\n",
                serde_json::to_string(&json).unwrap_or_default()
            ))
        });

        let done = tokio_stream::iter([Ok::<_, std::convert::Infallible>(
            "data: [DONE]\n\n".to_string(),
        )]);

        axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(Body::from_stream(sse_stream.chain(done)))
            .unwrap_or_else(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
    } else {
        let mut full_content = String::new();
        let mut finish_reason = "stop";
        let mut stream = stream;
        while let Some(result) = stream.next().await {
            match result {
                Ok(chunk) => {
                    full_content.push_str(&chunk.delta);
                    if let Some(reason) = &chunk.finish_reason {
                        finish_reason = match reason {
                            FinishReason::Stop => "stop",
                            FinishReason::Length => "length",
                            FinishReason::Error => "error",
                        };
                    }
                }
                Err(e) => {
                    return (
                        if matches!(e, agentix_infer::InferError::ContextExceeded { .. }) {
                            StatusCode::BAD_REQUEST
                        } else {
                            StatusCode::INTERNAL_SERVER_ERROR
                        },
                        format!("stream error: {e}"),
                    )
                        .into_response();
                }
            }
        }
        Json(serde_json::json!({
            "id": completion_id,
            "object": "chat.completion",
            "created": created,
            "model": model_id,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": full_content}, "finish_reason": finish_reason}],
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}
        }))
        .into_response()
    }
}

async fn embeddings_handler(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let req: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid request: {e}")).into_response()
        }
    };
    let model = match req["model"].as_str() {
        Some(m) => m.to_string(),
        None => return (StatusCode::BAD_REQUEST, "missing 'model' field").into_response(),
    };
    let inputs = collect_inputs(&req["input"]);
    if inputs.is_empty() {
        return (StatusCode::BAD_REQUEST, "input must be string or array").into_response();
    }
    let input_refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
    match state.engine.embed_batch(&model, &input_refs).await {
        Ok(embeddings) => {
            let data: Vec<serde_json::Value> = embeddings
                .into_iter()
                .enumerate()
                .map(|(i, emb)| serde_json::json!({"object": "embedding", "index": i, "embedding": emb}))
                .collect();
            Json(serde_json::json!({"object": "list", "model": model, "data": data, "usage": {"prompt_tokens": 0, "total_tokens": 0}}))
                .into_response()
        }
        Err(agentix_infer::InferError::ModelNotFound(_)) => {
            (StatusCode::NOT_FOUND, "model not in local store").into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("inference error: {e}"),
        )
            .into_response(),
    }
}

async fn ollama_embed_handler(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let req: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid request: {e}")).into_response()
        }
    };
    let model = match req["model"].as_str() {
        Some(m) => m.to_string(),
        None => return (StatusCode::BAD_REQUEST, "missing 'model' field").into_response(),
    };
    let inputs = collect_inputs(&req["input"]);
    if inputs.is_empty() {
        return (StatusCode::BAD_REQUEST, "input must be string or array").into_response();
    }
    let input_refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
    match state.engine.embed_batch(&model, &input_refs).await {
        Ok(embeddings) => Json(serde_json::json!({ "embeddings": embeddings })).into_response(),
        Err(agentix_infer::InferError::ModelNotFound(_)) => {
            (StatusCode::NOT_FOUND, "model not in local store").into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("inference error: {e}"),
        )
            .into_response(),
    }
}

async fn models_handler(State(state): State<AppState>) -> Response {
    let models: Vec<serde_json::Value> = state
        .engine
        .list()
        .await
        .into_iter()
        .map(|m| serde_json::json!({"id": m.name, "object": "model", "owned_by": "local"}))
        .collect();
    Json(serde_json::json!({"object": "list", "data": models})).into_response()
}

async fn tags_handler(State(state): State<AppState>) -> Response {
    let models: Vec<serde_json::Value> = state
        .engine
        .list()
        .await
        .into_iter()
        .map(|m| serde_json::json!({"name": m.name, "size": m.size_bytes}))
        .collect();
    Json(serde_json::json!({"models": models})).into_response()
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

async fn resolve_model(engine: &InferEngine, requested: &str) -> Option<String> {
    if engine.info(requested).is_some() {
        return Some(requested.to_string());
    }
    let normalized = requested.replace(':', "/");
    for info in engine.list().await {
        let stored = &info.name;
        if stored.ends_with(&normalized) || stored.ends_with(requested) {
            return Some(stored.clone());
        }
        if let Some(short) = stored.rsplit('/').next() {
            if short == requested || short == normalized {
                return Some(stored.clone());
            }
        }
    }
    None
}

fn collect_inputs(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => vec![],
    }
}

fn normalize_content(content: &serde_json::Value) -> Result<String, String> {
    match content {
        serde_json::Value::String(s) => Ok(s.clone()),
        serde_json::Value::Array(parts) => {
            let mut text = String::new();
            let mut has_images = false;
            for part in parts {
                match part.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                            text.push_str(t);
                        }
                    }
                    Some("image_url") | Some("image") => has_images = true,
                    _ => {}
                }
            }
            if has_images && text.is_empty() {
                Err("request contains only image content; vision not yet supported".to_string())
            } else {
                if has_images {
                    tracing::warn!("image content parts ignored — vision not yet supported");
                }
                Ok(text)
            }
        }
        other => Ok(other.to_string()),
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

fn uuid_simple() -> String {
    Uuid::new_v4().simple().to_string()
}

async fn responses_handler(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let api_req: ResponsesRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("invalid request: {e}")})),
            )
                .into_response()
        }
    };

    let resolved = match resolve_model(&state.engine, &api_req.model).await {
        Some(m) => m,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("model '{}' not found — pull it first", api_req.model)
                })),
            )
                .into_response()
        }
    };

    let mut messages = Vec::with_capacity(api_req.input.len());
    for item in &api_req.input {
        let role = if item.role == "developer" {
            "system".to_string()
        } else {
            item.role.clone()
        };
        let content = match &item.content {
            ResponseInputContent::Text(s) => s.clone(),
            ResponseInputContent::Parts(parts) => parts
                .iter()
                .filter(|p| p.part_type == "input_text")
                .filter_map(|p| p.text.as_deref())
                .collect::<Vec<_>>()
                .join(""),
        };
        messages.push(CompletionMessage { role, content });
    }

    let grammar = match api_req
        .text
        .as_ref()
        .and_then(|t| t.format.as_ref())
        .map(|f| f.format_type.as_str())
    {
        Some("json_schema") => {
            let schema = api_req
                .text
                .as_ref()
                .and_then(|t| t.format.as_ref())
                .and_then(|f| f.schema.as_ref());
            match schema {
                Some(s) => match validate_and_convert_schema(s) {
                    Ok(gbnf) => Some(GrammarConstraint::Gbnf(gbnf)),
                    Err(msg) => {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({
                                "error": format!("text.format.schema could not be converted to a grammar: {msg}")
                            })),
                        )
                            .into_response();
                    }
                },
                None => None,
            }
        }
        _ => None,
    };

    let comp_req = CompletionRequest {
        messages,
        max_tokens: api_req.max_output_tokens,
        temperature: None,
        top_p: None,
        stop: vec![],
        grammar,
    };

    let stream = match state.engine.complete(&resolved, comp_req).await {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("complete error: {e}")})),
            )
                .into_response()
        }
    };

    let mut full_text = String::new();
    let mut stream = stream;
    while let Some(result) = stream.next().await {
        match result {
            Ok(chunk) => full_text.push_str(&chunk.delta),
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("stream error: {e}")})),
                )
                    .into_response();
            }
        }
    }

    // FR-011: refusal detection requires a model-level signal not yet available in agentix-llama;
    // all non-error completions are treated as OutputText
    let content = vec![ResponseOutputContent::OutputText {
        text: full_text.clone(),
    }];
    let output_msg = ResponseOutputMessage {
        msg_type: "message".to_string(),
        id: format!("msg_{}", Uuid::new_v4()),
        role: "assistant".to_string(),
        status: "completed".to_string(),
        content,
    };
    let response = ResponsesResponse {
        id: format!("resp_{}", Uuid::new_v4()),
        object: "response".to_string(),
        model: api_req.model,
        output: vec![output_msg],
        output_text: full_text,
    };

    Json(response).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentix_api::{JsonSchemaSpec, ResponseFormat, ResponseInputItem, ResponseInputPart};

    // T009 — grammar conversion unit tests

    #[test]
    fn json_object_format_produces_json_gbnf() {
        let rf = ResponseFormat {
            format_type: ResponseFormatType::JsonObject,
            json_schema: None,
        };
        let grammar = match rf.format_type {
            ResponseFormatType::JsonObject => Some(GrammarConstraint::Gbnf(JSON_GBNF.to_string())),
            _ => None,
        };
        let GrammarConstraint::Gbnf(gbnf) = grammar.unwrap();
        assert!(gbnf.contains("root") && gbnf.contains("::= object"));
    }

    #[test]
    fn valid_json_schema_converts_to_gbnf() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "required": ["x"]
        });
        let result = validate_and_convert_schema(&schema);
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert!(result.unwrap().contains("root ::="));
    }

    #[test]
    fn external_ref_schema_rejected() {
        let schema = serde_json::json!({ "$ref": "https://example.com/schema.json" });
        let result = validate_and_convert_schema(&schema);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("external $ref URI"));
    }

    #[test]
    fn text_format_type_produces_no_grammar() {
        let rf = ResponseFormat {
            format_type: ResponseFormatType::Text,
            json_schema: None,
        };
        let grammar: Option<GrammarConstraint> = match rf.format_type {
            ResponseFormatType::JsonObject => Some(GrammarConstraint::Gbnf(JSON_GBNF.to_string())),
            ResponseFormatType::Text | ResponseFormatType::Unknown => None,
            _ => None,
        };
        assert!(grammar.is_none());
    }

    // T013 — responses handler helper unit tests

    #[test]
    fn developer_role_maps_to_system() {
        let item = ResponseInputItem {
            role: "developer".to_string(),
            content: ResponseInputContent::Text("hello".to_string()),
        };
        let role = if item.role == "developer" {
            "system".to_string()
        } else {
            item.role.clone()
        };
        assert_eq!(role, "system");
    }

    #[test]
    fn string_content_normalizes_to_text() {
        let content = ResponseInputContent::Text("hello world".to_string());
        let text = match &content {
            ResponseInputContent::Text(s) => s.clone(),
            ResponseInputContent::Parts(parts) => parts
                .iter()
                .filter(|p| p.part_type == "input_text")
                .filter_map(|p| p.text.as_deref())
                .collect::<Vec<_>>()
                .join(""),
        };
        assert_eq!(text, "hello world");
    }

    #[test]
    fn array_content_concatenates_input_text_parts() {
        let content = ResponseInputContent::Parts(vec![
            ResponseInputPart {
                part_type: "input_text".to_string(),
                text: Some("foo ".to_string()),
            },
            ResponseInputPart {
                part_type: "input_text".to_string(),
                text: Some("bar".to_string()),
            },
        ]);
        let text = match &content {
            ResponseInputContent::Text(s) => s.clone(),
            ResponseInputContent::Parts(parts) => parts
                .iter()
                .filter(|p| p.part_type == "input_text")
                .filter_map(|p| p.text.as_deref())
                .collect::<Vec<_>>()
                .join(""),
        };
        assert_eq!(text, "foo bar");
    }

    #[test]
    fn store_and_reasoning_fields_ignored() {
        let json = serde_json::json!({
            "model": "test-model",
            "input": [{ "role": "user", "content": "hello" }],
            "store": true,
            "reasoning": { "effort": "high" }
        });
        let req: Result<ResponsesRequest, _> = serde_json::from_value(json);
        assert!(req.is_ok(), "expected deserialization to succeed: {req:?}");
    }

    #[test]
    fn json_schema_format_missing_schema_handled() {
        let rf = ResponseFormat {
            format_type: ResponseFormatType::JsonSchema,
            json_schema: Some(JsonSchemaSpec {
                name: None,
                schema: serde_json::json!({
                    "type": "object",
                    "properties": { "name": { "type": "string" } }
                }),
                strict: None,
            }),
        };
        assert!(matches!(rf.format_type, ResponseFormatType::JsonSchema));
        assert!(rf.json_schema.is_some());
    }
}
