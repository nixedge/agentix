use agentix_api::ChatCompletionRequest;
use agentix_qwen::{
    config::QwenTextConfig,
    tokenizer::{GreedyStopReason, PromptRequest, QwenTokenizer},
    Error as QwenError,
};
use agentix_qwen_sys::{QwenGgufDecoder, QwenGgufTensorStore};
mod tool_calls;

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    convert::Infallible,
    env,
    error::Error,
    fs::File,
    io::Read,
    os::unix::fs::FileTypeExt,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::UnixListener,
    sync::{OwnedSemaphorePermit, Semaphore},
};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tracing::{error, info};

#[derive(Clone)]
struct AppState {
    store: Arc<QwenGgufTensorStore>,
    config: Arc<QwenTextConfig>,
    tokenizer: Arc<QwenTokenizer>,
    model_name: Arc<str>,
    context_limit: usize,
    cuda_output_device: Option<usize>,
    generation_slot: Arc<Semaphore>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agentix_qwen_worker=info".into()),
        )
        .init();

    let model_dir = env_path("AGENTIX_QWEN_MODEL_DIR", "/var/lib/agentix/qwen");
    let config_path = env_path_or_join("AGENTIX_QWEN_CONFIG", &model_dir, "config.json");
    let tokenizer_path = env_path_or_join("AGENTIX_QWEN_TOKENIZER", &model_dir, "tokenizer.json");
    let template_path = env_path_or_join(
        "AGENTIX_QWEN_CHAT_TEMPLATE",
        &model_dir,
        "chat_template.jinja",
    );
    let shard_paths = env::var_os("AGENTIX_QWEN_GGUF_SHARDS")
        .map(|value| env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_else(|| {
            vec![
                model_dir.join("iq3s-00001.gguf"),
                model_dir.join("iq3s-00002.gguf"),
            ]
        });
    if shard_paths.is_empty() {
        return Err("AGENTIX_QWEN_GGUF_SHARDS must list at least one shard".into());
    }
    let expected_hashes = env::var("AGENTIX_QWEN_EXPECTED_SHARD_SHA256")?;
    verify_shard_hashes(&shard_paths, &expected_hashes)?;

    let config_bytes = read_artifact(&config_path, "Qwen config")?;
    let config = QwenTextConfig::parse(&config_bytes)?;
    let expert_cache_bytes = env::var("AGENTIX_QWEN_EXPERT_CACHE_BYTES")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|error| format!("invalid AGENTIX_QWEN_EXPERT_CACHE_BYTES: {error}"))?
        .unwrap_or(256 * 1024 * 1024);
    let store = QwenGgufTensorStore::open_with_expert_cache_bytes(
        &shard_paths,
        &config,
        expert_cache_bytes,
    )
    .map_err(|error| std::io::Error::other(format!("cannot open Qwen GGUF shard set: {error}")))?;
    let tokenizer =
        QwenTokenizer::from_files(&tokenizer_path, &template_path).map_err(|error| {
            std::io::Error::other(format!(
                "cannot load Qwen tokenizer {} and chat template {}: {error}",
                tokenizer_path.display(),
                template_path.display()
            ))
        })?;
    if tokenizer.tokenizer_token_count() != 248_077
        || tokenizer.model_vocabulary_size() != config.vocab_size
    {
        return Err("Qwen tokenizer vocabulary does not match model geometry".into());
    }
    let context_limit = env::var("AGENTIX_QWEN_CONTEXT")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(config.max_position_embeddings)
        .min(config.max_position_embeddings);
    if context_limit == 0 {
        return Err("Qwen context limit must be positive".into());
    }
    let cuda_output_device = env::var("AGENTIX_QWEN_CUDA_OUTPUT_DEVICE")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()?;
    let mut decoder = QwenGgufDecoder::open(&store, &config).map_err(|error| {
        std::io::Error::other(format!("cannot initialize Qwen decoder: {error}"))
    })?;
    configure_decoder(&mut decoder, cuda_output_device).map_err(std::io::Error::other)?;
    drop(decoder);

    let state = AppState {
        store: Arc::new(store),
        config: Arc::new(config),
        tokenizer: Arc::new(tokenizer),
        model_name: Arc::from(
            env::var("AGENTIX_QWEN_MODEL").unwrap_or_else(|_| "qwen3.8-flash-next".into()),
        ),
        context_limit,
        cuda_output_device,
        generation_slot: Arc::new(Semaphore::new(1)),
    };

    let socket = env_path("AGENTIX_QWEN_SOCKET", "/run/agentix/qwen.sock");
    prepare_socket_path(&socket)?;
    let listener = UnixListener::bind(&socket).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!(
                "cannot bind Qwen worker socket {}: {error}",
                socket.display()
            ),
        )
    })?;
    info!(
        socket = %socket.display(),
        model = %state.model_name,
        shards = shard_paths.len(),
        context_limit,
        cuda_output_device = ?state.cuda_output_device,
        "Qwen worker ready"
    );

    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    let _ = std::fs::remove_file(socket);
    Ok(())
}

