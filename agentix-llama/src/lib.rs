use agentix_infer::{
    backend::{CompletionStream, InferBackend, LoadedModel},
    Capability, CompletionChunk, CompletionMessage, CompletionRequest, FinishReason, InferError,
    ModelFormat, ModelInfo,
};
use std::{num::NonZeroU32, path::Path, sync::Arc};

use llama_cpp_2::{
    context::params::{LlamaContextParams, LlamaPoolingType},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaModel},
    sampling::LlamaSampler,
};

// True encoder-only architectures. These support llama_encode() and use
// CLS/last-token pooling. Everything else (qwen2, llama, mistral, …) is a
// decoder model and must use llama_decode() with all-token output for
// mean-pooling based embeddings.
const ENCODER_ARCHS: &[&str] = &["bert", "nomic_bert", "roberta", "xlm_roberta"];

// Message type for the inference thread
enum InferMessage {
    EmbedBatch {
        inputs: Vec<String>,
        reply: tokio::sync::oneshot::Sender<Result<Vec<Vec<f32>>, InferError>>,
    },
    Complete {
        req: CompletionRequest,
        tx: tokio::sync::mpsc::UnboundedSender<Result<CompletionChunk, InferError>>,
    },
    Tokenize {
        text: String,
        reply: tokio::sync::oneshot::Sender<Result<Vec<i32>, InferError>>,
    },
}

pub struct LlamaCppLoadedModel {
    tx: std::sync::mpsc::SyncSender<InferMessage>,
    vram_est: u64,
    is_embedding: bool,
    #[allow(dead_code)] // stored for future tokenize/context-info APIs
    n_ctx: u32,
}

pub struct LlamaCppBackend {
    backend: Arc<LlamaBackend>,
    /// Number of model layers to offload to GPU. `u32::MAX` = all layers.
    /// Reads `AGENTIX_GPU_LAYERS` env var; defaults to `u32::MAX` when the
    /// `cuda` feature is enabled, 0 (CPU-only) otherwise.
    n_gpu_layers: u32,
    /// Maximum context window size for completion models.
    /// Reads `AGENTIX_MAX_CTX` env var; defaults to 32768.
    max_ctx: u32,
}

impl LlamaCppBackend {
    pub fn new() -> Result<Self, InferError> {
        let mut backend = LlamaBackend::init()
            .map_err(|e| InferError::Backend(format!("llama.cpp init failed: {:?}", e)))?;

        // Redirect noisy llama.cpp C-library logs through Rust's tracing
        // so they honour the configured log level instead of flooding stderr.
        backend.void_logs();

        let n_gpu_layers = std::env::var("AGENTIX_GPU_LAYERS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(if cfg!(feature = "cuda") { u32::MAX } else { 0 });

        let max_ctx = std::env::var("AGENTIX_MAX_CTX")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(32768);

        tracing::info!(
            n_gpu_layers,
            max_ctx,
            cuda = cfg!(feature = "cuda"),
            "LlamaCppBackend initialised"
        );
        Ok(Self {
            backend: Arc::new(backend),
            n_gpu_layers,
            max_ctx,
        })
    }
}

#[async_trait::async_trait]
impl InferBackend for LlamaCppBackend {
    fn name(&self) -> &'static str {
        "llamacpp"
    }

    fn supports_format(&self, format: ModelFormat) -> bool {
        format == ModelFormat::Gguf
    }

