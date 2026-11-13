use crate::{
    backend::{CompletionStream, LoadedModel},
    pool::Reservation,
    CompletionRequest, InferError,
};
use std::sync::Arc;

/// Lives with native allocations, including their background thread shutdown.
pub struct LoadReservation {
    pub local: Option<Reservation>,
    pub shared: Option<agentix_gpu::Lease>,
    pub peak_bytes: u64,
    pub plan: Option<agentix_gpu::ModelPlan>,
}
impl LoadReservation {
    pub fn unmanaged() -> Self {
        Self {
            local: None,
            shared: None,
            peak_bytes: 0,
            plan: None,
        }
    }
    pub fn activate(&self) -> Result<(), InferError> {
        if let Some(shared) = &self.shared {
            shared
                .activate_plan()
                .map_err(|e| InferError::GpuCapacity(e.to_string()))?;
        }
        Ok(())
    }
    pub fn wrap(self, loaded: Arc<dyn LoadedModel>) -> Arc<dyn LoadedModel> {
        Arc::new(LeasedModel {
            loaded,
            _reservation: self,
        })
    }
}
struct LeasedModel {
    // Field drop order matters: synchronous models are destroyed before their
    // reservation. Threaded backends move it into their native worker instead.
    loaded: Arc<dyn LoadedModel>,
    _reservation: LoadReservation,
}
#[async_trait::async_trait]
impl LoadedModel for LeasedModel {
    async fn embed(&self, input: &str) -> Result<Vec<f32>, InferError> {
        self.loaded.embed(input).await
    }
    async fn embed_batch(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, InferError> {
        self.loaded.embed_batch(inputs).await
    }
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionStream, InferError> {
        self.loaded.complete(request).await
    }
    async fn tokenize(&self, text: &str) -> Result<Vec<i32>, InferError> {
        self.loaded.tokenize(text).await
    }
    async fn transcribe(&self, pcm: &[f32]) -> Result<String, InferError> {
        self.loaded.transcribe(pcm).await
    }
    fn vram_bytes(&self) -> u64 {
        self._reservation.peak_bytes
    }
}