fn verify_shard_hashes(paths: &[PathBuf], expected: &str) -> Result<(), Box<dyn Error>> {
    let expected_hashes = expected.split(':').collect::<Vec<_>>();
    if expected_hashes.len() != paths.len()
        || expected_hashes
            .iter()
            .any(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err("expected SHA256 list does not match Qwen shard set".into());
    }
    let mut buffer = vec![0_u8; 1024 * 1024];
    for (path, expected_hash) in paths.iter().zip(expected_hashes) {
        let mut file = File::open(path).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("cannot open Qwen GGUF shard {}: {error}", path.display()),
            )
        })?;
        let mut digest = Sha256::new();
        loop {
            let count = file.read(&mut buffer).map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("cannot read Qwen GGUF shard {}: {error}", path.display()),
                )
            })?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        let actual_hash = format!("{:x}", digest.finalize());
        if !actual_hash.eq_ignore_ascii_case(expected_hash) {
            return Err(format!("Qwen GGUF shard hash mismatch: {}", path.display()).into());
        }
    }
    Ok(())
}

fn read_artifact(path: &std::path::Path, kind: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(path).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("cannot read {kind} {}: {error}", path.display()),
        )
    })
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({"status": "ready", "model": state.model_name.as_ref()}))
}

async fn models(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "object": "list",
        "data": [{"id": state.model_name.as_ref(), "object": "model", "owned_by": "agentix"}]
    }))
}

