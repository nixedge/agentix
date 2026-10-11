use crate::{
    backend::{CompletionStream, InferBackend},
    pool::ContextPool,
    store::ModelStore,
    CompletionRequest, InferConfig, InferError, ModelInfo,
};
use futures::StreamExt;
use std::sync::{Arc, RwLock};

struct EngineInner {
    config: InferConfig,
    pool: ContextPool,
    backends: RwLock<Vec<Arc<dyn InferBackend>>>,
}

/// Top-level inference handle. Clone is cheap (Arc internals).
#[derive(Clone)]
pub struct InferEngine {
    inner: Arc<EngineInner>,
}

impl InferEngine {
    pub async fn new(config: InferConfig) -> Result<Self, InferError> {
        std::fs::create_dir_all(&config.models_dir)?;
        let pool = ContextPool::new(config.vram_limit_bytes, config.max_loaded_models);
        Ok(Self {
            inner: Arc::new(EngineInner {
                pool,
                config,
                backends: RwLock::new(vec![]),
            }),
        })
    }

    pub fn register_backend(&self, backend: Arc<dyn InferBackend>) {
        self.inner
            .backends
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .push(backend);
    }

    // ── Model management ─────────────────────────────────────────────────────

    pub async fn pull(&self, model_ref: &str) -> Result<ModelInfo, InferError> {
        let store = self.store();
        let model_ref = model_ref.to_string();
        tokio::task::spawn_blocking(move || store.pull(&model_ref))
            .await
            .map_err(|e| InferError::Backend(e.to_string()))?
    }

    pub async fn list(&self) -> Vec<ModelInfo> {
        let store = self.store();
        tokio::task::spawn_blocking(move || store.list())
            .await
            .unwrap_or_default()
    }

