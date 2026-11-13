use crate::{backend::LoadedModel, InferError};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Instant,
};

struct PoolSlot {
    loaded: Arc<dyn LoadedModel>,
    last_used: Instant,
}
type ReleaseFn = Box<dyn FnOnce(Arc<dyn LoadedModel>) + Send>;
pub struct ModelGuard {
    loaded: Arc<dyn LoadedModel>,
    on_release: Option<ReleaseFn>,
}
impl std::ops::Deref for ModelGuard {
    type Target = dyn LoadedModel;
    fn deref(&self) -> &Self::Target {
        self.loaded.as_ref()
    }
}
impl Drop for ModelGuard {
    fn drop(&mut self) {
        if let Some(release) = self.on_release.take() {
            release(Arc::clone(&self.loaded));
        }
    }
}
#[derive(Default)]
struct Accounting {
    count: usize,
    bytes: u64,
}
/// A native owner retains this token after its pool entry is evicted.
pub struct Reservation {
    accounting: Arc<Mutex<Accounting>>,
    bytes: u64,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut accounting = self.accounting.lock().unwrap_or_else(|p| p.into_inner());
        accounting.count = accounting.count.saturating_sub(1);
        accounting.bytes = accounting.bytes.saturating_sub(self.bytes);
    }
}
#[derive(Default)]
struct PoolState {
    idle: HashMap<String, Vec<PoolSlot>>,
}
impl PoolState {
    fn take_lru(&mut self) -> Option<Arc<dyn LoadedModel>> {
        let oldest = self
            .idle
            .iter()
            .flat_map(|(name, slots)| {
                slots
                    .iter()
                    .enumerate()
                    .map(move |(index, slot)| (name.clone(), index, slot.last_used))
            })
            .min_by_key(|(_, _, used)| *used);
        let (name, index, _) = oldest?;
        let slots = self.idle.get_mut(&name)?;
        let slot = slots.remove(index);
        if slots.is_empty() {
            self.idle.remove(&name);
        }
        Some(slot.loaded)
    }
}
pub struct ContextPool {
    state: Arc<Mutex<PoolState>>,
    accounting: Arc<Mutex<Accounting>>,
    loading: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    vram_limit: Option<u64>,
    max_loaded_models: usize,
}
impl ContextPool {
    pub fn new(vram_limit: Option<u64>, max_loaded_models: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(PoolState::default())),
            accounting: Arc::new(Mutex::new(Accounting::default())),
            loading: Mutex::new(HashMap::new()),
            vram_limit,
            max_loaded_models,
        }
    }
    pub async fn lock_loading(&self, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut loading = self.loading.lock().unwrap_or_else(|p| p.into_inner());
            loading.retain(|_, lock| lock.strong_count() > 0);
            loading
                .get(name)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let lock = Arc::new(tokio::sync::Mutex::new(()));
                    loading.insert(name.into(), Arc::downgrade(&lock));
                    lock
                })
        };
        lock.lock_owned().await
    }
    pub fn reserve(&self, name: &str, bytes: u64) -> Result<Reservation, InferError> {
        loop {
            {
                let mut accounting = self.accounting.lock().unwrap_or_else(|p| p.into_inner());
                let total = accounting.bytes.checked_add(bytes);
                let fits =
                    total.is_some_and(|total| self.vram_limit.is_none_or(|limit| total <= limit));
                if fits && accounting.count < self.max_loaded_models {
                    accounting.bytes = total
                        .ok_or_else(|| InferError::Backend("pool accounting overflow".into()))?;
                    accounting.count += 1;
                    return Ok(Reservation {
                        accounting: Arc::clone(&self.accounting),
                        bytes,
                    });
                }
            }
            let evicted = self
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take_lru();
            match evicted {
                Some(model) => drop(model),
                None => {
                    return Err(InferError::VramExhausted {
                        model: name.into(),
                        required_bytes: bytes,
                    })
                }
            }
        }
    }
    fn guard(&self, name: &str, loaded: Arc<dyn LoadedModel>) -> ModelGuard {
        let state = Arc::clone(&self.state);
        let name = name.to_owned();
        ModelGuard {
            loaded,
            on_release: Some(Box::new(move |model| {
                state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .idle
                    .entry(name)
                    .or_default()
                    .push(PoolSlot {
                        loaded: model,
                        last_used: Instant::now(),
                    });
            })),
        }
    }
    pub fn acquire_idle(&self, name: &str) -> Option<ModelGuard> {
        let slot = self.state.lock().ok()?.idle.get_mut(name)?.pop()?;
        Some(self.guard(name, slot.loaded))
    }
    pub fn store_reserved(&self, name: &str, loaded: Arc<dyn LoadedModel>) -> ModelGuard {
        self.guard(name, loaded)
    }
    pub fn store_loaded(
        &self,
        name: &str,
        loaded: Arc<dyn LoadedModel>,
        needed_vram: u64,
    ) -> Result<ModelGuard, InferError> {
        let local = self.reserve(name, needed_vram)?;
        let loaded = crate::memory::LoadReservation {
            local: Some(local),
            shared: None,
            peak_bytes: needed_vram,
            plan: None,
        }
        .wrap(loaded);
        Ok(self.guard(name, loaded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_and_pending_reservations_are_counted_before_allocation() -> Result<(), InferError> {
        let pool = ContextPool::new(Some(100), 3);
        let active = pool.reserve("active", 60)?;
        assert!(matches!(
            pool.reserve("pending", 50),
            Err(InferError::VramExhausted { .. })
        ));
        let pending = pool.reserve("pending", 40)?;
        assert!(pool.reserve("third", 1).is_err());
        drop(active);
        drop(pending);
        let _all = pool.reserve("all", 100)?;
        Ok(())
    }
    #[test]
    fn allocation_is_charged_through_delayed_native_shutdown(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let pool = ContextPool::new(Some(100), 2);
        let allocation = pool.reserve("native", 90)?;
        let (release, released) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _allocation = allocation;
            let _ = released.recv();
        });
        assert!(pool.reserve("competing", 20).is_err());
        release.send(())?;
        worker.join().map_err(|_| "native test worker panicked")?;
        let _allocation = pool.reserve("competing", 100)?;
        Ok(())
    }
    #[test]
    fn count_limit_and_overflow_fail_closed() -> Result<(), InferError> {
        let pool = ContextPool::new(None, 1);
        let held = pool.reserve("one", 1)?;
        assert!(pool.reserve("two", 1).is_err());
        drop(held);
        let huge = pool.reserve("huge", u64::MAX)?;
        assert!(pool.reserve("overflow", 1).is_err());
        drop(huge);
        assert!(ContextPool::new(None, 0).reserve("disabled", 1).is_err());
        Ok(())
    }
    #[tokio::test]
    async fn cancelled_loading_guard_does_not_strand_waiters(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let pool = ContextPool::new(None, 1);
        let first = pool.lock_loading("model").await;
        let mut next = Box::pin(pool.lock_loading("model"));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), next.as_mut())
                .await
                .is_err()
        );
        drop(first);
        let _next = tokio::time::timeout(std::time::Duration::from_secs(1), next).await?;
        Ok(())
    }
}