async fn chat_completions(State(state): State<AppState>, body: Bytes) -> Response {
    let request: ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Malformed request",
            )
        }
    };
    if request.model != state.model_name.as_ref() {
        return api_error(
            StatusCode::NOT_FOUND,
            "model_unavailable",
            "Requested model is not loaded",
        );
    }
    let streaming = request.stream.unwrap_or(false);
    if request
        .temperature
        .is_some_and(|temperature| temperature != 0.0)
        || request.response_format.is_some()
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "unsupported_capability",
            "This worker currently supports greedy text completion only",
        );
    }
    if request.messages.iter().any(|message| {
        !(message.content.is_string()
            || message.content.is_null()
                && message.role == "assistant"
                && message.tool_calls.is_some())
    }) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "unsupported_capability",
            "Only text message content is supported",
        );
    }
    let tools = match request.extra.get("tools") {
        None => Vec::new(),
        Some(Value::Array(tools)) => {
            if tool_calls::validate_tool_definitions(tools).is_err() {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "Function tool definitions are invalid or use an unsupported schema",
                );
            }
            tools.clone()
        }
        Some(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "The tools field must be an array of function definitions",
            )
        }
    };
    if streaming && !tools.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "unsupported_capability",
            "Streaming function calls are not available in this worker build",
        );
    }
    let allowed_extra = [
        "tools",
        "reasoning_effort",
        "enable_thinking",
        "max_completion_tokens",
    ];
    if request
        .extra
        .keys()
        .any(|key| !allowed_extra.contains(&key.as_str()))
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "unsupported_capability",
            "Request contains unsupported generation options",
        );
    }

    let max_new_tokens = request
        .max_tokens
        .or_else(|| {
            request
                .extra
                .get("max_completion_tokens")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
        })
        .unwrap_or(256);
    if request.max_tokens.is_some() && request.extra.contains_key("max_completion_tokens") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Specify only one output token limit",
        );
    }
    if request
        .extra
        .get("max_completion_tokens")
        .is_some_and(|value| match value.as_u64() {
            Some(count) => u32::try_from(count).is_err(),
            None => true,
        })
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid output token limit",
        );
    }
    let Ok(max_new_tokens) = usize::try_from(max_new_tokens) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid token limit",
        );
    };
    if let Err(error) = validate_reasoning_options(
        request.extra.get("reasoning_effort"),
        request.extra.get("enable_thinking"),
    ) {
        match error {
            ReasoningOptionError::Invalid => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "Reasoning options have invalid types or values",
                )
            }
            ReasoningOptionError::Unsupported => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "unsupported_capability",
                    "Reasoning output is not available in this worker build",
                )
            }
        }
    }
    let reasoning_effort = "none";

    let messages = request
        .messages
        .iter()
        .map(|message| -> Result<Value, tool_calls::ToolCallError> {
            let mut mapped = json!({"role": message.role, "content": message.content});
            if let Some(tool_calls) = &message.tool_calls {
                mapped["tool_calls"] = tool_calls::normalize_tool_history(tool_calls)?;
            }
            if let Some(tool_call_id) = &message.tool_call_id {
                mapped["tool_call_id"] = json!(tool_call_id);
            }
            Ok(mapped)
        })
        .collect::<Result<Vec<_>, _>>();
    let messages = match messages {
        Ok(messages) => messages,
        Err(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Prior function-call messages are malformed",
            );
        }
    };
    let prepared = match state.tokenizer.prepare_prompt(PromptRequest {
        messages: &messages,
        tools: &tools,
        reasoning_effort,
        enable_thinking: false,
        preserve_thinking: false,
        add_generation_prompt: true,
        max_new_tokens,
        context_limit: state.context_limit,
    }) {
        Ok(prepared) => prepared,
        Err(QwenError::Capacity("context window")) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "context_length_exceeded",
                "Prompt and requested output exceed the supported context",
            )
        }
        Err(QwenError::Invalid("empty prompt, output bound, or context limit")) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Prompt and output token limit must be non-empty and positive",
            )
        }
        Err(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Prompt could not be prepared",
            )
        }
    };
    let permit = match Arc::clone(&state.generation_slot).try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "busy",
                "The Qwen worker is handling another request",
            )
        }
    };

    let model_state = state.clone();
    if streaming {
        return stream_completion(&state, prepared, permit);
    }

    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut decoder = QwenGgufDecoder::open(&model_state.store, &model_state.config)
            .map_err(|_| "Model initialization failed")?;
        configure_decoder(&mut decoder, model_state.cuda_output_device)?;
        let output = decoder
            .generate_prepared_greedy(&prepared, &model_state.tokenizer.stop_token_ids())
            .map_err(|_| "Model generation failed")?;
        let completion_tokens = output.generation.generated_token_ids.len();
        let output_tokens = match output.generation.stop_reason {
            GreedyStopReason::StopToken(stop) => output
                .generation
                .generated_token_ids
                .iter()
                .take_while(|token| **token != stop)
                .copied()
                .collect::<Vec<_>>(),
            GreedyStopReason::TokenLimit => output.generation.generated_token_ids,
        };
        let text = model_state
            .tokenizer
            .decode(&output_tokens)
            .map_err(|_| "Generated text could not be decoded")?;
        let stopped = matches!(
            output.generation.stop_reason,
            GreedyStopReason::StopToken(_)
        );
        Ok::<_, &'static str>((
            text,
            stopped,
            prepared.admission.prompt_tokens,
            completion_tokens,
        ))
    })
    .await;

    match result {
        Ok(Ok((text, stopped, prompt_tokens, completion_tokens))) => {
            let parsed_tool_output = match tool_calls::parse_tool_output(&text, &tools) {
                Ok(parsed) => parsed,
                Err(tool_calls::ToolCallError::InvalidDefinitions) => {
                    return api_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request",
                        "Function tool definitions are invalid",
                    );
                }
                Err(tool_calls::ToolCallError::MalformedOutput) => {
                    return api_error(
                        StatusCode::BAD_GATEWAY,
                        "invalid_tool_call",
                        "The model produced an invalid function call",
                    );
                }
            };
            let (content, response_tool_calls, finish) = match parsed_tool_output {
                Some(parsed) => {
                    let calls = parsed
                        .calls
                        .into_iter()
                        .map(|call| {
                            json!({
                                "id": format!("call_{}", uuid::Uuid::new_v4().simple()),
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": serde_json::to_string(&call.arguments)
                                        .unwrap_or_else(|_| "{}".to_owned())
                                }
                            })
                        })
                        .collect::<Vec<_>>();
                    (parsed.content, Some(calls), "tool_calls")
                }
                None => (Some(text), None, if stopped { "stop" } else { "length" }),
            };
            let total_tokens = prompt_tokens.saturating_add(completion_tokens);
            let mut message = json!({"role": "assistant", "content": content});
            if let Some(tool_calls) = response_tool_calls {
                message["tool_calls"] = json!(tool_calls);
            }
            Json(json!({
                "id": format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
                "object": "chat.completion",
                "created": SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_secs()),
                "model": state.model_name.as_ref(),
                "choices": [{
                    "index": 0,
                    "message": message,
                    "finish_reason": finish
                }],
                "usage": {
                    "prompt_tokens": prompt_tokens,
                    "completion_tokens": completion_tokens,
                    "total_tokens": total_tokens
                }
            }))
            .into_response()
        }
        Ok(Err(message)) => {
            error!(message, "Qwen generation failed");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Qwen generation failed",
            )
        }
        Err(error) => {
            error!(%error, "Qwen generation task failed");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Qwen generation failed",
            )
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReasoningOptionError {
    Invalid,
    Unsupported,
}

