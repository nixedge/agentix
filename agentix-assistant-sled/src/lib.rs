mod apply;
mod keys;
mod queries;

use agentix_assistant_core::{Error, Result, SCHEMA_VERSION};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct SledRepository {
    inner: Arc<Inner>,
}

struct Inner {
    db: Option<sled::Db>,
    gate: Mutex<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let Some(db) = self.db.take() else {
            return;
        };
        // Sled's final handle release can flush and join its background worker.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn_blocking(move || drop(db));
        } else {
            drop(db);
        }
    }
}

impl SledRepository {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || {
            let db = sled::open(path).map_err(storage)?;
            if db.tree_names().len() != 1 {
                return Err(Error::Schema(
                    "refusing a database with foreign named trees".into(),
                ));
            }
            match db.get(keys::SCHEMA).map_err(storage)? {
                Some(version) if version.as_ref() == SCHEMA_VERSION.to_be_bytes() => {}
                Some(_) => return Err(Error::Schema("unrecognized assistant schema".into())),
                None => {
                    if !db.is_empty() {
                        return Err(Error::Schema(
                            "refusing an unversioned nonempty or legacy database".into(),
                        ));
                    }
                    db.insert(keys::SCHEMA, &SCHEMA_VERSION.to_be_bytes())
                        .map_err(storage)?;
                    db.flush().map_err(storage)?;
                }
            }
            Ok(Self {
                inner: Arc::new(Inner {
                    db: Some(db),
                    gate: Mutex::new(()),
                }),
            })
        })
        .await
        .map_err(storage)?
    }

    /// Await final engine release before reopening the path. Other repository
    /// handles and in-flight operations must be released first.
    pub async fn close(self) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            let mut inner = Arc::try_unwrap(self.inner).map_err(|_| {
                Error::Invalid("repository still has other handles or operations".into())
            })?;
            if let Some(db) = inner.db.take() {
                db.flush().map_err(storage)?;
                drop(db);
            }
            Ok(())
        })
        .await
        .map_err(storage)?
    }

    async fn blocking<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&sled::Db) -> Result<T> + Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            // Every public read/write shares this gate; batches are atomic in the engine too.
            let _guard = inner.gate.lock().map_err(storage)?;
            let db = inner
                .db
                .as_ref()
                .ok_or_else(|| Error::Storage("repository is closed".into()))?;
            operation(db)
        })
        .await
        .map_err(storage)?
    }
}

fn storage(error: impl std::fmt::Display) -> Error {
    Error::Storage(error.to_string())
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(storage)
}
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(storage)
}
