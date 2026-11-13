use crate::{CompletionChunk, CompletionRequest, InferError, ModelFormat, ModelInfo};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

pub type CompletionStream =
    Pin<Box<dyn futures::stream::Stream<Item = Result<CompletionChunk, InferError>> + Send>>;

/// A backend capable of loading model blobs into memory.
#[async_trait::async_trait]
pub trait InferBackend: Send + Sync {
    /// Load a model blob into memory. Returns a handle to the loaded model.
    async fn load(
        &self,
        blob_path: &Path,
        info: &ModelInfo,
    ) -> Result<Arc<dyn LoadedModel>, InferError>;

    /// Threaded backends retain this until native context/weights are destroyed.
    async fn load_reserved(
        &self,
        blob_path: &Path,
        info: &ModelInfo,
        reservation: crate::memory::LoadReservation,
    ) -> Result<Arc<dyn LoadedModel>, InferError> {
        if self.uses_gpu() && reservation.shared.is_some() {
            return Err(InferError::GpuCapacity(
                "backend must implement native GPU lease ownership".into(),
            ));
        }
        Ok(reservation.wrap(self.load(blob_path, info).await?))
    }
    fn uses_gpu(&self) -> bool {
        true
    }

    /// Return true if this backend can handle the given model format.
    fn supports_format(&self, format: ModelFormat) -> bool;

    /// Human-readable backend name for logging and error messages.
    fn name(&self) -> &'static str;
}

/// A warm, in-memory model instance held by the ContextPool.
#[async_trait::async_trait]
pub trait LoadedModel: Send + Sync {
    /// Embed a single string.
    async fn embed(&self, input: &str) -> Result<Vec<f32>, InferError>;

    /// Embed a batch of strings.
    async fn embed_batch(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, InferError>;

    /// Stream a completion. Returns `InferError::CapabilityMissing` for embedding-only models.
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionStream, InferError>;

    /// Tokenize text without inference.
    async fn tokenize(&self, text: &str) -> Result<Vec<i32>, InferError>;

    /// Bytes of GPU/CPU memory held by this instance (used by pool for eviction).
    fn vram_bytes(&self) -> u64;

    /// Transcribe 16 kHz mono f32 PCM audio to text.
    /// Returns `InferError::CapabilityMissing` for non-transcription models.
    async fn transcribe(&self, _pcm: &[f32]) -> Result<String, InferError> {
        Err(InferError::CapabilityMissing(
            "model".to_string(),
            crate::Capability::Transcription,
        ))
    }
}