fn validate_reasoning_options(
    reasoning_effort: Option<&Value>,
    enable_thinking: Option<&Value>,
) -> Result<(), ReasoningOptionError> {
    if let Some(value) = reasoning_effort {
        match value.as_str() {
            Some("none") => {}
            Some(_) => return Err(ReasoningOptionError::Unsupported),
            None => return Err(ReasoningOptionError::Invalid),
        }
    }
    if let Some(value) = enable_thinking {
        match value.as_bool() {
            Some(false) => {}
            Some(true) => return Err(ReasoningOptionError::Unsupported),
            None => return Err(ReasoningOptionError::Invalid),
        }
    }
    Ok(())
}

fn stream_completion(
    state: &AppState,
    prepared: agentix_qwen::tokenizer::PreparedPrompt,
    permit: OwnedSemaphorePermit,
) -> Response {
    let model_state = state.clone();
    let response_id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs());
    let model = state.model_name.to_string();
    let (sender, receiver) = tokio::sync::mpsc::channel::<Bytes>(8);
    let task_sender = sender.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let result = stream_generation(
            &model_state,
            &prepared,
            &task_sender,
            &response_id,
            created,
            &model,
        );
        if let Err(error) = result {
            error!(%error, "Qwen streaming generation failed");
            let _ = send_sse(
                &task_sender,
                Some("error"),
                &json!({
                    "error": {"message": "Qwen generation failed", "type": "internal", "code": "internal"}
                }),
            );
        }
    });

    let stream = ReceiverStream::new(receiver).map(Ok::<_, Infallible>);
    let mut response = Response::new(axum::body::Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    response
}

fn stream_generation(
    state: &AppState,
    prepared: &agentix_qwen::tokenizer::PreparedPrompt,
    sender: &tokio::sync::mpsc::Sender<Bytes>,
    response_id: &str,
    created: u64,
    model: &str,
) -> Result<(), &'static str> {
    let mut decoder = QwenGgufDecoder::open(&state.store, &state.config)
        .map_err(|_| "Model initialization failed")?;
    configure_decoder(&mut decoder, state.cuda_output_device)?;
    let mut logits = None;
    for token_id in &prepared.token_ids {
        if sender.is_closed() {
            return Ok(());
        }
        logits = Some(
            decoder
                .step(*token_id)
                .map_err(|_| "Prompt prefill failed")?
                .logits,
        );
    }
    let mut logits = logits.ok_or("Prompt contained no tokens")?;
    let mut generated = Vec::new();
    let mut previous_text = String::new();
    if !send_sse(
        sender,
        None,
        &json!({
            "id": response_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]
        }),
    ) {
        return Ok(());
    }
    let stop_ids = state.tokenizer.stop_token_ids();
    let mut terminal_reason = "length";
    for index in 0..prepared.admission.requested_output_tokens {
        if sender.is_closed() {
            return Ok(());
        }
        let token_id = agentix_qwen::tokenizer::greedy_next_token(&logits)
            .map_err(|_| "Greedy sampling failed")?;
        if stop_ids.contains(&token_id) {
            terminal_reason = "stop";
            break;
        }
        generated.push(token_id);
        let text = state
            .tokenizer
            .decode(&generated)
            .map_err(|_| "Generated text decoding failed")?;
        let delta = text
            .strip_prefix(&previous_text)
            .ok_or("Incremental tokenizer output was not prefix-stable")?;
        if !delta.is_empty() {
            let chunk = json!({
                "id": response_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{"index": 0, "delta": {"content": delta}, "finish_reason": null}]
            });
            if !send_sse(sender, None, &chunk) {
                return Ok(());
            }
        }
        previous_text = text;
        if index + 1 < prepared.admission.requested_output_tokens {
            logits = decoder
                .step(token_id)
                .map_err(|_| "Decode step failed")?
                .logits;
        }
    }

    let terminal = json!({
        "id": response_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{"index": 0, "delta": {}, "finish_reason": terminal_reason}]
    });
    if send_sse(sender, None, &terminal) {
        let _ = sender.blocking_send(Bytes::from_static(b"data: [DONE]\n\n"));
    }
    Ok(())
}