    pub async fn remove(&self, name: &str) -> Result<(), InferError> {
        let store = self.store();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || store.remove(&name))
            .await
            .map_err(|e| InferError::Backend(e.to_string()))?
    }

    pub fn info(&self, name: &str) -> Option<ModelInfo> {
        self.store().info(name)
    }

    pub async fn model_digest(&self, name: &str) -> Option<String> {
        let store = self.store();
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || {
            let path = store.resolve(&name)?;
            path.file_name()?
                .to_str()?
                .strip_prefix("sha256-")
                .map(str::to_owned)
        })
        .await
        .ok()
        .flatten()
    }

    /// Returns the names of all registered backends (e.g. `["llamacpp"]`).
    pub fn backend_names(&self) -> Vec<&'static str> {
        self.inner
            .backends
            .read()
            .map(|b| b.iter().map(|b| b.name()).collect())
            .unwrap_or_default()
    }

    // ── Inference ────────────────────────────────────────────────────────────

    pub async fn embed(&self, model: &str, input: &str) -> Result<Vec<f32>, InferError> {
        self.check_capability(model, crate::Capability::Embedding)?;
        let guard = self.acquire(model).await?;
        guard.embed(input).await
    }

    pub async fn embed_batch(
        &self,
        model: &str,
        inputs: &[&str],
    ) -> Result<Vec<Vec<f32>>, InferError> {
        self.check_capability(model, crate::Capability::Embedding)?;
        let guard = self.acquire(model).await?;
        guard.embed_batch(inputs).await
    }

    pub async fn complete(
        &self,
        model: &str,
        req: CompletionRequest,
    ) -> Result<CompletionStream, InferError> {
        self.check_capability(model, crate::Capability::Completion)?;
        let guard = self.acquire(model).await?;
        let stream = guard.complete(req).await?;
        Ok(Box::pin(futures::stream::unfold(
            (stream, guard),
            |(mut stream, guard)| async move { stream.next().await.map(|item| (item, (stream, guard))) },
        )))
    }

    pub async fn tokenize(&self, model: &str, text: &str) -> Result<Vec<i32>, InferError> {
        let guard = self.acquire(model).await?;
        guard.tokenize(text).await
    }

    pub async fn complete_tools(
        &self,
        model: &str,
        request: crate::exchange::ToolCompletionRequest,
        cancel: crate::exchange::Cancellation,
    ) -> Result<crate::backend::ToolCompletionStream, InferError> {
        request.validate()?;
        if cancel.is_cancelled() {
            return Err(InferError::Cancelled);
        }
        self.check_capability(model, crate::Capability::Completion)?;
        let guard = self.acquire(model).await?;
        if cancel.is_cancelled() {
            return Err(InferError::Cancelled);
        }
        let stream = guard.complete_tools(request, cancel.clone()).await?;
        let cancel_on_drop = CancelOnDrop(cancel);
        Ok(Box::pin(futures::stream::unfold(
            (stream, guard, cancel_on_drop),
            |(mut stream, guard, cancel)| async move {
                stream
                    .next()
                    .await
                    .map(|item| (item, (stream, guard, cancel)))
            },
        )))
    }

    /// Transcribe 16 kHz mono f32 PCM audio to text using the specified model.
    pub async fn transcribe_pcm(&self, model: &str, pcm: &[f32]) -> Result<String, InferError> {
        self.check_capability(model, crate::Capability::Transcription)?;
        let guard = self.acquire(model).await?;
        guard.transcribe(pcm).await
    }

    /// Pre-load a model into the pool so the first inference request is fast.
    /// The guard is dropped immediately, returning the model to the idle pool warm.
    pub async fn warmup(&self, model: &str) -> Result<(), InferError> {
        let _guard = self.acquire(model).await?;
        Ok(())
    }

    pub fn residency(&self, model: &str) -> crate::pool::Residency {
        self.inner.pool.residency(model)
    }

    pub fn capacity(&self) -> crate::pool::PoolCapacity {
        self.inner.pool.capacity()
    }

    pub fn model_details(&self, model: &str) -> Option<(bool, u32)> {
        self.inner.pool.model_details(model)
    }

    pub async fn coding_supported(&self, model: &str) -> Result<bool, InferError> {
        Ok(self.acquire(model).await?.coding_supported())
    }

    // ── Internal ─────────────────────────────────────────────────────────────

    fn store(&self) -> ModelStore {
        ModelStore::new(self.inner.config.models_dir.clone())
    }

    /// Return `CapabilityMissing` if the model's manifest does not include `cap`.
    /// Returns `Ok(())` if the model is unknown (load will fail later with ModelNotFound).
    fn check_capability(&self, model: &str, cap: crate::Capability) -> Result<(), InferError> {
        if let Some(info) = self.store().info(model) {
            if !info.capabilities.contains(&cap) {
                return Err(InferError::CapabilityMissing(model.to_string(), cap));
            }
        }
        Ok(())
    }

    async fn acquire(&self, model: &str) -> Result<crate::pool::ModelGuard, InferError> {
        let _loading = self.inner.pool.lock_loading(model).await;
        if let Some(guard) = self.inner.pool.acquire_idle(model) {
            return Ok(guard);
        }
        if self.inner.pool.residency(model) == crate::pool::Residency::Busy {
            return Err(InferError::ModelBusy(model.to_string()));
        }
        self.load_model(model).await
    }

    async fn load_model(&self, model: &str) -> Result<crate::pool::ModelGuard, InferError> {
        let store = self.store();
        let info = store
            .info(model)
            .ok_or_else(|| InferError::ModelNotFound(model.to_string()))?;

        let blob_path = store
            .resolve(model)
            .ok_or_else(|| InferError::ModelNotFound(model.to_string()))?;

        let backend = {
            let backends = self
                .inner
                .backends
                .read()
                .map_err(|_| InferError::Backend("backends lock poisoned".to_string()))?;
            // Select by BackendHint name first — both LlamaCpp and Whisper accept GGUF,
            // so format alone is ambiguous.
            let hint_name = match info.backend {
                crate::BackendHint::LlamaCpp => "llamacpp",
                crate::BackendHint::Whisper => "whisper",
                crate::BackendHint::Candle => "candle",
            };
            backends
                .iter()
                .find(|b| b.name() == hint_name)
                .or_else(|| backends.iter().find(|b| b.supports_format(info.format)))
                .cloned()
                .ok_or(InferError::NoBackend(info.format))?
        };

        tracing::info!(model = %model, backend = %backend.name(), "loading model");
        let loaded = backend.load(&blob_path, &info).await?;
        let vram = loaded.vram_bytes();

        self.inner.pool.store_loaded(model, loaded, vram)
    }
}

struct CancelOnDrop(crate::exchange::Cancellation);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
