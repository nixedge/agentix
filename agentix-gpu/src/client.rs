use crate::protocol::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelPlan {
    pub peak_bytes: u64,
    #[serde(default)]
    pub model_sha256: Vec<String>,
    #[serde(default)]
    pub resident_floor_bytes: u64,
    #[serde(default)]
    pub context_tokens: u32,
    #[serde(default)]
    pub batch_tokens: u32,
    #[serde(default = "default_priority")]
    pub priority: u32,
}
impl ModelPlan {
    pub fn check_hashes(&self, hashes: &[String]) -> Result<()> {
        if hashes.is_empty()
            || self.model_sha256.len() != hashes.len()
            || self
                .model_sha256
                .iter()
                .zip(hashes)
                .any(|(expected, actual)| {
                    expected.len() != 64
                        || !expected.bytes().all(|b| b.is_ascii_hexdigit())
                        || !expected.eq_ignore_ascii_case(actual)
                })
        {
            return Err(Error::Capacity(
                "GPU memory plan does not match model shard hashes".into(),
            ));
        }
        Ok(())
    }
    pub fn check_shape(&self, context: usize, batch: usize) -> Result<()> {
        if context > self.context_tokens as usize || batch > self.batch_tokens as usize {
            return Err(Error::Capacity(
                "context or batch exceeds calibrated GPU peak plan".into(),
            ));
        }
        Ok(())
    }
}
fn default_priority() -> u32 {
    50
}

struct State {
    allocations: HashMap<String, Allocation>,
}
struct Inner {
    socket: PathBuf,
    owner: Owner,
    device: u32,
    wait: Duration,
    state: Mutex<State>,
    plans: HashMap<String, ModelPlan>,
}
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}
pub struct Lease {
    client: Client,
    id: String,
    released: AtomicBool,
}
static NEXT: AtomicU64 = AtomicU64::new(1);

