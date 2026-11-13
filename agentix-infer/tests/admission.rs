use agentix_infer::{
    backend::{CompletionStream, InferBackend, LoadedModel},
    CompletionRequest, InferConfig, InferEngine, InferError, ModelFormat, ModelInfo,
};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
struct FakeBackend {
    loads: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
}
struct FakeModel;
#[async_trait::async_trait]
impl InferBackend for FakeBackend {
    fn name(&self) -> &'static str {
        "llamacpp"
    }
    fn supports_format(&self, format: ModelFormat) -> bool {
        format == ModelFormat::Gguf
    }
    async fn load(&self, _: &Path, _: &ModelInfo) -> Result<Arc<dyn LoadedModel>, InferError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(InferError::Backend("deliberate load failure".into()));
        }
        Ok(Arc::new(FakeModel))
    }
}
#[async_trait::async_trait]
impl LoadedModel for FakeModel {
    async fn embed(&self, _: &str) -> Result<Vec<f32>, InferError> {
        Ok(vec![1.0])
    }
    async fn embed_batch(&self, _: &[&str]) -> Result<Vec<Vec<f32>>, InferError> {
        Ok(vec![vec![1.0]])
    }
    async fn complete(&self, _: CompletionRequest) -> Result<CompletionStream, InferError> {
        Ok(Box::pin(futures::stream::pending()))
    }
    async fn tokenize(&self, _: &str) -> Result<Vec<i32>, InferError> {
        Ok(vec![])
    }
    fn vram_bytes(&self) -> u64 {
        1
    }
}
#[tokio::test]
async fn stream_retains_active_allocation_and_admission_precedes_backend_load(
) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = std::env::var("AGENTIX_TEST_MODEL_PATH")?;
    let directory = tempfile::tempdir()?;
    let store = agentix_infer::store::ModelStore::new(directory.path().into());
    let info = store.pull(&fixture)?;
    store.add_alias("second", &info.name)?;
    let loads = Arc::new(AtomicUsize::new(0));
    let engine = InferEngine::new(InferConfig::new(
        directory.path().into(),
        Some(info.size_bytes),
        1,
        256,
    ))
    .await?;
    engine.register_backend(Arc::new(FakeBackend {
        loads: loads.clone(),
        fail: Arc::new(AtomicBool::new(false)),
    }));
    // Tokenize bypasses capability classification of the embedding-only fixture.
    engine.tokenize(&info.name, "hello").await?;
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    engine.warmup("second").await?;
    assert_eq!(loads.load(Ordering::SeqCst), 2);
    // Acquire a completion stream through an alias with completion capability
    // explicitly added to the fixture's manifest, without changing its weights.
    let manifest_path = directory
        .path()
        .join("manifests/agentix")
        .join(&info.name)
        .join("latest");
    let mut manifest = agentix_infer::store::manifest::read_manifest(&manifest_path)?;
    manifest
        ._agentix
        .as_mut()
        .ok_or("fixture has no agentix metadata")?
        .capabilities
        .push(agentix_infer::Capability::Completion);
    agentix_infer::store::manifest::write_manifest(&manifest_path, &manifest)?;
    let stream = engine
        .complete(
            "second",
            CompletionRequest::new(vec![], Some(1), Some(0.0), None, vec![]),
        )
        .await?;
    assert!(matches!(
        engine.warmup(&info.name).await,
        Err(InferError::VramExhausted { .. })
    ));
    assert_eq!(
        loads.load(Ordering::SeqCst),
        2,
        "native load must not run before admission"
    );
    drop(stream);
    engine.warmup(&info.name).await?;
    assert_eq!(loads.load(Ordering::SeqCst), 3);
    Ok(())
}
#[tokio::test]
async fn failed_load_releases_pending_budget_and_loading_lock(
) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = std::env::var("AGENTIX_TEST_MODEL_PATH")?;
    let directory = tempfile::tempdir()?;
    let store = agentix_infer::store::ModelStore::new(directory.path().into());
    let info = store.pull(&fixture)?;
    let loads = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicBool::new(true));
    let engine = InferEngine::new(InferConfig::new(
        directory.path().into(),
        Some(info.size_bytes),
        1,
        256,
    ))
    .await?;
    engine.register_backend(Arc::new(FakeBackend {
        loads: loads.clone(),
        fail: fail.clone(),
    }));
    assert!(engine.warmup(&info.name).await.is_err());
    fail.store(false, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(1), engine.warmup(&info.name)).await??;
    assert_eq!(loads.load(Ordering::SeqCst), 2);
    Ok(())
}
