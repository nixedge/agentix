//! Durable warm-up bookkeeping. Call from a blocking thread; this module performs fsync.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

mod runner;
pub use runner::{WarmLoader, WarmOperations};

const SNAPSHOT_LIMIT: u64 = 4 * 1024 * 1024;
const RETENTION_SECONDS: u64 = 24 * 60 * 60;

#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    #[error("operation store is unavailable")]
    Storage(#[source] std::io::Error),
    #[error("operation store has invalid data or permissions")]
    InvalidStore,
    #[error("operation store already has a writer")]
    Locked,
    #[error("operation request is invalid")]
    InvalidRequest,
    #[error("operation idempotency key belongs to a different request")]
    IdempotencyConflict,
    #[error("operation capacity exhausted")]
    Capacity,
    #[error("operation not found")]
    NotFound,
    #[error("operation changed or transition is invalid")]
    Conflict,
    #[error("operation store must be reopened after a write failure")]
    NeedsRecovery,
    #[error("operation task stopped unexpectedly")]
    TaskStopped,
}

impl From<std::io::Error> for OperationError {
    fn from(error: std::io::Error) -> Self {
        Self::Storage(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarmRequest {
    pub idempotency_key: String,
    pub model_id: String,
}

impl WarmRequest {
    fn valid(&self) -> bool {
        valid_text(&self.idempotency_key, 128) && valid_text(&self.model_id, 256)
    }
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.contains('\0')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarmPhase {
    Queued,
    Warming,
    Ready,
    Failed,
    Cancelled,
    Interrupted,
}

impl WarmPhase {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Warming)
    }
}

/// Fixed codes prevent backend diagnostics, paths or credentials being persisted as errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarmFailure {
    MissingModel,
    Unsupported,
    Capacity,
    BackendUnavailable,
    LoadFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarmOperation {
    pub id: Uuid,
    pub request: WarmRequest,
    pub phase: WarmPhase,
    pub sequence: u64,
    pub cancel_requested: bool,
    pub failure: Option<WarmFailure>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct OperationLimits {
    pub active: usize,
    pub retained: usize,
}

impl Default for OperationLimits {
    fn default() -> Self {
        Self {
            active: 8,
            retained: 1024,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    schema_version: u32,
    operations: Vec<WarmOperation>,
}

pub struct OperationStore {
    directory: PathBuf,
    // The OS lock is released on crash; an existing lock file alone is not ownership.
    _writer: File,
    operations: Vec<WarmOperation>,
    limits: OperationLimits,
    needs_recovery: bool,
}

impl OperationStore {
    pub fn open(
        directory: impl AsRef<Path>,
        limits: OperationLimits,
    ) -> Result<Self, OperationError> {
        if limits.active == 0 || limits.active > limits.retained || limits.retained > 1024 {
            return Err(OperationError::InvalidRequest);
        }
        let directory = directory.as_ref().to_owned();
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)?;
        let metadata = fs::symlink_metadata(&directory)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(OperationError::InvalidStore);
        }
        let lock_path = directory.join("writer.lock");
        check_private_file(&lock_path)?;
        let writer = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)?;
        writer.try_lock().map_err(|_| OperationError::Locked)?;
        let path = directory.join("operations.json");
        check_private_file(&path)?;
        let mut operations = match File::open(&path) {
            Ok(file) => {
                let mut bytes = vec![];
                file.take(SNAPSHOT_LIMIT + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > SNAPSHOT_LIMIT {
                    return Err(OperationError::InvalidStore);
                }
                let snapshot: Snapshot =
                    serde_json::from_slice(&bytes).map_err(|_| OperationError::InvalidStore)?;
                if snapshot.schema_version != 1 || snapshot.operations.len() > 1024 {
                    return Err(OperationError::InvalidStore);
                }
                snapshot.operations
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(error) => return Err(error.into()),
        };
        let mut ids = std::collections::HashSet::new();
        let mut keys = std::collections::HashSet::new();
        for operation in &operations {
            if !operation.request.valid()
                || operation.id.is_nil()
                || !ids.insert(operation.id)
                || !keys.insert(operation.request.idempotency_key.clone())
                || operation.updated_at < operation.created_at
                || (operation.phase == WarmPhase::Failed) != operation.failure.is_some()
                || (operation.phase == WarmPhase::Cancelled && !operation.cancel_requested)
            {
                return Err(OperationError::InvalidStore);
            }
        }
        let now = now()?;
        operations.retain(|operation| !expired(operation, now));
        for operation in &mut operations {
            if !operation.phase.is_terminal() {
                operation.phase = WarmPhase::Interrupted;
                operation.sequence = operation
                    .sequence
                    .checked_add(1)
                    .ok_or(OperationError::InvalidStore)?;
                operation.updated_at = now.max(operation.updated_at);
            }
        }
        let mut store = Self {
            directory,
            _writer: writer,
            operations: vec![],
            limits,
            needs_recovery: false,
        };
        store.commit(operations)?;
        Ok(store)
    }

    /// A retry returns the same operation, including terminal failures; retry work with a new key.
    pub fn submit(
        &mut self,
        request: WarmRequest,
    ) -> Result<(WarmOperation, bool), OperationError> {
        self.healthy()?;
        if !request.valid() {
            return Err(OperationError::InvalidRequest);
        }
        let now = now()?;
        let mut operations = self.operations.clone();
        operations.retain(|operation| !expired(operation, now));
        if let Some(existing) = operations
            .iter()
            .find(|operation| operation.request.idempotency_key == request.idempotency_key)
        {
            return if existing.request == request {
                Ok((existing.clone(), false))
            } else {
                Err(OperationError::IdempotencyConflict)
            };
        }
        if operations.len() >= self.limits.retained
            || operations
                .iter()
                .filter(|operation| !operation.phase.is_terminal())
                .count()
                >= self.limits.active
        {
            return Err(OperationError::Capacity);
        }
        let operation = WarmOperation {
            id: Uuid::new_v4(),
            request,
            phase: WarmPhase::Queued,
            sequence: 0,
            cancel_requested: false,
            failure: None,
            created_at: now,
            updated_at: now,
        };
        operations.push(operation.clone());
        self.commit(operations)?;
        Ok((operation, true))
    }

    pub fn get(&self, id: Uuid) -> Result<WarmOperation, OperationError> {
        self.healthy()?;
        let now = now()?;
        self.operations
            .iter()
            .find(|operation| operation.id == id && !expired(operation, now))
            .cloned()
            .ok_or(OperationError::NotFound)
    }

    pub fn start(&mut self, id: Uuid, sequence: u64) -> Result<WarmOperation, OperationError> {
        let mut operation = self.get(id)?;
        if operation.sequence != sequence || operation.phase != WarmPhase::Queued {
            return Err(OperationError::Conflict);
        }
        operation.phase = WarmPhase::Warming;
        self.replace(operation)
    }

    /// Cancellation during warming waits for the loader to return; it does not claim FFI stopped.
    pub fn cancel(&mut self, id: Uuid) -> Result<WarmOperation, OperationError> {
        let mut operation = self.get(id)?;
        if operation.phase.is_terminal() || operation.cancel_requested {
            return Ok(operation);
        }
        operation.cancel_requested = true;
        if operation.phase == WarmPhase::Queued {
            operation.phase = WarmPhase::Cancelled;
        }
        self.replace(operation)
    }

    /// Only the current warming generation may finish. Cancellation takes precedence over success.
    pub fn finish(
        &mut self,
        id: Uuid,
        sequence: u64,
        outcome: Result<(), WarmFailure>,
    ) -> Result<WarmOperation, OperationError> {
        let mut operation = self.get(id)?;
        // A cancel request may increment the sequence while the loader owns this generation.
        let expected = if operation.cancel_requested {
            sequence.checked_add(1)
        } else {
            Some(sequence)
        };
        if Some(operation.sequence) != expected || operation.phase != WarmPhase::Warming {
            return Err(OperationError::Conflict);
        }
        if operation.cancel_requested {
            operation.phase = WarmPhase::Cancelled;
        } else if let Err(failure) = outcome {
            operation.phase = WarmPhase::Failed;
            operation.failure = Some(failure);
        } else {
            operation.phase = WarmPhase::Ready;
        }
        self.replace(operation)
    }

    fn replace(&mut self, mut operation: WarmOperation) -> Result<WarmOperation, OperationError> {
        operation.sequence = operation
            .sequence
            .checked_add(1)
            .ok_or(OperationError::Conflict)?;
        operation.updated_at = now()?.max(operation.updated_at);
        let mut operations = self.operations.clone();
        let current = operations
            .iter_mut()
            .find(|item| item.id == operation.id)
            .ok_or(OperationError::NotFound)?;
        *current = operation.clone();
        self.commit(operations)?;
        Ok(operation)
    }

    fn healthy(&self) -> Result<(), OperationError> {
        if self.needs_recovery {
            Err(OperationError::NeedsRecovery)
        } else {
            Ok(())
        }
    }

    fn commit(&mut self, operations: Vec<WarmOperation>) -> Result<(), OperationError> {
        self.healthy()?;
        let bytes = serde_json::to_vec(&Snapshot {
            schema_version: 1,
            operations: operations.clone(),
        })
        .map_err(|_| OperationError::InvalidStore)?;
        if bytes.len() as u64 > SNAPSHOT_LIMIT {
            return Err(OperationError::Capacity);
        }
        let write = || -> Result<(), OperationError> {
            let mut temp = tempfile::NamedTempFile::new_in(&self.directory)?;
            temp.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
            temp.write_all(&bytes)?;
            temp.as_file().sync_all()?;
            temp.persist(self.directory.join("operations.json"))
                .map_err(|error| OperationError::Storage(error.error))?;
            File::open(&self.directory)?.sync_all()?;
            Ok(())
        };
        if let Err(error) = write() {
            // Rename may have succeeded even when directory fsync failed. Reopen before another write.
            self.needs_recovery = true;
            return Err(error);
        }
        self.operations = operations;
        Ok(())
    }
}

fn check_private_file(path: &Path) -> Result<(), OperationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.permissions().mode() & 0o077 == 0 => Ok(()),
        Ok(_) => Err(OperationError::InvalidStore),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn now() -> Result<u64, OperationError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| OperationError::InvalidStore)
}

fn expired(operation: &WarmOperation, now: u64) -> bool {
    operation.phase.is_terminal() && now.saturating_sub(operation.updated_at) >= RETENTION_SECONDS
}