fn send_sse(
    sender: &tokio::sync::mpsc::Sender<Bytes>,
    event: Option<&str>,
    payload: &Value,
) -> bool {
    let serialized = match serde_json::to_vec(payload) {
        Ok(serialized) => serialized,
        Err(_) => return false,
    };
    let mut bytes = Vec::with_capacity(serialized.len() + 32);
    if let Some(event) = event {
        bytes.extend_from_slice(b"event: ");
        bytes.extend_from_slice(event.as_bytes());
        bytes.push(b'\n');
    }
    bytes.extend_from_slice(b"data: ");
    bytes.extend_from_slice(&serialized);
    bytes.extend_from_slice(b"\n\n");
    sender.blocking_send(Bytes::from(bytes)).is_ok()
}

fn api_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": code,
                "code": code
            }
        })),
    )
        .into_response()
}

fn env_path(name: &str, default: &str) -> PathBuf {
    env::var_os(name).map_or_else(|| PathBuf::from(default), PathBuf::from)
}

#[cfg(feature = "cuda")]
fn configure_decoder(
    decoder: &mut QwenGgufDecoder<'_>,
    device: Option<usize>,
) -> Result<(), &'static str> {
    if let Some(device) = device {
        decoder
            .enable_cuda_output_projection(device)
            .map_err(|_| "cannot enable the requested CUDA output projection")?;
    }
    Ok(())
}

#[cfg(not(feature = "cuda"))]
fn configure_decoder(
    _decoder: &mut QwenGgufDecoder<'_>,
    device: Option<usize>,
) -> Result<(), &'static str> {
    if device.is_some() {
        Err("CUDA output projection requested from a worker built without CUDA")
    } else {
        Ok(())
    }
}

fn env_path_or_join(name: &str, base: &std::path::Path, filename: &str) -> PathBuf {
    env::var_os(name).map_or_else(|| base.join(filename), PathBuf::from)
}