    async fn load(
        &self,
        blob_path: &Path,
        info: &ModelInfo,
    ) -> Result<Arc<dyn LoadedModel>, InferError> {
        let path = blob_path.to_path_buf();
        let backend = Arc::clone(&self.backend);

        // When the manifest doesn't explicitly include Embedding, re-read GGUF
        // to confirm — this catches stale manifests written before the name
        // heuristic was added (e.g. jina models with no pooling_type key).
        let (is_embedding, gguf_meta) = if !info.capabilities.contains(&Capability::Embedding) {
            match agentix_infer::meta::gguf::read_gguf_metadata(&path) {
                Ok(m) => {
                    let emb = m.capabilities.contains(&Capability::Embedding);
                    (emb, Some(m))
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), err = %e, "GGUF metadata read failed; assuming completion-only");
                    (false, None)
                }
            }
        } else {
            (true, None)
        };

        // Prefer GGUF-derived architecture (fresh read) over manifest (may be stale).
        let architecture = gguf_meta
            .as_ref()
            .map(|m| m.architecture.clone())
            .unwrap_or_else(|| info.architecture.clone());

        // Encoder-only models use llama_encode() + last-token output.
        // Decoder models used as embedding models use llama_decode() + all-token output.
        let use_encoder_path = ENCODER_ARCHS.contains(&architecture.as_str());

        tracing::info!(
            model = %info.name,
            architecture = %architecture,
            is_embedding,
            use_encoder_path,
            n_gpu_layers = self.n_gpu_layers,
            "loading model",
        );

        // Completion models get a larger context window; embedding models cap at 4096.
        let max_ctx = if is_embedding { 4096u32 } else { self.max_ctx };
        let n_ctx_val = info.context_length.clamp(64, max_ctx).max(256);
        let size_bytes = info.size_bytes;

        let n_gpu_layers = self.n_gpu_layers;

        // Phase 1: load the model weights (blocking; model is Send)
        let model = tokio::task::spawn_blocking({
            let backend = Arc::clone(&backend);
            move || {
                let params = LlamaModelParams::default().with_n_gpu_layers(n_gpu_layers);
                LlamaModel::load_from_file(&backend, &path, &params)
                    .map_err(|e| InferError::Backend(format!("model load failed: {e:?}")))
            }
        })
        .await
        .map_err(|e| InferError::Backend(e.to_string()))??;

        // Phase 2: spawn a dedicated thread that owns model + context
        let (tx, rx) = std::sync::mpsc::sync_channel::<InferMessage>(16);

        std::thread::Builder::new()
            .name(format!(
                "llama-{}",
                blob_path
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            ))
            .spawn(move || {
                // n_ctx_val >= 256 due to clamping; NonZeroU32::MIN (1) is an unreachable fallback
                let n_ctx = NonZeroU32::new(n_ctx_val).unwrap_or(NonZeroU32::MIN);
                // Decoder-based embedding models need explicit mean pooling — the GGUF
                // has no pooling_type key so llama.cpp defaults to Unspecified (no
                // per-sequence pool), which makes embeddings_seq_ith return null.
                let pooling_type = if is_embedding && !use_encoder_path {
                    LlamaPoolingType::Mean
                } else {
                    LlamaPoolingType::Unspecified
                };
                let n_threads = std::thread::available_parallelism()
                    .map(|n| n.get() as i32)
                    .unwrap_or(4);
                let ctx_params = LlamaContextParams::default()
                    .with_n_ctx(Some(n_ctx))
                    .with_embeddings(is_embedding)
                    .with_pooling_type(pooling_type)
                    // Use all available CPU threads — prefill is memory-bandwidth limited
                    // and llama.cpp defaults to 1 thread without this.
                    .with_n_threads(n_threads)
                    .with_n_threads_batch(n_threads)
                    // Process full context in one pass (no sub-batch splitting).
                    .with_n_batch(n_ctx_val);

                let mut ctx = match model.new_context(&backend, ctx_params) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("context creation failed: {:?}", e);
                        return;
                    }
                };

                for msg in rx {
                    match msg {
                        InferMessage::EmbedBatch { inputs, reply } => {
                            let result =
                                embed_batch_sync(&model, &mut ctx, &inputs, use_encoder_path);
                            let _ = reply.send(result);
                        }
                        InferMessage::Complete { req, tx } => {
                            complete_sync(&model, &mut ctx, &req, &tx);
                        }
                        InferMessage::Tokenize { text, reply } => {
                            let result = model
                                .str_to_token(&text, AddBos::Never)
                                .map(|tokens| tokens.into_iter().map(|t| t.0).collect())
                                .map_err(|e| InferError::Backend(format!("tokenize error: {e:?}")));
                            let _ = reply.send(result);
                        }
                    }
                }
                tracing::debug!("inference thread exiting");
            })
            .map_err(|e| InferError::Backend(format!("failed to spawn inference thread: {e}")))?;

        Ok(Arc::new(LlamaCppLoadedModel {
            tx,
            vram_est: size_bytes,
            is_embedding,
            n_ctx: n_ctx_val,
        }))
    }
}

