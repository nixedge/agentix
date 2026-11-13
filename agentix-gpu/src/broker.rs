use crate::{
    policy::{available, external_bytes, Held, Observation},
    protocol::*,
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

pub trait Probe: Send + Sync {
    fn observe(&self, device: u32) -> Result<Observation>;
}
pub struct NvidiaProbe;
fn nvidia(args: &[&str]) -> Result<std::process::Output> {
    let mut child = Command::new("nvidia-smi")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Invalid("NVIDIA observation timed out".into()));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(child.wait_with_output()?)
}
impl Probe for NvidiaProbe {
    fn observe(&self, device: u32) -> Result<Observation> {
        let started = Instant::now();
        let output = nvidia(&[
            "--query-gpu=uuid,memory.total,memory.free",
            "--format=csv,noheader,nounits",
            "--id",
            &device.to_string(),
        ])?;
        if !output.status.success() {
            return Err(Error::Invalid("NVIDIA device observation failed".into()));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let values: Vec<_> = text.trim().split(',').map(str::trim).collect();
        if values.len() != 3 {
            return Err(Error::Invalid("invalid NVIDIA device observation".into()));
        }
        let mib = |s: &str| -> Result<u64> {
            s.parse::<u64>()
                .ok()
                .and_then(|v| v.checked_mul(1 << 20))
                .ok_or_else(|| Error::Invalid("invalid NVIDIA memory measurement".into()))
        };
        let mut observation = Observation {
            device,
            uuid: values[0].into(),
            total: mib(values[1])?,
            free: mib(values[2])?.saturating_sub(1 << 20),
            processes: HashMap::new(),
        };
        let processes = nvidia(&[
            "--query-compute-apps=gpu_uuid,pid,used_gpu_memory",
            "--format=csv,noheader,nounits",
            "--id",
            &device.to_string(),
        ])?;
        if processes.status.success() {
            for line in String::from_utf8_lossy(&processes.stdout).lines() {
                let fields: Vec<_> = line.split(',').map(str::trim).collect();
                if fields.len() == 3 && fields[0] == observation.uuid {
                    if let (Ok(pid), Ok(bytes)) = (fields[1].parse(), mib(fields[2])) {
                        // nvidia-smi reports whole MiB; round the resident estimate
                        // down so future-growth slack is never understated.
                        observation
                            .processes
                            .insert(pid, bytes.saturating_sub(1 << 20));
                    }
                }
            }
        }
        if started.elapsed() > Duration::from_secs(1) {
            return Err(Error::Invalid(
                "NVIDIA observation is too old for admission".into(),
            ));
        }
        Ok(observation)
    }
}

struct Pending {
    id: String,
    owner: Owner,
    bytes: u64,
    priority: u32,
    entered: Instant,
    sequence: u64,
}
struct State {
    devices: HashSet<u32>,
    held: HashMap<String, Held>,
    released: HashMap<String, Owner>,
    roles: HashMap<String, Owner>,
    seen_roles: HashSet<String>,
    pending: Vec<Pending>,
    sequence: u64,
    processing: bool,
    cooldown: HashMap<u32, Instant>,
    observations: HashMap<u32, (Observation, Instant)>,
    failures: u64,
    reclaimed: u64,
    reclaim_failures: u64,
}
pub struct Broker {
    state: Mutex<State>,
    changed: Condvar,
    probe: Arc<dyn Probe>,
    roles: HashSet<String>,
    safety: u64,
    cooldown: Duration,
    epoch: String,
}
impl Broker {
    pub fn new(
        probe: Arc<dyn Probe>,
        roles: Vec<String>,
        safety: u64,
        cooldown: Duration,
    ) -> Result<Self> {
        if roles.is_empty() || roles.iter().any(|r| r.is_empty()) {
            return Err(Error::Invalid(
                "expected GPU worker roles are required".into(),
            ));
        }
        let owner = Process::current()?;
        Ok(Self {
            state: Mutex::new(State {
                devices: HashSet::new(),
                held: HashMap::new(),
                released: HashMap::new(),
                roles: HashMap::new(),
                seen_roles: HashSet::new(),
                pending: Vec::new(),
                sequence: 0,
                processing: false,
                cooldown: HashMap::new(),
                observations: HashMap::new(),
                failures: 0,
                reclaimed: 0,
                reclaim_failures: 0,
            }),
            changed: Condvar::new(),
            probe,
            roles: roles.into_iter().collect(),
            safety,
            cooldown,
            epoch: format!("{}-{}", owner.pid, owner.start_ticks),
        })
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn validate(&self, owner: &Owner) -> Result<()> {
        if !self.roles.contains(&owner.role) || !owner.process.verified_alive() {
            return Err(Error::Invalid(
                "unknown or dead GPU worker incarnation".into(),
            ));
        }
        Ok(())
    }
    fn validate_registered(&self, owner: &Owner) -> Result<()> {
        self.validate(owner)?;
        if self.lock().roles.get(&owner.role) != Some(owner) {
            return Err(Error::Invalid(
                "GPU worker must reconcile before allocation".into(),
            ));
        }
        Ok(())
    }
    fn reap(state: &mut State) {
        state.held.retain(|_, h| h.owner.process.alive());
        state.roles.retain(|_, owner| owner.process.alive());
        state.pending.retain(|p| p.owner.process.alive());
        state.released.retain(|_, owner| owner.process.alive());
    }
    pub fn handle(&self, request: Request) -> Response {
        match self.apply(request) {
            Ok(bytes) => Response::success(bytes, &self.epoch),
            Err(error) => {
                self.lock().failures += 1;
                Response::failure(error, &self.epoch)
            }
        }
    }
    fn apply(&self, request: Request) -> Result<u64> {
        match request {
            Request::Sync { owner, allocations } => {
                self.validate(&owner)?;
                let mut state = self.lock();
                Self::reap(&mut state);
                if state.roles.get(&owner.role).is_some_and(|o| o != &owner) {
                    return Err(Error::Invalid(
                        "another live process owns the GPU worker role".into(),
                    ));
                }
                let mut ids = HashSet::new();
                for allocation in &allocations {
                    if allocation.resident_floor > allocation.bytes
                        || allocation.id.is_empty()
                        || allocation.id.len() > 256
                        || !ids.insert(&allocation.id)
                        || state
                            .held
                            .get(&allocation.id)
                            .is_some_and(|h| h.owner != owner)
                    {
                        return Err(Error::Invalid(
                            "invalid or duplicate GPU lease identity".into(),
                        ));
                    }
                }
                state.held.retain(|_, h| h.owner != owner);
                state.roles.insert(owner.role.clone(), owner.clone());
                state.seen_roles.insert(owner.role.clone());
                for allocation in allocations {
                    // Release may race an older heartbeat snapshot. A unique
                    // lease ID must never be resurrected after native destruction.
                    if state.released.contains_key(&allocation.id) {
                        continue;
                    }
                    state.held.insert(
                        allocation.id.clone(),
                        Held {
                            owner: owner.clone(),
                            allocation,
                        },
                    );
                }
                self.changed.notify_all();
                Ok(0)
            }
            Request::Reserve {
                owner,
                allocation,
                priority,
                wait_ms,
            } => {
                self.validate_registered(&owner)?;
                if allocation.resident_floor != 0
                    || allocation.id.is_empty()
                    || allocation.bytes == 0
                    || (allocation.cache && allocation.control.is_none())
                {
                    return Err(Error::Invalid("invalid GPU reservation".into()));
                }
                self.reserve(
                    owner,
                    allocation,
                    priority,
                    Duration::from_millis(wait_ms.min(30_000)),
                )
            }
            Request::Release { owner, id } => {
                self.validate_registered(&owner)?;
                let mut state = self.lock();
                if state.held.get(&id).is_some_and(|h| h.owner != owner) {
                    return Err(Error::Invalid("GPU lease belongs to another worker".into()));
                }
                state.held.remove(&id);
                state.released.insert(id.clone(), owner.clone());
                state.pending.retain(|p| p.id != id || p.owner != owner);
                self.changed.notify_all();
                Ok(0)
            }
            Request::ResizeCache { owner, id, bytes } => {
                self.validate_registered(&owner)?;
                let mut state = self.lock();
                let held = state
                    .held
                    .get(&id)
                    .filter(|h| h.owner == owner && h.allocation.cache)
                    .cloned()
                    .ok_or_else(|| Error::Invalid("unknown cache lease".into()))?;
                let old = held.allocation.bytes;
                if bytes > old {
                    if !state.pending.is_empty()
                        || state.processing
                        || state
                            .cooldown
                            .get(&held.allocation.device)
                            .is_some_and(|until| Instant::now() < *until)
                    {
                        return Err(Error::Capacity("expert refill deferred".into()));
                    }
                    let observation = self.probe.observe(held.allocation.device)?;
                    let entries = state.held.values().cloned().collect::<Vec<_>>();
                    let room = available(&observation, &entries, self.safety)?;
                    state
                        .observations
                        .insert(observation.device, (observation, Instant::now()));
                    if room < 256 * 1024 * 1024 {
                        return Err(Error::Capacity(
                            "expert refill below hysteresis increment".into(),
                        ));
                    }
                    let granted = old.saturating_add(room.min(bytes - old));
                    if let Some(held) = state.held.get_mut(&id) {
                        held.allocation.bytes = granted;
                    }
                    return Ok(granted);
                }
                if let Some(held) = state.held.get_mut(&id) {
                    held.allocation.bytes = bytes;
                    held.allocation.resident_floor = held.allocation.resident_floor.min(bytes);
                }
                if bytes < old {
                    state
                        .cooldown
                        .insert(held.allocation.device, Instant::now() + self.cooldown);
                }
                self.changed.notify_all();
                Ok(bytes)
            }
            Request::Status => Ok(0),
            Request::Reclaim { .. } => Err(Error::Invalid(
                "reclaim is a worker control operation".into(),
            )),
        }
    }
    fn reserve(
        &self,
        owner: Owner,
        allocation: Allocation,
        priority: u32,
        wait: Duration,
    ) -> Result<u64> {
        let deadline = Instant::now() + wait;
        {
            let mut state = self.lock();
            if state.held.contains_key(&allocation.id)
                || state.released.contains_key(&allocation.id)
                || state.pending.iter().any(|p| p.id == allocation.id)
                || state.pending.len() >= 128
            {
                return Err(Error::Invalid(
                    "duplicate lease or full admission queue".into(),
                ));
            }
            let sequence = state.sequence;
            state.sequence = state.sequence.wrapping_add(1);
            state.pending.push(Pending {
                id: allocation.id.clone(),
                owner: owner.clone(),
                bytes: allocation.bytes,
                priority,
                entered: Instant::now(),
                sequence,
            });
            state.devices.insert(allocation.device);
        }
        let outcome = self.wait_reserve(&allocation, deadline);
        let mut state = self.lock();
        state.pending.retain(|p| p.id != allocation.id);
        self.changed.notify_all();
        outcome
    }
    fn wait_reserve(&self, allocation: &Allocation, deadline: Instant) -> Result<u64> {
        loop {
            let mut state = self.lock();
            Self::reap(&mut state);
            let front = state.pending.iter().max_by_key(|p| {
                (
                    u64::from(p.priority) + p.entered.elapsed().as_secs() / 5,
                    std::cmp::Reverse(p.sequence),
                )
            });
            let owner = state
                .pending
                .iter()
                .find(|p| p.id == allocation.id)
                .map(|p| p.owner.clone())
                .ok_or_else(|| Error::Capacity("reservation cancelled".into()))?;
            let ready = self.roles.iter().all(|r| state.seen_roles.contains(r));
            if !state.processing && ready && front.is_some_and(|p| p.id == allocation.id) {
                state.processing = true;
                drop(state);
                let decision = self.try_reserve(&owner, allocation, deadline);
                state = self.lock();
                state.processing = false;
                self.changed.notify_all();
                match decision {
                    Ok(Some(bytes)) => return Ok(bytes),
                    Err(error) => return Err(error),
                    Ok(None) => {}
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::Capacity(
                    if ready {
                        "bounded GPU admission wait expired"
                    } else {
                        "GPU workers have not reconciled with coordinator"
                    }
                    .into(),
                ));
            }
            let _ = self
                .changed
                .wait_timeout(
                    state,
                    Duration::from_millis(100)
                        .min(deadline.saturating_duration_since(Instant::now())),
                )
                .unwrap_or_else(|p| p.into_inner());
        }
    }
    fn try_reserve(
        &self,
        owner: &Owner,
        allocation: &Allocation,
        deadline: Instant,
    ) -> Result<Option<u64>> {
        let observation = self.probe.observe(allocation.device)?;
        if !allocation.cache && allocation.bytes > observation.total.saturating_sub(self.safety) {
            return Err(Error::Capacity(
                "non-reclaimable peak exceeds device capacity".into(),
            ));
        }
        let mut state = self.lock();
        let held = state.held.values().cloned().collect::<Vec<_>>();
        if !state
            .pending
            .iter()
            .any(|pending| pending.id == allocation.id)
        {
            return Err(Error::Capacity(
                "GPU reservation cancelled before grant".into(),
            ));
        }
        let room = available(&observation, &held, self.safety)?;
        state
            .observations
            .insert(observation.device, (observation, Instant::now()));
        let cooling = state
            .cooldown
            .get(&allocation.device)
            .is_some_and(|until| *until > Instant::now());
        let granted = if allocation.cache {
            if cooling {
                0
            } else {
                room.min(allocation.bytes)
            }
        } else {
            allocation.bytes
        };
        if granted <= room && (granted > 0 || allocation.cache) {
            let mut allocation = allocation.clone();
            allocation.bytes = granted;
            state.held.insert(
                allocation.id.clone(),
                Held {
                    owner: owner.clone(),
                    allocation,
                },
            );
            return Ok(Some(granted));
        }
        let victims = held
            .into_iter()
            .filter(|h| {
                h.allocation.device == allocation.device
                    && h.allocation.cache
                    && h.allocation.resident_floor > 0
            })
            .collect::<Vec<_>>();
        drop(state);
        for victim in victims {
            let Some(path) = &victim.allocation.control else {
                continue;
            };
            let timeout = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(2));
            if timeout.is_zero() {
                break;
            }
            let response = call(
                Path::new(path),
                &Request::Reclaim {
                    id: victim.allocation.id.clone(),
                },
                timeout,
            );
            let mut state = self.lock();
            match response {
                Ok(response) if response.bytes <= victim.allocation.bytes => {
                    if let Some(entry) = state.held.get_mut(&victim.allocation.id) {
                        entry.allocation.bytes = response.bytes;
                        entry.allocation.resident_floor = response.bytes;
                    }
                    state.reclaimed = state
                        .reclaimed
                        .saturating_add(victim.allocation.bytes - response.bytes);
                    state
                        .cooldown
                        .insert(allocation.device, Instant::now() + self.cooldown);
                }
                _ => state.reclaim_failures += 1,
            }
        }
        // A new driver observation on the next iteration must confirm actual
        // headroom; an acknowledgement alone does not grant device capacity.
        Ok(None)
    }
    pub fn metrics(&self) -> String {
        let mut state = self.lock();
        Self::reap(&mut state);
        let held = state.held.values().cloned().collect::<Vec<_>>();
        let mut output = format!("agentix_gpu_admission_failures_total {}\nagentix_gpu_reclaimed_bytes_total {}\nagentix_gpu_reclaim_failures_total {}\n", state.failures, state.reclaimed, state.reclaim_failures);
        for device in state.devices.iter().copied().collect::<Vec<_>>() {
            match self.probe.observe(device) {
                Ok(observation) => {
                    state
                        .observations
                        .insert(device, (observation, Instant::now()));
                }
                Err(_) => {
                    state.observations.remove(&device);
                    output.push_str(&format!(
                        "agentix_gpu_observation_success{{device=\"{device}\"}} 0\n"
                    ));
                    continue;
                }
            }
            if let Some((observation, sampled)) = state.observations.get(&device) {
                output.push_str(&format!("agentix_gpu_observation_success{{device=\"{device}\"}} 1\nagentix_gpu_device_free_bytes{{device=\"{device}\"}} {}\nagentix_gpu_device_total_bytes{{device=\"{device}\"}} {}\nagentix_gpu_external_bytes{{device=\"{device}\"}} {}\nagentix_gpu_observation_age_seconds{{device=\"{device}\"}} {}\n", observation.free, observation.total, external_bytes(observation, &held), sampled.elapsed().as_secs_f64()));
            }
        }
        for role in &self.roles {
            let entries = held
                .iter()
                .filter(|h| &h.owner.role == role)
                .collect::<Vec<_>>();
            let reserved = entries
                .iter()
                .fold(0u64, |n, h| n.saturating_add(h.allocation.bytes));
            let cache = entries
                .iter()
                .filter(|h| h.allocation.cache)
                .fold(0u64, |n, h| n.saturating_add(h.allocation.resident_floor));
            let cache_reserved = entries
                .iter()
                .filter(|h| h.allocation.cache)
                .fold(0u64, |n, h| n.saturating_add(h.allocation.bytes));
            let floor = entries
                .iter()
                .fold(0u64, |n, h| n.saturating_add(h.allocation.resident_floor));
            let pending = state
                .pending
                .iter()
                .filter(|p| &p.owner.role == role)
                .fold(0u64, |n, p| n.saturating_add(p.bytes));
            // JSON escaping is also valid for Prometheus quoted label values.
            let label = serde_json::to_string(role).unwrap_or_else(|_| "\"invalid\"".into());
            output.push_str(&format!("agentix_gpu_worker_reconciled{{worker={label}}} {}\nagentix_gpu_worker_alive{{worker={label}}} {}\nagentix_gpu_cache_reserved_bytes{{worker={label}}} {cache_reserved}\n", u8::from(state.seen_roles.contains(role)), u8::from(state.roles.contains_key(role))));
            output.push_str(&format!("agentix_gpu_reserved_bytes{{worker={label}}} {reserved}\nagentix_gpu_resident_floor_bytes{{worker={label}}} {floor}\nagentix_gpu_reclaimable_bytes{{worker={label}}} {cache}\nagentix_gpu_pending_bytes{{worker={label}}} {pending}\n"));
            if let Some(owner) = state.roles.get(role) {
                output.push_str(&format!("agentix_gpu_worker_info{{worker={label},pid=\"{}\",start_ticks=\"{}\",session=\"{}\"}} 1\nagentix_gpu_worker_identity_verified{{worker={label}}} {}\n", owner.process.pid, owner.process.start_ticks, owner.session, u8::from(owner.process.verified_alive())));
                let mut resident = 0u64;
                let mut known = false;
                for (observation, _) in state.observations.values() {
                    if let Some(bytes) = observation.processes.get(&owner.process.pid) {
                        resident = resident.saturating_add(*bytes);
                        known = true;
                    }
                }
                if known {
                    output.push_str(&format!(
                        "agentix_gpu_resident_bytes{{worker={label}}} {resident}\n"
                    ));
                }
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    };
    struct FakeProbe {
        free: AtomicU64,
        used: AtomicU64,
        fail: AtomicBool,
    }
    impl Probe for FakeProbe {
        fn observe(&self, device: u32) -> Result<Observation> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(Error::Invalid("probe failed".into()));
            }
            Ok(Observation {
                device,
                uuid: "test-gpu".into(),
                total: 100,
                free: self.free.load(Ordering::SeqCst),
                processes: [(std::process::id(), self.used.load(Ordering::SeqCst))]
                    .into_iter()
                    .collect(),
            })
        }
    }
    fn setup() -> Result<(Arc<Broker>, Arc<FakeProbe>, Owner, Owner)> {
        let probe = Arc::new(FakeProbe {
            free: AtomicU64::new(100),
            used: AtomicU64::new(0),
            fail: AtomicBool::new(false),
        });
        let broker = Arc::new(Broker::new(
            probe.clone(),
            vec!["qwen".into(), "llama".into()],
            5,
            Duration::from_secs(30),
        )?);
        let qwen = Owner {
            role: "qwen".into(),
            process: Process::current()?,
            session: 1,
        };
        let llama = Owner {
            role: "llama".into(),
            process: Process::current()?,
            session: 2,
        };
        broker
            .handle(Request::Sync {
                owner: qwen.clone(),
                allocations: vec![],
            })
            .check()?;
        broker
            .handle(Request::Sync {
                owner: llama.clone(),
                allocations: vec![],
            })
            .check()?;
        Ok((broker, probe, qwen, llama))
    }
    fn allocation(id: &str, bytes: u64) -> Allocation {
        Allocation {
            id: id.into(),
            model: id.into(),
            device: 0,
            bytes,
            resident_floor: 0,
            cache: false,
            control: None,
        }
    }
    fn reserve(broker: &Broker, owner: &Owner, id: &str, bytes: u64) -> Response {
        broker.handle(Request::Reserve {
            owner: owner.clone(),
            allocation: allocation(id, bytes),
            priority: 50,
            wait_ms: 0,
        })
    }
    #[test]
    fn pending_and_active_peaks_are_charged_until_release() -> Result<()> {
        let (broker, _, qwen, llama) = setup()?;
        reserve(&broker, &qwen, "loading", 60).check()?;
        assert!(!reserve(&broker, &llama, "other", 40).ok);
        broker
            .handle(Request::Release {
                owner: qwen,
                id: "loading".into(),
            })
            .check()?;
        reserve(&broker, &llama, "other", 40).check()?;
        Ok(())
    }
    #[test]
    fn failed_observation_and_impossible_peak_fail_closed() -> Result<()> {
        let (broker, probe, qwen, _) = setup()?;
        assert!(!reserve(&broker, &qwen, "too-big", 96).ok);
        probe.fail.store(true, Ordering::SeqCst);
        assert!(!reserve(&broker, &qwen, "no-telemetry", 1).ok);
        assert!(broker.lock().held.is_empty());
        Ok(())
    }
    #[test]
    fn released_lease_cannot_be_revived_by_an_older_snapshot() -> Result<()> {
        let (broker, _, qwen, _) = setup()?;
        reserve(&broker, &qwen, "old", 60).check()?;
        let old = broker.lock().held["old"].allocation.clone();
        broker
            .handle(Request::Release {
                owner: qwen.clone(),
                id: "old".into(),
            })
            .check()?;
        broker
            .handle(Request::Sync {
                owner: qwen.clone(),
                allocations: vec![old],
            })
            .check()?;
        assert!(!broker.lock().held.contains_key("old"));
        reserve(&broker, &qwen, "new", 60).check()?;
        Ok(())
    }
    #[test]
    fn another_client_session_cannot_replace_live_worker_state() -> Result<()> {
        let (broker, _, qwen, _) = setup()?;
        reserve(&broker, &qwen, "live", 60).check()?;
        let mut replacement = qwen;
        replacement.session += 1;
        assert!(
            !broker
                .handle(Request::Sync {
                    owner: replacement.clone(),
                    allocations: vec![]
                })
                .ok
        );
        assert!(!reserve(&broker, &replacement, "steal", 1).ok);
        assert!(broker.lock().held.contains_key("live"));
        Ok(())
    }
    #[test]
    fn restart_requires_full_reconciliation_before_new_grants() -> Result<()> {
        let (original, probe, qwen, llama) = setup()?;
        reserve(&original, &qwen, "old-peak", 70).check()?;
        let allocations = original
            .lock()
            .held
            .values()
            .map(|h| h.allocation.clone())
            .collect();
        let restarted = Broker::new(
            probe,
            vec!["qwen".into(), "llama".into()],
            5,
            Duration::ZERO,
        )?;
        restarted
            .handle(Request::Sync {
                owner: qwen.clone(),
                allocations,
            })
            .check()?;
        assert!(!reserve(&restarted, &qwen, "before-reconcile", 1).ok);
        restarted
            .handle(Request::Sync {
                owner: llama.clone(),
                allocations: vec![],
            })
            .check()?;
        assert!(!reserve(&restarted, &llama, "would-overcommit", 30).ok);
        reserve(&restarted, &llama, "fits", 20).check()?;
        Ok(())
    }
    #[test]
    fn dead_process_lease_is_removed_but_delayed_device_free_stays_external() -> Result<()> {
        let (broker, probe, qwen, llama) = setup()?;
        let mut dead = qwen;
        dead.process.start_ticks += 1;
        broker.lock().held.insert(
            "orphan".into(),
            Held {
                owner: dead,
                allocation: allocation("orphan", 80),
            },
        );
        probe.free.store(10, Ordering::SeqCst);
        assert!(!reserve(&broker, &llama, "delayed-free", 20).ok);
        assert!(!broker.lock().held.contains_key("orphan"));
        probe.free.store(100, Ordering::SeqCst);
        reserve(&broker, &llama, "confirmed-free", 20).check()?;
        Ok(())
    }
    #[test]
    fn competing_load_waits_for_real_release_and_driver_confirmation() -> Result<()> {
        let (broker, probe, qwen, llama) = setup()?;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("cache.sock");
        let listener = UnixListener::bind(&path)?;
        let mut base = allocation("base", 10);
        base.resident_floor = 10;
        let mut cache = allocation("cache", 50);
        cache.resident_floor = 50;
        cache.cache = true;
        cache.control = Some(path.to_string_lossy().into());
        broker
            .handle(Request::Sync {
                owner: qwen.clone(),
                allocations: vec![base, cache],
            })
            .check()?;
        probe.free.store(40, Ordering::SeqCst);
        probe.used.store(60, Ordering::SeqCst);
        let (entered, entry) = mpsc::channel();
        let (acknowledge, ack) = mpsc::channel();
        let control = std::thread::spawn(move || -> Result<()> {
            let (mut stream, _) = listener.accept()?;
            assert!(matches!(
                read::<Request>(&mut stream)?,
                Request::Reclaim { .. }
            ));
            entered
                .send(())
                .map_err(|e| Error::Invalid(e.to_string()))?;
            ack.recv_timeout(Duration::from_secs(2))
                .map_err(|e| Error::Invalid(e.to_string()))?;
            write(&mut stream, &Response::success(0, "test"))
        });
        let (done, completion) = mpsc::channel();
        let waiting = Arc::clone(&broker);
        let request_owner = llama.clone();
        let waiter = std::thread::spawn(move || {
            let response = waiting.handle(Request::Reserve {
                owner: request_owner,
                allocation: allocation("embedding", 50),
                priority: 50,
                wait_ms: 1500,
            });
            let _ = done.send(response);
        });
        entry
            .recv_timeout(Duration::from_secs(2))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        assert!(completion.try_recv().is_err());
        // The acknowledgement alone must not grant capacity while the device
        // still reports the old usage.
        acknowledge
            .send(())
            .map_err(|e| Error::Invalid(e.to_string()))?;
        control
            .join()
            .map_err(|_| Error::Invalid("control thread panicked".into()))??;
        std::thread::sleep(Duration::from_millis(150));
        assert!(completion.try_recv().is_err());
        probe.free.store(90, Ordering::SeqCst);
        probe.used.store(10, Ordering::SeqCst);
        completion
            .recv_timeout(Duration::from_secs(2))
            .map_err(|e| Error::Invalid(e.to_string()))?
            .check()?;
        waiter
            .join()
            .map_err(|_| Error::Invalid("waiter panicked".into()))?;
        assert!(
            !broker
                .handle(Request::ResizeCache {
                    owner: qwen,
                    id: "cache".into(),
                    bytes: 1
                })
                .ok,
            "refill must respect cooldown"
        );
        Ok(())
    }
    #[test]
    fn priority_orders_queued_requests_and_release_wakes_waiters() -> Result<()> {
        let (broker, _, qwen, llama) = setup()?;
        reserve(&broker, &qwen, "blocker", 90).check()?;
        let (sent, received) = mpsc::channel();
        let mut threads = Vec::new();
        for (id, priority) in [("low", 1), ("high", 100)] {
            let broker = Arc::clone(&broker);
            let owner = llama.clone();
            let sent = sent.clone();
            threads.push(std::thread::spawn(move || {
                let result = broker.handle(Request::Reserve {
                    owner,
                    allocation: allocation(id, 60),
                    priority,
                    wait_ms: 1500,
                });
                let _ = sent.send((id, result));
            }));
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while broker.lock().pending.len() < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(broker.lock().pending.len(), 2);
        broker
            .handle(Request::Release {
                owner: qwen,
                id: "blocker".into(),
            })
            .check()?;
        let (first, response) = received
            .recv_timeout(Duration::from_secs(2))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        assert_eq!(first, "high");
        response.check()?;
        broker
            .handle(Request::Release {
                owner: llama,
                id: "high".into(),
            })
            .check()?;
        let (second, response) = received
            .recv_timeout(Duration::from_secs(2))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        assert_eq!(second, "low");
        response.check()?;
        for thread in threads {
            thread
                .join()
                .map_err(|_| Error::Invalid("queue thread panicked".into()))?;
        }
        Ok(())
    }
}