fn prepare_socket_path(path: &std::path::Path) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!(
                    "cannot create Qwen socket directory {}: {error}",
                    parent.display()
                ),
            )
        })?;
    }
    match std::fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            error
        } else {
            std::io::Error::new(
                error.kind(),
                format!(
                    "cannot inspect Qwen socket path {}: {error}",
                    path.display()
                ),
            )
        }
    }) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            std::fs::remove_file(path).map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!(
                        "cannot remove stale Qwen socket {}: {error}",
                        path.display()
                    ),
                )
            })?
        }
        Ok(_) => return Err("refusing to replace a non-socket Qwen path".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        let mut signal =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(_) => return,
            };
        let _ = signal.recv().await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::{
        prepare_socket_path, send_sse, validate_reasoning_options, verify_shard_hashes,
        ReasoningOptionError,
    };
    use agentix_api::ChatCompletionRequest;
    use serde_json::{json, Value};
    use std::{error::Error, fs, os::unix::net::UnixListener};

    fn temporary_path(suffix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("agentix-qwen-{suffix}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn reasoning_options_reject_invalid_types_and_unsupported_modes() {
        let invalid_effort = json!({"reasoning_effort": 1});
        assert_eq!(
            validate_reasoning_options(invalid_effort.get("reasoning_effort"), None),
            Err(ReasoningOptionError::Invalid)
        );

        let unsupported_effort = json!({"reasoning_effort": "high"});
        assert_eq!(
            validate_reasoning_options(unsupported_effort.get("reasoning_effort"), None),
            Err(ReasoningOptionError::Unsupported)
        );

        let invalid_thinking = json!({"enable_thinking": "false"});
        assert_eq!(
            validate_reasoning_options(None, invalid_thinking.get("enable_thinking")),
            Err(ReasoningOptionError::Invalid)
        );

        let enabled_thinking = json!({"enable_thinking": true});
        assert_eq!(
            validate_reasoning_options(None, enabled_thinking.get("enable_thinking")),
            Err(ReasoningOptionError::Unsupported)
        );

        let supported = json!({"reasoning_effort": "none", "enable_thinking": false});
        assert_eq!(
            validate_reasoning_options(
                supported.get("reasoning_effort"),
                supported.get("enable_thinking")
            ),
            Ok(())
        );
    }

    #[test]
    fn chat_request_preserves_prior_tool_calls_and_tool_response_ids() -> Result<(), Box<dyn Error>>
    {
        let request: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "qwen3.8-flash-next",
            "messages": [
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "run_command",
                            "arguments": "{\"command\":\"pwd\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_123",
                    "content": "/workspace"
                }
            ],
            "tools": []
        }))?;
        assert_eq!(
            request.messages[0]
                .tool_calls
                .as_ref()
                .and_then(Value::as_array)
                .and_then(|calls| calls.first())
                .and_then(|call| call.get("id"))
                .and_then(Value::as_str),
            Some("call_123")
        );
        assert_eq!(
            request.messages[1].tool_call_id.as_deref(),
            Some("call_123")
        );
        assert!(request.extra.get("tools").is_some_and(Value::is_array));
        Ok(())
    }

    #[test]
    fn shard_integrity_requires_matching_ordered_hashes() -> Result<(), Box<dyn Error>> {
        let path = temporary_path("hash.gguf");
        fs::write(&path, b"abc")?;
        let result = verify_shard_hashes(
            std::slice::from_ref(&path),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
        result?;
        assert!(verify_shard_hashes(
            std::slice::from_ref(&path),
            "0000000000000000000000000000000000000000000000000000000000000000"
        )
        .is_err());
        let _ = fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn missing_shard_error_names_the_expected_path() {
        let path = temporary_path("missing.gguf");
        let result = verify_shard_hashes(
            std::slice::from_ref(&path),
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        let error = match result {
            Err(error) => error,
            Ok(()) => panic!("missing shard must fail startup validation"),
        };
        let message = error.to_string();
        assert!(message.contains("cannot open Qwen GGUF shard"));
        assert!(message.contains(path.to_string_lossy().as_ref()));
    }

    #[test]
    fn socket_setup_removes_a_stale_socket() -> Result<(), Box<dyn Error>> {
        let path = temporary_path("stale.sock");
        let listener = UnixListener::bind(&path)?;
        drop(listener);

        prepare_socket_path(&path)?;

        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn socket_setup_preserves_an_existing_regular_file() -> Result<(), Box<dyn Error>> {
        let path = temporary_path("regular-file");
        fs::write(&path, b"keep this file")?;

        assert!(prepare_socket_path(&path).is_err());
        assert_eq!(fs::read(&path)?, b"keep this file");
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn sse_frames_include_event_and_json_data_lines() -> Result<(), Box<dyn Error>> {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        assert!(send_sse(
            &sender,
            Some("error"),
            &json!({"code": "internal"})
        ));
        let frame = receiver.blocking_recv().ok_or("SSE frame missing")?;
        let frame = std::str::from_utf8(&frame)?;
        assert!(frame.starts_with("event: error\ndata: "));
        assert!(frame.ends_with("\n\n"));
        let json_line = frame
            .strip_prefix("event: error\ndata: ")
            .and_then(|frame| frame.strip_suffix("\n\n"))
            .ok_or("malformed SSE frame")?;
        let payload: Value = serde_json::from_str(json_line)?;
        assert_eq!(payload, json!({"code": "internal"}));
        Ok(())
    }

    #[test]
    fn sse_sender_reports_disconnect_without_panicking() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        assert!(!send_sse(&sender, None, &json!({"delta": "text"})));
    }
}
