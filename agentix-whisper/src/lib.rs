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

            Ok(Arc::new(WhisperLoadedModel {
                ctx: Arc::new(ctx),
                size_bytes,
            }) as Arc<dyn LoadedModel>)
        })
        .await
        .map_err(|e| InferError::Backend(format!("spawn_blocking join error: {e}")))?
    }
}

// ── Loaded model ─────────────────────────────────────────────────────────────

pub struct WhisperLoadedModel {
    ctx: Arc<WhisperContext>,
    size_bytes: u64,
}

// SAFETY: WhisperContext implements Send + Sync via `unsafe impl` in whisper-rs,
// justified by the upstream assertion that whisper_context is safe to share across
// threads as long as only one thread creates inference state at a time.
// We always create WhisperState inside spawn_blocking and drop it before returning.
unsafe impl Send for WhisperLoadedModel {}
// SAFETY: Same rationale — WhisperContext is safe to alias across threads.
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
        let ctx = Arc::clone(&self.ctx);
        let pcm = audio_pcm.to_vec();

        tokio::task::spawn_blocking(move || {
            // WhisperState is !Send and must stay inside this closure.
            let mut state = ctx
                .create_state()
                .map_err(|e| InferError::Transcription(format!("state creation failed: {e:?}")))?;

            // FullParams has lifetime parameters — create it here inside spawn_blocking.
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_print_special(false);
            params.set_single_segment(false);

            state
                .full(params, &pcm)
                .map_err(|e| InferError::Transcription(format!("whisper full() failed: {e:?}")))?;

            let n = state
                .full_n_segments()
                .map_err(|e| InferError::Transcription(format!("full_n_segments failed: {e:?}")))?;

            let mut text = String::new();
            for i in 0..n {
                let seg = state.full_get_segment_text(i).map_err(|e| {
                    InferError::Transcription(format!("segment {i} text failed: {e:?}"))
                })?;
                text.push_str(&seg);
            }

            Ok(text.trim().to_string())
        })
        .await
        .map_err(|e| InferError::Transcription(format!("spawn_blocking join error: {e}")))?
    }
}