fn embed_batch_sync(
    model: &LlamaModel,
    ctx: &mut llama_cpp_2::context::LlamaContext<'_>,
    inputs: &[String],
    use_encoder_path: bool,
) -> Result<Vec<Vec<f32>>, InferError> {
    let mut results = Vec::with_capacity(inputs.len());

    for input in inputs {
        let tokens = model
            .str_to_token(input, AddBos::Never)
            .map_err(|e| InferError::Backend(format!("tokenize error: {e:?}")))?;

        if tokens.is_empty() {
            results.push(vec![]);
            continue;
        }

        // Clear KV cache before each sequence so prior decode passes don't
        // bleed through. seq_id is always 0 — one sequence per decode call.
        ctx.clear_kv_cache();

        let n = tokens.len();
        let mut batch = LlamaBatch::new(n, 1);

        for (pos, &token) in tokens.iter().enumerate() {
            // Encoder models: only mark last token as output (encoder pooling handles it).
            // Decoder models: mark ALL tokens as output so llama.cpp can mean-pool them.
            let logit_output = !use_encoder_path || pos == n - 1;
            batch
                .add(token, pos as i32, &[0], logit_output)
                .map_err(|e| InferError::Backend(format!("batch add error: {e:?}")))?;
        }

        if use_encoder_path {
            ctx.encode(&mut batch)
                .map_err(|e| InferError::Backend(format!("encode error: {e:?}")))?;
        } else {
            ctx.decode(&mut batch)
                .map_err(|e| InferError::Backend(format!("decode error: {e:?}")))?;
        }

        let emb = ctx
            .embeddings_seq_ith(0)
            .map_err(|e| InferError::Backend(format!("embeddings error: {e:?}")))?;

        results.push(emb.to_vec());
    }

    Ok(results)
}

pub struct ToolCallResult {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

pub fn parse_tool_calls(output: &str) -> Result<Vec<ToolCallResult>, String> {
    let mut results = Vec::new();
    let mut remaining = output;

    while let Some(start) = remaining.find("<tool_call>") {
        let after_open = &remaining[start + "<tool_call>".len()..];
        let end = match after_open.find("</tool_call>") {
            Some(i) => i,
            None => break,
        };
        let body = &after_open[..end];

        let v: serde_json::Value = serde_json::from_str(body.trim())
            .map_err(|e| format!("invalid tool_call JSON: {e}"))?;

        let name = v["name"]
            .as_str()
            .ok_or_else(|| "tool_call missing 'name' field".to_string())?
            .to_string();

        let arguments = match v.get("arguments") {
            None => "{}".to_string(),
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(other) => {
                serde_json::to_string(other).map_err(|e| format!("arguments serialize: {e}"))?
            }
        };

        let id = format!("call_{}", uuid::Uuid::new_v4());
        results.push(ToolCallResult {
            id,
            name,
            arguments,
        });

        remaining = &after_open[end + "</tool_call>".len()..];
    }

    Ok(results)
}

fn complete_sync(
    model: &LlamaModel,
    ctx: &mut llama_cpp_2::context::LlamaContext<'_>,
    req: &CompletionRequest,
    tx: &tokio::sync::mpsc::UnboundedSender<Result<CompletionChunk, InferError>>,
) {
    // Apply the model's built-in chat template to convert structured messages to a prompt.
    let prompt = match apply_chat_template(model, &req.messages, req.tools.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(Err(InferError::Backend(format!(
                "chat template error: {e}"
            ))));
            return;
        }
    };

    tracing::debug!(prompt_len = prompt.len(), "complete_sync: applying prompt");

    let tokens = match model.str_to_token(&prompt, AddBos::Always) {
        Ok(t) => t,
        Err(e) => {
            let _ = tx.send(Err(InferError::Backend(format!("tokenize error: {e:?}"))));
            return;
        }
    };

    if tokens.is_empty() {
        let _ = tx.send(Ok(CompletionChunk::new(
            String::new(),
            Some(FinishReason::Stop),
        )));
        return;
    }

    let n_ctx = ctx.n_ctx();
    let max_new = req.max_tokens.unwrap_or(1024);
    if tokens.len() as u32 + max_new > n_ctx {
        let _ = tx.send(Err(InferError::ContextExceeded {
            prompt_tokens: tokens.len() as u32,
            max_new_tokens: max_new,
            context_window: n_ctx,
        }));
        return;
    }

    ctx.clear_kv_cache();

    // Prefill: add all prompt tokens in one batch, only last needs logits.
    let n_prompt = tokens.len();
    let mut batch = LlamaBatch::new(n_prompt, 1);
    for (i, &tok) in tokens.iter().enumerate() {
        let is_last = i == n_prompt - 1;
        if let Err(e) = batch.add(tok, i as i32, &[0], is_last) {
            let _ = tx.send(Err(InferError::Backend(format!("batch add error: {e:?}"))));
            return;
        }
    }

