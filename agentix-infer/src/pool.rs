use crate::{backend::LoadedModel, InferError};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Instant,
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

struct PoolSlot {
    loaded: Arc<dyn LoadedModel>,
    last_used: Instant,
    busy: bool,
}

impl PoolSlot {
    fn is_busy(&self) -> bool {
        self.busy || self.loaded.is_busy()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Residency {
    Cold,
    Idle,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolCapacity {
    pub resident_models: usize,
    pub active_models: usize,
    pub resident_bytes: u64,
    pub max_models: usize,
    pub limit_bytes: Option<u64>,
}

pub struct ModelGuard {
    loaded: Arc<dyn LoadedModel>,
    state: Arc<Mutex<HashMap<String, PoolSlot>>>,
    name: String,
}

impl std::ops::Deref for ModelGuard {
    type Target = dyn LoadedModel;
    fn deref(&self) -> &Self::Target {
        self.loaded.as_ref()
    }
}

impl Drop for ModelGuard {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(slot) = state.get_mut(&self.name) {
            slot.busy = false;
            slot.last_used = Instant::now();
        }
    }
}

pub struct ContextPool {
    state: Arc<Mutex<HashMap<String, PoolSlot>>>,
    loading: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
    vram_limit: Option<u64>,
    max_loaded_models: usize,
}

impl ContextPool {
    pub fn new(vram_limit: Option<u64>, max_loaded_models: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            loading: Mutex::new(HashMap::new()),
            vram_limit,
            max_loaded_models,
        }
    }

    // An owned mutex guard clears serialization even if loading is cancelled.
    // Weak entries avoid retaining every model name ever requested.
    pub async fn lock_loading(&self, name: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut loading = self.loading.lock().unwrap_or_else(|p| p.into_inner());
            loading.retain(|_, lock| lock.strong_count() > 0);
            match loading.get(name).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(AsyncMutex::new(()));
                    loading.insert(name.to_string(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }

    pub fn residency(&self, name: &str) -> Residency {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        match state.get(name) {
            None => Residency::Cold,
            Some(slot) if slot.is_busy() => Residency::Busy,
            Some(_) => Residency::Idle,
        }
    }

    pub fn capacity(&self) -> PoolCapacity {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        PoolCapacity {
            resident_models: state.len(),
            active_models: state.values().filter(|slot| slot.is_busy()).count(),
            resident_bytes: state.values().fold(0u64, |sum, slot| {
                sum.saturating_add(slot.loaded.vram_bytes())
            }),
            max_models: self.max_loaded_models,
            limit_bytes: self.vram_limit,
        }
    }

    pub fn model_details(&self, name: &str) -> Option<(bool, u32)> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(name)
            .map(|slot| (slot.loaded.coding_supported(), slot.loaded.context_limit()))
    }

    pub fn acquire_idle(&self, name: &str) -> Option<ModelGuard> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let slot = state.get_mut(name)?;
        if slot.is_busy() {
            return None;
        }
        slot.busy = true;
        Some(self.guard(name, Arc::clone(&slot.loaded)))
    }

    fn guard(&self, name: &str, loaded: Arc<dyn LoadedModel>) -> ModelGuard {
        ModelGuard {
            loaded,
            state: Arc::clone(&self.state),
            name: name.to_string(),
        }
    }

    pub fn store_loaded(
        &self,
        name: &str,
        loaded: Arc<dyn LoadedModel>,
        needed_vram: u64,
    ) -> Result<ModelGuard, InferError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.contains_key(name) {
            return Err(InferError::ModelBusy(name.to_string()));
        }
        // Check feasibility before evicting anything; rejected admission must
        // preserve the previous warm model selection.
        let needed_vram = needed_vram.max(loaded.vram_bytes());
        let active = state.values().filter(|slot| slot.is_busy());
        let active_bytes: u128 = active
            .clone()
            .map(|s| u128::from(s.loaded.vram_bytes()))
            .sum();
        if active.count() >= self.max_loaded_models {
            return Err(InferError::CapacityExhausted);
        }
        if self
            .vram_limit
            .is_some_and(|limit| active_bytes + u128::from(needed_vram) > u128::from(limit))
        {
            return Err(InferError::VramExhausted {
                model: name.to_string(),
                required_bytes: needed_vram,
            });
        }
        loop {
            let bytes: u128 = state
                .values()
                .map(|s| u128::from(s.loaded.vram_bytes()))
                .sum();
            let over_bytes = self
                .vram_limit
                .is_some_and(|limit| bytes + u128::from(needed_vram) > u128::from(limit));
            if state.len() < self.max_loaded_models && !over_bytes {
                break;
            }
            let oldest = state
                .iter()
                .filter(|(_, slot)| !slot.is_busy())
                .min_by_key(|(_, slot)| slot.last_used)
                .map(|(name, _)| name.clone());
            match oldest {
                Some(name) => {
                    state.remove(&name);
                }
                None => return Err(InferError::CapacityExhausted),
            }
        }
        state.insert(
            name.to_string(),
            PoolSlot {
                loaded: Arc::clone(&loaded),
                last_used: Instant::now(),
                busy: true,
            },
        );
        Ok(self.guard(name, loaded))
    }
}
