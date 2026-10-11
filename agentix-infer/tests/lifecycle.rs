use agentix_infer::{
    backend::{CompletionStream, InferBackend, LoadedModel},
    pool::{ContextPool, Residency},
    CompletionRequest, InferConfig, InferEngine, InferError, ModelFormat, ModelInfo,
};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

struct Model(u64);

#[async_trait::async_trait]
impl LoadedModel for Model {
    async fn embed(&self, _: &str) -> Result<Vec<f32>, InferError> {
        Ok(vec![])
    }
    async fn embed_batch(&self, _: &[&str]) -> Result<Vec<Vec<f32>>, InferError> {
        Ok(vec![])
    }
    async fn complete(&self, _: CompletionRequest) -> Result<CompletionStream, InferError> {
        Ok(Box::pin(futures::stream::pending()))
    }
    async fn tokenize(&self, _: &str) -> Result<Vec<i32>, InferError> {
        Ok(vec![])
    }
    fn vram_bytes(&self) -> u64 {
        self.0
    }
}

#[test]
fn active_guards_count_against_capacity_and_cannot_be_evicted() -> Result<(), InferError> {
    let pool = ContextPool::new(Some(100), 2);
    let first = pool.store_loaded("first", Arc::new(Model(60)), 60)?;
    assert_eq!(pool.capacity().active_models, 1);
    assert_eq!(pool.capacity().resident_bytes, 60);
    assert!(matches!(
        pool.store_loaded("second", Arc::new(Model(50)), 50),
        Err(InferError::VramExhausted { .. })
    ));
    assert!(matches!(
        pool.store_loaded("first", Arc::new(Model(1)), 1),
        Err(InferError::ModelBusy(_))
    ));
    assert_eq!(pool.residency("first"), Residency::Busy);
    drop(first);
    let second = pool.store_loaded("second", Arc::new(Model(50)), 50)?;
    assert_eq!(pool.residency("first"), Residency::Cold);
    assert_eq!(pool.residency("second"), Residency::Busy);
    drop(second);
    assert_eq!(pool.capacity().active_models, 0);
    assert_eq!(pool.residency("second"), Residency::Idle);
    Ok(())
}

#[test]
fn rejected_admission_preserves_idle_models_and_zero_capacity_is_enforced() -> Result<(), InferError>
{
    let pool = ContextPool::new(Some(100), 1);
    drop(pool.store_loaded("warm", Arc::new(Model(30)), 30)?);
    assert!(pool
        .store_loaded("oversized", Arc::new(Model(101)), 101)
        .is_err());
    assert_eq!(pool.residency("warm"), Residency::Idle);
    let guard = pool
        .acquire_idle("warm")
        .ok_or(InferError::CapacityExhausted)?;
    assert!(matches!(
        pool.store_loaded("other", Arc::new(Model(1)), 1),
        Err(InferError::CapacityExhausted)
    ));
    drop(guard);
    let disabled = ContextPool::new(None, 0);
    assert!(matches!(
        disabled.store_loaded("any", Arc::new(Model(0)), 0),
        Err(InferError::CapacityExhausted)
    ));
    Ok(())
}

#[tokio::test]
async fn loader_lock_serializes_and_releases_after_cancellation(
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = Arc::new(ContextPool::new(None, 1));
    let held = pool.lock_loading("model").await;
    let other_pool = Arc::clone(&pool);
    let waiter = tokio::spawn(async move { other_pool.lock_loading("model").await });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());
    waiter.abort();
    assert!(waiter.await.is_err());
    drop(held);
    let _next = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        pool.lock_loading("model"),
    )
    .await?;
    Ok(())
}

struct Backend {
    loads: AtomicUsize,
    fail_first: bool,
}

#[async_trait::async_trait]
impl InferBackend for Backend {
    async fn load(&self, _: &Path, _: &ModelInfo) -> Result<Arc<dyn LoadedModel>, InferError> {
        let attempt = self.loads.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        if self.fail_first && attempt == 0 {
            return Err(InferError::Backend("fixture load failure".into()));
        }
        Ok(Arc::new(Model(60)))
    }
    fn supports_format(&self, _: ModelFormat) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        "llamacpp"
    }
}

async fn engine(
    fail_first: bool,
) -> Result<(tempfile::TempDir, InferEngine, String, Arc<Backend>), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let engine = InferEngine::new(InferConfig::new(
        dir.path().to_path_buf(),
        Some(100),
        1,
        512,
    ))
    .await?;
    let model = engine
        .pull(&std::env::var("AGENTIX_TEST_GENERATION_MODEL_PATH")?)
        .await?;
    let backend = Arc::new(Backend {
        loads: AtomicUsize::new(0),
        fail_first,
    });
    engine.register_backend(backend.clone());
    Ok((dir, engine, model.name, backend))
}

#[tokio::test]
async fn concurrent_warmup_loads_once_and_stream_holds_guard_until_drop(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_dir, engine, name, backend) = engine(false).await?;
    let (a, b) = tokio::join!(engine.warmup(&name), engine.warmup(&name));
    a?;
    b?;
    assert_eq!(backend.loads.load(Ordering::SeqCst), 1);
    assert_eq!(engine.residency(&name), Residency::Idle);
    let stream = engine
        .complete(
            &name,
            CompletionRequest::new(vec![], Some(1), None, None, vec![]),
        )
        .await?;
    assert_eq!(engine.residency(&name), Residency::Busy);
    assert!(matches!(
        engine.warmup(&name).await,
        Err(InferError::ModelBusy(_))
    ));
    assert_eq!(backend.loads.load(Ordering::SeqCst), 1);
    drop(stream);
    assert_eq!(engine.residency(&name), Residency::Idle);
    Ok(())
}

#[tokio::test]
async fn failed_load_releases_serialization_for_retry() -> Result<(), Box<dyn std::error::Error>> {
    let (_dir, engine, name, backend) = engine(true).await?;
    assert!(engine.warmup(&name).await.is_err());
    assert_eq!(engine.residency(&name), Residency::Cold);
    tokio::time::timeout(std::time::Duration::from_secs(1), engine.warmup(&name)).await??;
    assert_eq!(backend.loads.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn typed_completion_rejects_cancelled_requests_and_unsupported_backends(
) -> Result<(), Box<dyn std::error::Error>> {
    use agentix_infer::exchange::{Cancellation, Message, ToolCompletionRequest};
    let (_dir, engine, name, backend) = engine(false).await?;
    let request = ToolCompletionRequest {
        messages: vec![Message::User {
            content: "hello".into(),
        }],
        tools: vec![],
        max_tokens: 4,
        temperature: None,
    };
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(matches!(
        engine
            .complete_tools(&name, request.clone(), cancelled)
            .await,
        Err(InferError::Cancelled)
    ));
    assert_eq!(backend.loads.load(Ordering::SeqCst), 0);
    assert!(matches!(
        engine
            .complete_tools(&name, request, Cancellation::default())
            .await,
        Err(InferError::ToolCompletionUnsupported)
    ));
    assert_eq!(engine.residency(&name), Residency::Idle);
    Ok(())
}