    if let Err(e) = ctx.decode(&mut batch) {
        let _ = tx.send(Err(InferError::Backend(format!(
            "prefill decode error: {e:?}"
        ))));
        return;
    }

    // Build sampler chain: grammar (optional, must be first) → top-k → top-p → temperature → dist
    let temperature = req.temperature.unwrap_or(0.8);
    let top_p = req.top_p.unwrap_or(0.95);
    let mut samplers: Vec<LlamaSampler> = Vec::new();
    if let Some(agentix_infer::GrammarConstraint::Gbnf(gbnf)) = &req.grammar {
        match LlamaSampler::grammar(model, gbnf, "root") {
            Ok(s) => samplers.push(s),
            Err(e) => {
                let _ = tx.send(Err(InferError::Backend(format!("grammar init: {e:?}"))));
                return;
            }
        }
    }
    samplers.push(LlamaSampler::top_k(40));
    samplers.push(LlamaSampler::top_p(top_p, 1));
    samplers.push(LlamaSampler::temp(temperature));
    samplers.push(LlamaSampler::dist(0xDEAD_BEEF));
    let mut sampler = LlamaSampler::chain_simple(samplers);

    let mut n_pos = n_prompt;

    for i in 0..max_new {
        // sample() internally calls llama_sampler_accept; no separate accept call needed.
        let new_token = sampler.sample(ctx, -1);

        if model.is_eog_token(new_token) {
            let _ = tx.send(Ok(CompletionChunk::new(
                String::new(),
                Some(FinishReason::Stop),
            )));
            return;
        }

        // token_to_piece_bytes(token, max_size, special=false, lstrip=None)
        let piece = match model.token_to_piece_bytes(new_token, 32, false, None) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) => {
                let _ = tx.send(Err(InferError::Backend(format!("detokenize error: {e:?}"))));
                return;
            }
        };

        // Check stop strings
        if req.stop.iter().any(|s| piece.contains(s.as_str())) {
            let _ = tx.send(Ok(CompletionChunk::new(
                String::new(),
                Some(FinishReason::Stop),
            )));
            return;
        }

        if tx.send(Ok(CompletionChunk::new(piece, None))).is_err() {
            // Client disconnected
            return;
        }

        if i == max_new - 1 {
            let _ = tx.send(Ok(CompletionChunk::new(
                String::new(),
                Some(FinishReason::Length),
            )));
            return;
        }

        // Decode the generated token to extend the KV cache.
        let mut next_batch = LlamaBatch::new(1, 1);
        if let Err(e) = next_batch.add(new_token, n_pos as i32, &[0], true) {
            let _ = tx.send(Err(InferError::Backend(format!("next batch add: {e:?}"))));
            return;
        }
        if let Err(e) = ctx.decode(&mut next_batch) {
            let _ = tx.send(Err(InferError::Backend(format!("next decode: {e:?}"))));
            return;
        }
        n_pos += 1;
    }
}

