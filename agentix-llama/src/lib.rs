mod generation;
pub mod tool_template;

use agentix_infer::{
    backend::{CompletionStream, InferBackend, LoadedModel},
    Capability, CompletionChunk, CompletionMessage, CompletionRequest, FinishReason, InferError,
    ModelFormat, ModelInfo,
};
use std::{
    num::NonZeroU32,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use llama_cpp_2::{
    context::params::{LlamaContextParams, LlamaPoolingType},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaModel},
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
        tx: tokio::sync::mpsc::Sender<Result<CompletionChunk, InferError>>,
        cancel: agentix_infer::exchange::Cancellation,
        _busy: BusyGuard,
    },
    Tools {
        request: agentix_infer::exchange::ToolCompletionRequest,
        tx: tokio::sync::mpsc::Sender<
            Result<agentix_infer::exchange::ToolCompletionEvent, InferError>,
        >,
        cancel: agentix_infer::exchange::Cancellation,
        _busy: BusyGuard,
    },
    Tokenize {
        text: String,
        reply: tokio::sync::oneshot::Sender<Result<Vec<i32>, InferError>>,
    },
}

pub struct LlamaCppLoadedModel {
    tx: tokio::sync::mpsc::Sender<InferMessage>,
    busy: Arc<AtomicBool>,
    coding_supported: bool,
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
        let (tx, mut rx) = tokio::sync::mpsc::channel::<InferMessage>(16);
        let busy = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

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
                        let _ = ready_tx.send(Err(InferError::Backend(format!(
                            "context creation failed: {e:?}"
                        ))));
                        return;
                    }
                };

                let template = model.chat_template(None).ok().and_then(|value| {
                    value
                        .to_str()
                        .ok()
                        .and_then(|source| tool_template::ToolTemplate::new(source).ok())
                });
                if ready_tx.send(Ok(template.is_some())).is_err() {
                    return;
                }
                while let Some(msg) = rx.blocking_recv() {
                    match msg {
                        InferMessage::EmbedBatch { inputs, reply } => {
                            let result =
                                embed_batch_sync(&model, &mut ctx, &inputs, use_encoder_path);
                            let _ = reply.send(result);
                        }
                        InferMessage::Complete {
                            req,
                            tx,
                            cancel,
                            _busy,
                        } => {
                            complete_sync(&model, &mut ctx, &req, &tx, &cancel);
                        }
                        InferMessage::Tools {
                            request,
                            tx,
                            cancel,
                            _busy,
                        } => {
                            let result = match template.as_ref() {
                                Some(template) => {
                                    tools_sync(&model, &mut ctx, template, request, &tx, &cancel)
                                }
                                None => Err(InferError::ToolCompletionUnsupported),
                            };
                            if let Err(error) = result {
                                let _ = send_chunk(&tx, Err(error), &cancel);
                            }
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

        let coding_supported = ready_rx
            .await
            .map_err(|_| InferError::Backend("model initialization interrupted".into()))??;

        Ok(Arc::new(LlamaCppLoadedModel {
            tx,
            busy,
            coding_supported,
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

fn complete_sync(
    model: &LlamaModel,
    ctx: &mut llama_cpp_2::context::LlamaContext<'_>,
    req: &CompletionRequest,
    tx: &tokio::sync::mpsc::Sender<Result<CompletionChunk, InferError>>,
    cancel: &agentix_infer::exchange::Cancellation,
) {
    let result = apply_chat_template(model, &req.messages)
        .map_err(InferError::Backend)
        .and_then(|prompt| {
            generation::generate(
                model,
                ctx,
                &prompt,
                generation::Options {
                    max_tokens: req.max_tokens.unwrap_or(1024),
                    temperature: req.temperature,
                    top_p: req.top_p,
                    grammar: req.grammar.as_ref(),
                    stop: &req.stop,
                },
                cancel,
                |text| send_chunk(tx, Ok(CompletionChunk::new(text, None)), cancel),
            )
        });
    let final_chunk = result.map(|reason| CompletionChunk::new(String::new(), Some(reason)));
    let _ = send_chunk(tx, final_chunk, cancel);
}

fn send_chunk<T>(
    tx: &tokio::sync::mpsc::Sender<T>,
    mut item: T,
    cancel: &agentix_infer::exchange::Cancellation,
) -> Result<(), InferError> {
    loop {
        if cancel.is_cancelled() {
            return Err(InferError::Cancelled);
        }
        match tx.try_send(item) {
            Ok(()) => return Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                return Err(InferError::Cancelled)
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(value)) => item = value,
        }
        // Runs only on the dedicated inference thread, never Tokio.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn tools_sync(
    model: &LlamaModel,
    ctx: &mut llama_cpp_2::context::LlamaContext<'_>,
    template: &tool_template::ToolTemplate,
    mut request: agentix_infer::exchange::ToolCompletionRequest,
    tx: &tokio::sync::mpsc::Sender<
        Result<agentix_infer::exchange::ToolCompletionEvent, InferError>,
    >,
    cancel: &agentix_infer::exchange::Cancellation,
) -> Result<(), InferError> {
    use agentix_infer::exchange::{
        Message, ToolCompletionEvent as Event, ToolFinishReason as Reason,
    };
    for attempt in 0..=1 {
        let prompt = template.render(&request)?;
        let mut output = String::new();
        let mut emitted = 0;
        let reason = generation::generate(
            model,
            ctx,
            &prompt,
            generation::Options {
                max_tokens: request.max_tokens,
                temperature: request.temperature,
                top_p: None,
                grammar: None,
                stop: &[],
            },
            cancel,
            |text| {
                output.push_str(&text);
                // Hold every possible marker suffix and all text after a tool marker.
                let available = output.find('<').unwrap_or(output.len());
                if available > emitted {
                    send_chunk(
                        tx,
                        Ok(Event::Text(output[emitted..available].to_string())),
                        cancel,
                    )?;
                    emitted = available;
                }
                Ok(())
            },
        )?;
        if reason != FinishReason::Stop {
            send_chunk(tx, Ok(Event::Finished(Reason::Length)), cancel)?;
            return Ok(());
        }
        match template.parse_output(&output, &request.tools) {
            Ok(parsed) => {
                if parsed.text.len() > emitted {
                    send_chunk(
                        tx,
                        Ok(Event::Text(parsed.text[emitted..].to_string())),
                        cancel,
                    )?;
                }
                let finish = if parsed.calls.is_empty() {
                    Reason::Stop
                } else {
                    send_chunk(tx, Ok(Event::Calls(parsed.calls)), cancel)?;
                    Reason::ToolCalls
                };
                return send_chunk(tx, Ok(Event::Finished(finish)), cancel);
            }
            Err(_) if attempt == 0 && !request.tools.is_empty() => {
                // Exactly one repair, with no tool execution or invented result.
                request.messages.push(Message::Assistant {
                    content: output,
                    tool_calls: vec![],
                });
                request.messages.push(Message::User {
                    content: template.repair_prompt().into(),
                });
            }
            Err(error) => return Err(error),
        }
    }
    Err(InferError::InvalidToolExchange(
        "tool repair budget exhausted",
    ))
}

fn apply_chat_template(
    model: &LlamaModel,
    messages: &[CompletionMessage],
) -> Result<String, String> {
    use llama_cpp_2::model::LlamaChatMessage;

    let tmpl = match model.chat_template(None) {
        Ok(template) => template,
        Err(llama_cpp_2::ChatTemplateError::MissingTemplate)
            if messages.len() == 1 && messages[0].role == "user" =>
        {
            return Ok(messages[0].content.clone())
        }
        Err(error) => return Err(format!("chat_template: {error:?}")),
    };

    let chat: Vec<LlamaChatMessage> = messages
        .iter()
        .map(|m| LlamaChatMessage::new(m.role.clone(), m.content.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("LlamaChatMessage: {e:?}"))?;

    model
        .apply_chat_template(&tmpl, &chat, true)
        .map_err(|e| format!("apply_chat_template: {e:?}"))
}

#[async_trait::async_trait]
impl LoadedModel for LlamaCppLoadedModel {
    fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }
    fn coding_supported(&self) -> bool {
        self.coding_supported
    }
    fn context_limit(&self) -> u32 {
        self.n_ctx
    }

    async fn complete_tools(
        &self,
        request: agentix_infer::exchange::ToolCompletionRequest,
        cancel: agentix_infer::exchange::Cancellation,
    ) -> Result<agentix_infer::backend::ToolCompletionStream, InferError> {
        request.validate()?;
        if !self.coding_supported {
            return Err(InferError::ToolCompletionUnsupported);
        }
        let busy = BusyGuard::acquire(&self.busy)?;
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        self.tx
            .send(InferMessage::Tools {
                request,
                tx,
                cancel: cancel.clone(),
                _busy: busy,
            })
            .await
            .map_err(|_| InferError::Backend("inference thread closed".into()))?;
        Ok(Box::pin(CancellableStream {
            inner: tokio_stream::wrappers::ReceiverStream::new(rx),
            cancel,
        }))
    }

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
            .await
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

        let busy = BusyGuard::acquire(&self.busy)?;
        let cancel = agentix_infer::exchange::Cancellation::default();
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(32);
        self.tx
            .send(InferMessage::Complete {
                req,
                tx: chunk_tx,
                cancel: cancel.clone(),
                _busy: busy,
            })
            .await
            .map_err(|_| InferError::Backend("inference thread closed".to_string()))?;

        Ok(Box::pin(CancellableStream {
            inner: tokio_stream::wrappers::ReceiverStream::new(chunk_rx),
            cancel,
        }))
    }

    async fn tokenize(&self, text: &str) -> Result<Vec<i32>, InferError> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(InferMessage::Tokenize {
                text: text.to_string(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| InferError::Backend("inference thread closed".to_string()))?;
        reply_rx
            .await
            .map_err(|_| InferError::Backend("reply channel dropped".to_string()))?
    }

    fn vram_bytes(&self) -> u64 {
        self.vram_est
    }
}

struct BusyGuard(Arc<AtomicBool>);
impl BusyGuard {
    fn acquire(busy: &Arc<AtomicBool>) -> Result<Self, InferError> {
        busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| InferError::ModelBusy("inference worker".into()))?;
        Ok(Self(Arc::clone(busy)))
    }
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct CancellableStream<T> {
    inner: tokio_stream::wrappers::ReceiverStream<T>,
    cancel: agentix_infer::exchange::Cancellation,
}
impl<T> tokio_stream::Stream for CancellableStream<T> {
    type Item = T;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<T>> {
        let this = self.get_mut();
        std::pin::Pin::new(&mut this.inner).poll_next(cx)
    }
}
impl<T> Drop for CancellableStream<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
