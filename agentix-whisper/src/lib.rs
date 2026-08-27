//! Whisper backend for in-process speech-to-text transcription.
//!
//! `WhisperContext` is `Send + Sync` (upstream `unsafe impl` in whisper-rs).
//! `WhisperState` is `!Send`, so it is always created and dropped inside `spawn_blocking`.
//! `FullParams` has lifetime parameters, so it is also created inside `spawn_blocking`.

pub mod audio;

pub use audio::decode_audio_to_pcm;

use agentix_infer::{
    backend::{CompletionStream, InferBackend, LoadedModel},
    Capability, CompletionRequest, InferError, ModelFormat, ModelInfo,
};
use std::{path::Path, sync::Arc};
use tokio::sync::oneshot;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

// ── Backend ──────────────────────────────────────────────────────────────────

pub struct WhisperBackend;

#[async_trait::async_trait]
impl InferBackend for WhisperBackend {
    fn name(&self) -> &'static str {
        "whisper"
    }

    fn supports_format(&self, format: ModelFormat) -> bool {
        matches!(format, ModelFormat::Gguf | ModelFormat::WhisperBin)
    }

    async fn load(
        &self,
        blob_path: &Path,
        info: &ModelInfo,
    ) -> Result<Arc<dyn LoadedModel>, InferError> {
        let path = blob_path.to_owned();
        let size_bytes = info.size_bytes;

        tokio::task::spawn_blocking(move || {
            let path_str = path
                .to_str()
                .ok_or_else(|| InferError::Backend("non-UTF-8 model path".to_string()))?;

            tracing::info!(path = %path_str, size_bytes, "loading whisper model");

            let ctx =
                WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
                    .map_err(|e| {
                        InferError::Backend(format!("whisper context init failed: {e:?}"))
                    })?;

            let worker = WhisperWorker::new(ctx)?;

            Ok(Arc::new(WhisperLoadedModel { worker, size_bytes }) as Arc<dyn LoadedModel>)
        })
        .await
        .map_err(|e| InferError::Backend(format!("spawn_blocking join error: {e}")))?
    }
}

// ── Worker thread ────────────────────────────────────────────────────────────

struct WorkItem {
    pcm: Vec<f32>,
    reply: oneshot::Sender<Result<String, InferError>>,
}

/// Owns a `WhisperState` on a dedicated OS thread so state is created once and
/// reused across requests, avoiding the per-request GPU allocation overhead.
struct WhisperWorker {
    tx: std::sync::mpsc::SyncSender<WorkItem>,
}

impl WhisperWorker {
    fn new(ctx: WhisperContext) -> Result<Self, InferError> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<WorkItem>(4);

        let mut state = ctx
            .create_state()
            .map_err(|e| InferError::Backend(format!("whisper state init failed: {e:?}")))?;

        std::thread::Builder::new()
            .name("whisper-worker".into())
            .spawn(move || {
                // ctx is moved here so it outlives state on this thread.
                let _ctx = ctx;
                while let Ok(work) = rx.recv() {
                    let result = run_transcription(&mut state, &work.pcm);
                    let _ = work.reply.send(result);
                }
            })
            .map_err(|e| InferError::Backend(format!("whisper thread spawn failed: {e}")))?;

        Ok(Self { tx })
    }

    async fn transcribe(&self, pcm: Vec<f32>) -> Result<String, InferError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(WorkItem { pcm, reply: reply_tx })
            .map_err(|_| InferError::Backend("whisper worker channel closed".into()))?;
        reply_rx
            .await
            .map_err(|_| InferError::Backend("whisper worker dropped".into()))?
    }
}

fn run_transcription(
    state: &mut whisper_rs::WhisperState,
    pcm: &[f32],
) -> Result<String, InferError> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_print_special(false);
    params.set_single_segment(false);

    state
        .full(params, pcm)
        .map_err(|e| InferError::Transcription(format!("whisper full() failed: {e:?}")))?;

    let n = state
        .full_n_segments()
        .map_err(|e| InferError::Transcription(format!("full_n_segments failed: {e:?}")))?;

    let mut text = String::new();
    for i in 0..n {
        let seg = state
            .full_get_segment_text(i)
            .map_err(|e| InferError::Transcription(format!("segment {i} text failed: {e:?}")))?;
        text.push_str(&seg);
    }

    Ok(text.trim().to_string())
}

// ── Loaded model ─────────────────────────────────────────────────────────────

pub struct WhisperLoadedModel {
    worker: WhisperWorker,
    size_bytes: u64,
}

unsafe impl Send for WhisperLoadedModel {}
unsafe impl Sync for WhisperLoadedModel {}

#[async_trait::async_trait]
impl LoadedModel for WhisperLoadedModel {
    fn vram_bytes(&self) -> u64 {
        self.size_bytes
    }

    async fn embed(&self, _input: &str) -> Result<Vec<f32>, InferError> {
        Err(InferError::CapabilityMissing(
            "whisper".to_string(),
            Capability::Embedding,
        ))
    }

    async fn embed_batch(&self, _inputs: &[&str]) -> Result<Vec<Vec<f32>>, InferError> {
        Err(InferError::CapabilityMissing(
            "whisper".to_string(),
            Capability::Embedding,
        ))
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionStream, InferError> {
        Err(InferError::CapabilityMissing(
            "whisper".to_string(),
            Capability::Completion,
        ))
    }

    async fn tokenize(&self, _text: &str) -> Result<Vec<i32>, InferError> {
        Err(InferError::CapabilityMissing(
            "whisper".to_string(),
            Capability::Completion,
        ))
    }

    async fn transcribe(&self, audio_pcm: &[f32]) -> Result<String, InferError> {
        self.worker.transcribe(audio_pcm.to_vec()).await
    }
}