fn apply_chat_template(
    model: &LlamaModel,
    messages: &[CompletionMessage],
    tools: Option<&[serde_json::Value]>,
) -> Result<String, String> {
    use llama_cpp_2::model::LlamaChatMessage;

    let tools_present = tools.map(|t| !t.is_empty()).unwrap_or(false);

    if !tools_present {
        let tmpl = model
            .chat_template(None)
            .map_err(|e| format!("chat_template: {e:?}"))?;

        let chat: Vec<LlamaChatMessage> = messages
            .iter()
            .map(|m| LlamaChatMessage::new(m.role.clone(), m.content.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("LlamaChatMessage: {e:?}"))?;

        return model
            .apply_chat_template(&tmpl, &chat, true)
            .map_err(|e| format!("apply_chat_template: {e:?}"));
    }

    // Tools path: render the raw Jinja2 template from the GGUF via minijinja.
    let tmpl_obj = model
        .chat_template(None)
        .map_err(|e| format!("chat_template: {e:?}"))?;
    let template_str = tmpl_obj
        .to_str()
        .map_err(|e| format!("template_to_str: {e:?}"))?
        .to_string();

    // Build messages context. For tool-call assistant turns the content must be
    // JSON null (falsy) so Qwen2.5's template detects tool-call-only turns.
    // normalize_content() maps Value::Null to the string "null"; we reverse that here.
    let messages_json: Vec<serde_json::Value> = messages
        .iter()
        .map(|msg| {
            let content = if msg.content == "null" {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(msg.content.clone())
            };
            let mut obj = serde_json::json!({
                "role": msg.role,
                "content": content,
            });
            if let Some(tc) = &msg.tool_calls {
                obj["tool_calls"] = tc.clone();
            }
            if let Some(id) = &msg.tool_call_id {
                obj["tool_call_id"] = serde_json::Value::String(id.clone());
            }
            obj
        })
        .collect();

    let ctx = serde_json::json!({
        "messages": messages_json,
        "tools": tools,
        "add_generation_prompt": true,
    });

    let env = minijinja::Environment::new();
    env.render_str(&template_str, ctx)
        .map_err(|e| format!("minijinja render error: {e}"))
}

#[async_trait::async_trait]
impl LoadedModel for LlamaCppLoadedModel {
    async fn embed(&self, input: &str) -> Result<Vec<f32>, InferError> {
        let mut results = self.embed_batch(&[input]).await?;
        results
            .pop()
            .ok_or_else(|| InferError::Backend("empty embedding result".to_string()))
    }

    async fn embed_batch(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, InferError> {
        if !self.is_embedding {
            return Err(InferError::CapabilityMissing(
                String::new(),
                agentix_infer::Capability::Embedding,
            ));
        }
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(InferMessage::EmbedBatch {
                inputs: inputs.iter().map(|s| s.to_string()).collect(),
                reply: reply_tx,
            })
            .map_err(|_| InferError::Backend("inference thread closed".to_string()))?;
        reply_rx
            .await
            .map_err(|_| InferError::Backend("reply channel dropped".to_string()))?
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionStream, InferError> {
        if self.is_embedding {
            return Err(InferError::CapabilityMissing(
                String::new(),
                agentix_infer::Capability::Completion,
            ));
        }

        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::unbounded_channel();
        self.tx
            .send(InferMessage::Complete { req, tx: chunk_tx })
            .map_err(|_| InferError::Backend("inference thread closed".to_string()))?;

        let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(chunk_rx);
        Ok(Box::pin(stream))
    }

    async fn tokenize(&self, text: &str) -> Result<Vec<i32>, InferError> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(InferMessage::Tokenize {
                text: text.to_string(),
                reply: reply_tx,
            })
            .map_err(|_| InferError::Backend("inference thread closed".to_string()))?;
        reply_rx
            .await
            .map_err(|_| InferError::Backend("reply channel dropped".to_string()))?
    }

    fn vram_bytes(&self) -> u64 {
        self.vram_est
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_tool_call() {
        let output = r#"<tool_call>{"name":"todo_list","arguments":{}}</tool_call>"#;
        let calls = parse_tool_calls(output).expect("parse ok");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "todo_list");
        assert_eq!(calls[0].arguments, "{}");
        assert!(calls[0].id.starts_with("call_"));
    }

    #[test]
    fn multiple_tool_calls() {
        let output = concat!(
            r#"<tool_call>{"name":"foo","arguments":{"x":1}}</tool_call>"#,
            r#"<tool_call>{"name":"bar","arguments":{}}</tool_call>"#
        );
        let calls = parse_tool_calls(output).expect("parse ok");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "foo");
        assert_eq!(calls[1].name, "bar");
    }

    #[test]
    fn no_markers() {
        let calls = parse_tool_calls("Just a plain text response").expect("parse ok");
        assert!(calls.is_empty());
    }

    #[test]
    fn malformed_json_body() {
        let output = "<tool_call>not valid json</tool_call>";
        let result = parse_tool_calls(output);
        assert!(result.is_err());
    }

    #[test]
    fn arguments_as_object_serialized_to_string() {
        let output = r#"<tool_call>{"name":"f","arguments":{"key":"val"}}</tool_call>"#;
        let calls = parse_tool_calls(output).expect("parse ok");
        assert_eq!(calls[0].arguments, r#"{"key":"val"}"#);
    }

    #[test]
    fn arguments_absent_defaults_to_empty_object() {
        let output = r#"<tool_call>{"name":"f"}</tool_call>"#;
        let calls = parse_tool_calls(output).expect("parse ok");
        assert_eq!(calls[0].arguments, "{}");
    }

    #[test]
    fn arguments_as_string_preserved() {
        let output = r#"<tool_call>{"name":"f","arguments":"{\"key\":\"val\"}"}</tool_call>"#;
        let calls = parse_tool_calls(output).expect("parse ok");
        assert_eq!(calls[0].arguments, r#"{"key":"val"}"#);
    }
}
