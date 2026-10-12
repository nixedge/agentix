use agentix_infer::model_operation::{
    OperationError, OperationLimits, OperationStore, WarmFailure, WarmLoader, WarmOperations,
    WarmPhase, WarmRequest,
};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn request(key: &str) -> WarmRequest {
    WarmRequest {
        idempotency_key: key.into(),
        model_id: "org/model:Q8_0".into(),
    }
}

struct ControlledLoader {
    calls: std::sync::atomic::AtomicUsize,
    started: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    failure: Option<WarmFailure>,
}

impl ControlledLoader {
    fn new(failure: Option<WarmFailure>) -> Self {
        Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
            started: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
            failure,
        }
    }
}

#[async_trait::async_trait]
impl WarmLoader for ControlledLoader {
    async fn warm(&self, _: &str) -> std::result::Result<(), WarmFailure> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.started.add_permits(1);
        let permit = self
            .release
            .acquire()
            .await
            .map_err(|_| WarmFailure::BackendUnavailable)?;
        permit.forget();
        match self.failure {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }
}

async fn terminal(
    operations: &WarmOperations,
    id: uuid::Uuid,
) -> Result<agentix_infer::model_operation::WarmOperation> {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let operation = operations.get(id).await?;
            if operation.phase.is_terminal() {
                return Ok::<_, OperationError>(operation);
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await?
    .map_err(Into::into)
}

#[tokio::test]
async fn concurrent_retries_run_once_and_survive_submitter_disconnect() -> Result {
    let temp = tempfile::tempdir()?;
    let loader = std::sync::Arc::new(ControlledLoader::new(None));
    let operations = WarmOperations::open(
        temp.path().join("operations"),
        OperationLimits::default(),
        loader.clone(),
    )
    .await?;
    let observer = operations.clone();
    let submissions =
        futures::future::join_all((0..12).map(|_| operations.submit(request("same")))).await;
    let mut ids = std::collections::HashSet::new();
    for operation in submissions {
        ids.insert(operation?.id);
    }
    assert_eq!(ids.len(), 1);
    let id = ids.into_iter().next().ok_or("missing operation")?;
    tokio::time::timeout(std::time::Duration::from_secs(3), loader.started.acquire())
        .await??
        .forget();
    drop(operations);
    assert_eq!(observer.get(id).await?.phase, WarmPhase::Warming);
    assert_eq!(loader.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    loader.release.add_permits(1);
    assert_eq!(terminal(&observer, id).await?.phase, WarmPhase::Ready);
    Ok(())
}

#[tokio::test]
async fn cancelled_load_holds_capacity_until_the_backend_stops() -> Result {
    let temp = tempfile::tempdir()?;
    let loader = std::sync::Arc::new(ControlledLoader::new(None));
    let operations = WarmOperations::open(
        temp.path().join("operations"),
        OperationLimits {
            active: 1,
            retained: 10,
        },
        loader.clone(),
    )
    .await?;
    let queued = operations.submit(request("one")).await?;
    tokio::time::timeout(std::time::Duration::from_secs(3), loader.started.acquire())
        .await??
        .forget();
    let cancelling = operations.cancel(queued.id).await?;
    assert_eq!(cancelling.phase, WarmPhase::Warming);
    assert!(cancelling.cancel_requested);
    assert!(matches!(
        operations.submit(request("two")).await,
        Err(OperationError::Capacity)
    ));
    loader.release.add_permits(1);
    assert_eq!(
        terminal(&operations, queued.id).await?.phase,
        WarmPhase::Cancelled
    );
    let next = operations.submit(request("two")).await?;
    loader.release.add_permits(1);
    assert_eq!(
        terminal(&operations, next.id).await?.phase,
        WarmPhase::Ready
    );
    Ok(())
}

#[tokio::test]
async fn failed_load_is_durable_and_same_key_does_not_retry_it() -> Result {
    let temp = tempfile::tempdir()?;
    let loader = std::sync::Arc::new(ControlledLoader::new(Some(WarmFailure::LoadFailed)));
    loader.release.add_permits(1);
    let operations = WarmOperations::open(
        temp.path().join("operations"),
        OperationLimits::default(),
        loader.clone(),
    )
    .await?;
    let queued = operations.submit(request("one")).await?;
    let failed = terminal(&operations, queued.id).await?;
    assert_eq!(failed.phase, WarmPhase::Failed);
    assert_eq!(failed.failure, Some(WarmFailure::LoadFailed));
    assert_eq!(operations.submit(request("one")).await?, failed);
    assert_eq!(loader.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn retries_are_durable_and_conflicting_keys_cannot_start_work() -> Result {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let dir = temp.path().join("operations");
    let mut store = OperationStore::open(&dir, OperationLimits::default())?;
    let (queued, created) = store.submit(request("one"))?;
    assert!(created);
    assert_eq!(store.submit(request("one"))?, (queued.clone(), false));
    let mut conflict = request("one");
    conflict.model_id = "other".into();
    assert!(matches!(
        store.submit(conflict),
        Err(OperationError::IdempotencyConflict)
    ));
    assert!(matches!(
        store.finish(queued.id, queued.sequence, Ok(())),
        Err(OperationError::Conflict)
    ));
    let warming = store.start(queued.id, queued.sequence)?;
    assert!(matches!(
        store.start(queued.id, queued.sequence),
        Err(OperationError::Conflict)
    ));
    let ready = store.finish(warming.id, warming.sequence, Ok(()))?;
    assert_eq!(ready.phase, WarmPhase::Ready);
    assert_eq!(ready.sequence, 2);
    assert_eq!(store.cancel(ready.id)?, ready);
    drop(store);
    let mut reopened = OperationStore::open(&dir, OperationLimits::default())?;
    assert_eq!(reopened.submit(request("one"))?, (ready, false));
    assert_eq!(fs::metadata(&dir)?.permissions().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(dir.join("operations.json"))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    Ok(())
}

#[test]
fn cancellation_waits_for_loader_and_cannot_be_overwritten_by_success() -> Result {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let mut store = OperationStore::open(temp.path(), OperationLimits::default())?;
    let (queued, _) = store.submit(request("one"))?;
    let warming = store.start(queued.id, queued.sequence)?;
    let cancelling = store.cancel(warming.id)?;
    assert_eq!(cancelling.phase, WarmPhase::Warming);
    assert!(cancelling.cancel_requested);
    assert_eq!(store.cancel(warming.id)?, cancelling);
    let cancelled = store.finish(warming.id, warming.sequence, Ok(()))?;
    assert_eq!(cancelled.phase, WarmPhase::Cancelled);
    assert!(matches!(
        store.finish(warming.id, warming.sequence, Ok(())),
        Err(OperationError::Conflict)
    ));
    let (queued, _) = store.submit(request("two"))?;
    assert_eq!(store.cancel(queued.id)?.phase, WarmPhase::Cancelled);
    assert!(matches!(
        store.start(queued.id, queued.sequence),
        Err(OperationError::Conflict)
    ));
    Ok(())
}

#[test]
fn restart_interrupts_pending_work_and_rejects_late_loader_results() -> Result {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let mut store = OperationStore::open(temp.path(), OperationLimits::default())?;
    let (queued, _) = store.submit(request("queued"))?;
    let (loading, _) = store.submit(request("loading"))?;
    let warming = store.start(loading.id, loading.sequence)?;
    drop(store);
    let mut store = OperationStore::open(temp.path(), OperationLimits::default())?;
    for old in [queued, warming] {
        let current = store.get(old.id)?;
        assert_eq!(current.phase, WarmPhase::Interrupted);
        assert_eq!(current.sequence, old.sequence + 1);
        assert!(matches!(
            store.finish(old.id, old.sequence, Ok(())),
            Err(OperationError::Conflict)
        ));
        assert_eq!(store.submit(old.request)?, (current, false));
    }
    assert!(store.submit(request("explicit-retry"))?.1);
    Ok(())
}

#[test]
fn capacity_bounds_allow_idempotent_retries_but_not_unbounded_retention() -> Result {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let mut store = OperationStore::open(
        temp.path(),
        OperationLimits {
            active: 1,
            retained: 2,
        },
    )?;
    let (first, _) = store.submit(request("one"))?;
    assert!(!store.submit(request("one"))?.1);
    assert!(matches!(
        store.submit(request("two")),
        Err(OperationError::Capacity)
    ));
    let warming = store.start(first.id, first.sequence)?;
    let failed = store.finish(warming.id, warming.sequence, Err(WarmFailure::LoadFailed))?;
    assert_eq!(failed.failure, Some(WarmFailure::LoadFailed));
    let (second, _) = store.submit(request("two"))?;
    store.cancel(second.id)?;
    assert!(matches!(
        store.submit(request("three")),
        Err(OperationError::Capacity)
    ));
    Ok(())
}

#[test]
fn exclusive_writer_is_released_when_store_drops() -> Result {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let store = OperationStore::open(temp.path(), OperationLimits::default())?;
    assert!(matches!(
        OperationStore::open(temp.path(), OperationLimits::default()),
        Err(OperationError::Locked)
    ));
    drop(store);
    let _reopened = OperationStore::open(temp.path(), OperationLimits::default())?;
    Ok(())
}

#[test]
fn expired_terminal_records_release_ids_keys_and_capacity() -> Result {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let mut store = OperationStore::open(
        temp.path(),
        OperationLimits {
            active: 1,
            retained: 1,
        },
    )?;
    let (first, _) = store.submit(request("one"))?;
    store.cancel(first.id)?;
    drop(store);
    let path = temp.path().join("operations.json");
    let mut snapshot: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
    snapshot["operations"][0]["created_at"] = 0.into();
    snapshot["operations"][0]["updated_at"] = 0.into();
    fs::write(&path, serde_json::to_vec(&snapshot)?)?;
    let mut store = OperationStore::open(
        temp.path(),
        OperationLimits {
            active: 1,
            retained: 1,
        },
    )?;
    assert!(matches!(store.get(first.id), Err(OperationError::NotFound)));
    let (second, created) = store.submit(request("one"))?;
    assert!(created);
    assert_ne!(first.id, second.id);
    Ok(())
}

#[test]
fn corruption_and_unknown_schema_are_not_silently_discarded() -> Result {
    for contents in [
        b"{truncated".as_slice(),
        br#"{"schema_version":2,"operations":[]}"#,
    ] {
        let temp = tempfile::tempdir()?;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
        drop(OperationStore::open(
            temp.path(),
            OperationLimits::default(),
        )?);
        fs::write(temp.path().join("operations.json"), contents)?;
        assert!(matches!(
            OperationStore::open(temp.path(), OperationLimits::default()),
            Err(OperationError::InvalidStore)
        ));
        assert_eq!(fs::read(temp.path().join("operations.json"))?, contents);
    }
    Ok(())
}

#[test]
fn persistence_failure_does_not_publish_a_phantom_operation() -> Result {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let mut store = OperationStore::open(temp.path(), OperationLimits::default())?;
    let (existing, _) = store.submit(request("one"))?;
    let path = temp.path().join("operations.json");
    let saved = temp.path().join("saved.json");
    fs::rename(&path, &saved)?;
    fs::create_dir(&path)?;
    assert!(matches!(
        store.submit(request("two")),
        Err(OperationError::Storage(_))
    ));
    assert!(matches!(
        store.get(existing.id),
        Err(OperationError::NeedsRecovery)
    ));
    assert!(matches!(
        store.submit(request("three")),
        Err(OperationError::NeedsRecovery)
    ));
    fs::remove_dir(&path)?;
    fs::rename(&saved, &path)?;
    drop(store);
    let mut store = OperationStore::open(temp.path(), OperationLimits::default())?;
    assert_eq!(store.get(existing.id)?.phase, WarmPhase::Interrupted);
    assert!(store.submit(request("two"))?.1);
    Ok(())
}

#[test]
fn insecure_paths_and_invalid_requests_are_rejected() -> Result {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let link = temp.path().join("alias");
    symlink(temp.path(), &link)?;
    assert!(matches!(
        OperationStore::open(&link, OperationLimits::default()),
        Err(OperationError::InvalidStore)
    ));
    let mut store = OperationStore::open(temp.path(), OperationLimits::default())?;
    assert!(matches!(
        store.submit(request("")),
        Err(OperationError::InvalidRequest)
    ));
    assert!(matches!(
        store.submit(request(&"x".repeat(129))),
        Err(OperationError::InvalidRequest)
    ));
    drop(store);
    fs::set_permissions(
        temp.path().join("operations.json"),
        fs::Permissions::from_mode(0o644),
    )?;
    assert!(matches!(
        OperationStore::open(temp.path(), OperationLimits::default()),
        Err(OperationError::InvalidStore)
    ));
    Ok(())
}
