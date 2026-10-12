use super::{
    OperationError, OperationLimits, OperationStore, WarmFailure, WarmOperation, WarmRequest,
};
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[async_trait]
pub trait WarmLoader: Send + Sync {
    /// Return only once the load has stopped. Cancelling a job does not cancel this future.
    async fn warm(&self, model_id: &str) -> Result<(), WarmFailure>;
}

#[derive(Clone)]
pub struct WarmOperations {
    store: Arc<Mutex<OperationStore>>,
    loader: Arc<dyn WarmLoader>,
}

impl WarmOperations {
    pub async fn open(
        directory: PathBuf,
        limits: OperationLimits,
        loader: Arc<dyn WarmLoader>,
    ) -> Result<Self, OperationError> {
        let store = tokio::task::spawn_blocking(move || OperationStore::open(directory, limits))
            .await
            .map_err(|_| OperationError::TaskStopped)??;
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            loader,
        })
    }

    pub async fn submit(&self, request: WarmRequest) -> Result<WarmOperation, OperationError> {
        // The submission owns its continuation: disconnecting after the durable write must
        // not leave an accepted operation queued without a loader until daemon restart.
        let operations = self.clone();
        tokio::spawn(async move {
            let (operation, created) = operations
                .access(move |store| store.submit(request))
                .await?;
            if created {
                let queued = operation.clone();
                tokio::spawn(async move { operations.run(queued).await });
            }
            Ok(operation)
        })
        .await
        .map_err(|_| OperationError::TaskStopped)?
    }

    pub async fn get(&self, id: Uuid) -> Result<WarmOperation, OperationError> {
        self.access(move |store| store.get(id)).await
    }

    pub async fn cancel(&self, id: Uuid) -> Result<WarmOperation, OperationError> {
        self.access(move |store| store.cancel(id)).await
    }

    async fn run(self, queued: WarmOperation) {
        let warming = match self
            .access(move |store| store.start(queued.id, queued.sequence))
            .await
        {
            Ok(operation) => operation,
            // A queued cancellation has already persisted its terminal result.
            Err(OperationError::Conflict) => return,
            Err(_) => {
                tracing::error!("could not persist model warm-up start; operation needs recovery");
                return;
            }
        };
        let loader = Arc::clone(&self.loader);
        let model_id = warming.request.model_id.clone();
        // A panicking backend must not strand a live operation forever.
        let outcome = tokio::spawn(async move { loader.warm(&model_id).await })
            .await
            .unwrap_or(Err(WarmFailure::BackendUnavailable));
        if self
            .access(move |store| store.finish(warming.id, warming.sequence, outcome))
            .await
            .is_err()
        {
            tracing::error!("could not persist model warm-up outcome; operation needs recovery");
        }
    }

    async fn access<T, F>(&self, action: F) -> Result<T, OperationError>
    where
        T: Send + 'static,
        F: FnOnce(&mut OperationStore) -> Result<T, OperationError> + Send + 'static,
    {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            let mut store = store.lock().map_err(|_| OperationError::NeedsRecovery)?;
            action(&mut store)
        })
        .await
        .map_err(|_| OperationError::TaskStopped)?
    }
}