impl Client {
    pub fn from_env(role: &str) -> Result<Option<Self>> {
        let Some(socket) = std::env::var_os("AGENTIX_GPU_COORDINATOR") else {
            return Ok(None);
        };
        let plans = serde_json::from_str(
            &std::env::var("AGENTIX_GPU_PLANS").unwrap_or_else(|_| "{}".into()),
        )?;
        let device = std::env::var("AGENTIX_GPU_DEVICE")
            .unwrap_or_else(|_| "0".into())
            .parse()
            .map_err(|_| Error::Invalid("invalid coordinated GPU device".into()))?;
        if std::env::var_os("CUDA_VISIBLE_DEVICES").is_some() {
            return Err(Error::Invalid("managed GPU device IDs must match physical NVIDIA indices; CUDA_VISIBLE_DEVICES is unsupported".into()));
        }
        Self::new(
            socket.into(),
            role.into(),
            device,
            plans,
            Duration::from_secs(30),
        )
        .map(Some)
    }
    pub fn new(
        socket: PathBuf,
        role: String,
        device: u32,
        plans: HashMap<String, ModelPlan>,
        wait: Duration,
    ) -> Result<Self> {
        let owner = Owner {
            role,
            process: Process::current()?,
            session: NEXT.fetch_add(1, Ordering::Relaxed),
        };
        let client = Self {
            inner: Arc::new(Inner {
                socket,
                owner,
                device,
                wait: wait.min(Duration::from_secs(30)),
                state: Mutex::new(State {
                    allocations: HashMap::new(),
                }),
                plans,
            }),
        };
        client.sync()?;
        let weak = Arc::downgrade(&client.inner);
        std::thread::Builder::new()
            .name("gpu-lease-sync".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(1));
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                // Failed reconciliation never authorizes new allocations. Existing
                // allocation owners retain their leases and retry next iteration.
                let _ = (Client { inner }).sync();
            })?;
        Ok(client)
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn sync_locked(&self, state: &State) -> Result<()> {
        call(
            &self.inner.socket,
            &Request::Sync {
                owner: self.inner.owner.clone(),
                allocations: state.allocations.values().cloned().collect(),
            },
            Duration::from_secs(3),
        )?;
        Ok(())
    }
    pub fn sync(&self) -> Result<()> {
        self.sync_locked(&self.lock())
    }
    pub fn device(&self) -> u32 {
        self.inner.device
    }
    pub fn plan(&self, model: &str) -> Result<&ModelPlan> {
        self.inner
            .plans
            .get(model)
            .filter(|plan| plan.peak_bytes > 0 && plan.resident_floor_bytes <= plan.peak_bytes)
            .ok_or_else(|| Error::Capacity(format!("missing calibrated GPU peak plan for {model}")))
    }
    pub fn reserve_model(&self, model: &str) -> Result<Lease> {
        let plan = self.plan(model)?;
        self.reserve(model, plan.peak_bytes, plan.priority, None)
    }
    pub fn reserve_cache(&self, model: &str, ceiling: u64, control: &Path) -> Result<Lease> {
        self.reserve(model, ceiling, 0, Some(control))
    }
    fn reserve(
        &self,
        model: &str,
        bytes: u64,
        priority: u32,
        control: Option<&Path>,
    ) -> Result<Lease> {
        let mut state = self.lock();
        self.sync_locked(&state)?;
        let id = format!(
            "{}-{}-{}",
            self.inner.owner.process.pid,
            self.inner.owner.process.start_ticks,
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let mut allocation = Allocation {
            id: id.clone(),
            model: model.into(),
            device: self.inner.device,
            bytes,
            resident_floor: 0,
            cache: control.is_some(),
            control: control.map(|p| p.to_string_lossy().into_owned()),
        };
        let response = call(
            &self.inner.socket,
            &Request::Reserve {
                owner: self.inner.owner.clone(),
                allocation: allocation.clone(),
                priority,
                wait_ms: self.inner.wait.as_millis() as u64,
            },
            self.inner.wait + Duration::from_secs(3),
        );
        match response {
            Ok(response) => {
                allocation.bytes = response.bytes;
                state.allocations.insert(id.clone(), allocation);
                Ok(Lease {
                    client: self.clone(),
                    id,
                    released: AtomicBool::new(false),
                })
            }
            Err(error) => {
                let _ = call(
                    &self.inner.socket,
                    &Request::Release {
                        owner: self.inner.owner.clone(),
                        id,
                    },
                    Duration::from_secs(3),
                );
                Err(error)
            }
        }
    }
    /// Called only after the engine has completed release. Reconciliation can
    /// publish this immediately; the requesting broker also receives the ack.
    pub fn cache_released(&self, id: &str, remaining: u64) -> Result<()> {
        let mut state = self.lock();
        let allocation = state
            .allocations
            .get_mut(id)
            .filter(|a| a.cache)
            .ok_or_else(|| Error::Invalid("unknown local expert-cache lease".into()))?;
        if remaining > allocation.bytes {
            return Err(Error::Invalid("release cannot grow a lease".into()));
        }
        allocation.bytes = remaining;
        allocation.resident_floor = allocation.resident_floor.min(remaining);
        Ok(())
    }
}
impl Lease {
    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn device(&self) -> u32 {
        self.client.device()
    }
    pub fn bytes(&self) -> u64 {
        if self.is_released() {
            return 0;
        }
        self.client
            .lock()
            .allocations
            .get(&self.id)
            .map_or(0, |a| a.bytes)
    }
    pub fn activate_plan(&self) -> Result<()> {
        let state = self.client.lock();
        let allocation = state
            .allocations
            .get(&self.id)
            .ok_or_else(|| Error::Invalid("unknown local GPU lease".into()))?;
        let floor = self.client.plan(&allocation.model)?.resident_floor_bytes;
        drop(state);
        self.activate_floor(floor)
    }
    pub fn activate_floor(&self, bytes: u64) -> Result<()> {
        if self.is_released() {
            return Err(Error::Invalid("GPU allocation owner has exited".into()));
        }
        let mut state = self.client.lock();
        let allocation = state
            .allocations
            .get_mut(&self.id)
            .ok_or_else(|| Error::Invalid("unknown local GPU lease".into()))?;
        if bytes > allocation.bytes {
            return Err(Error::Invalid("resident floor exceeds GPU lease".into()));
        }
        allocation.resident_floor = bytes;
        self.client.sync_locked(&state)
    }
    pub fn resize_cache(&self, bytes: u64) -> Result<()> {
        if self.is_released() {
            return Err(Error::Invalid("GPU allocation owner has exited".into()));
        }
        let mut state = self.client.lock();
        self.client.sync_locked(&state)?;
        let response = call(
            &self.client.inner.socket,
            &Request::ResizeCache {
                owner: self.client.inner.owner.clone(),
                id: self.id.clone(),
                bytes,
            },
            Duration::from_secs(3),
        )?;
        let allocation = state
            .allocations
            .get_mut(&self.id)
            .filter(|a| a.cache)
            .ok_or_else(|| Error::Invalid("unknown local cache lease".into()))?;
        allocation.bytes = response.bytes;
        allocation.resident_floor = allocation.resident_floor.min(response.bytes);
        Ok(())
    }
    /// The caller must have completed native destruction. Other Arc holders
    /// may observe this lease but can no longer reserve or refill it.
    pub fn release_after_free(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        // Another acquisition may hold the snapshot lock while it waits for
        // this native owner to finish. Release remotely before taking that lock.
        let _ = call(
            &self.client.inner.socket,
            &Request::Release {
                owner: self.client.inner.owner.clone(),
                id: self.id.clone(),
            },
            Duration::from_secs(3),
        );
        self.client.lock().allocations.remove(&self.id);
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.release_after_free();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peak_profile_is_bound_to_shards_context_and_batch() -> Result<()> {
        let plan = ModelPlan {
            peak_bytes: 100,
            model_sha256: vec!["a".repeat(64), "b".repeat(64)],
            resident_floor_bytes: 0,
            context_tokens: 4096,
            batch_tokens: 256,
            priority: 50,
        };
        plan.check_hashes(&["A".repeat(64), "B".repeat(64)])?;
        assert!(plan
            .check_hashes(&["b".repeat(64), "a".repeat(64)])
            .is_err());
        assert!(plan.check_hashes(&["a".repeat(64)]).is_err());
        assert!(plan.check_hashes(&[]).is_err());
        plan.check_shape(4096, 256)?;
        assert!(plan.check_shape(4097, 256).is_err());
        assert!(plan.check_shape(4096, 257).is_err());
        Ok(())
    }
}
