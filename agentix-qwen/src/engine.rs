use crate::tokenizer::{greedy_next_token, GreedyGeneration, GreedyStopReason, PreparedPrompt};
use crate::{Error, Result};
use rusting_strata::{
    backend,
    gguf::Gguf,
    gpu_model::GpuModel,
    model::{Model, State},
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{mpsc, Arc, Mutex, MutexGuard},
    thread,
};

#[derive(Clone, Debug)]
pub struct Options {
    pub backend: String,
    pub device: usize,
    pub context: usize,
    pub chunk: usize,
    pub vram_margin: usize,
    pub expert_budget: Option<usize>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            backend: "cuda".into(),
            device: 0,
            context: 4096,
            chunk: 256,
            vram_margin: 512 * 1024 * 1024,
            expert_budget: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub architecture: String,
    pub vocabulary: usize,
    pub trained_context: usize,
    pub backend: String,
    pub device: String,
}

type Cancellation = Arc<dyn Fn() -> bool + Send + Sync>;
enum Command {
    Reset,
    Prefill(Vec<u32>, Cancellation),
    Step(u32),
}
enum Request {
    Infer {
        command: Command,
        reply: mpsc::SyncSender<Result<Vec<f32>>>,
    },
    ExpertBudget {
        bytes: usize,
        reply: mpsc::SyncSender<Result<usize>>,
    },
    Memory {
        reply: mpsc::SyncSender<Result<MemoryInfo>>,
    },
    Shutdown,
}
struct Client {
    sender: mpsc::Sender<Request>,
}

/// The thread owns the mmap, borrowed model and GPU in stack order for its entire life.
/// Channels move only owned tokens/logits; no self-referencing structs or leaked weights.
pub struct Engine {
    client: Mutex<Option<Client>>,
    thread: Option<thread::JoinHandle<()>>,
    pub info: ModelInfo,
    options: Options,
    control: Control,
}
#[derive(Clone)]
pub struct Control {
    sender: mpsc::Sender<Request>,
}
#[derive(Debug)]
pub struct MemoryInfo {
    pub expert_bytes: usize,
    pub free_bytes: Option<usize>,
    pub total_bytes: Option<usize>,
}
impl Control {
    pub fn memory(&self) -> Result<MemoryInfo> {
        let (reply, result) = mpsc::sync_channel(1);
        self.sender
            .send(Request::Memory { reply })
            .map_err(|_| Error::Backend("inference thread stopped".into()))?;
        result
            .recv()
            .map_err(|_| Error::Backend("memory observation failed".into()))?
    }
    pub fn set_expert_budget(&self, bytes: usize) -> Result<usize> {
        let (reply, result) = mpsc::sync_channel(1);
        self.sender
            .send(Request::ExpertBudget { bytes, reply })
            .map_err(|_| Error::Backend("inference thread stopped".into()))?;
        result
            .recv()
            .map_err(|_| Error::Backend("expert budget command failed".into()))?
    }
}
pub struct ManagedBudget {
    pub peak: agentix_gpu::Lease,
    pub cache: Arc<agentix_gpu::Lease>,
}
impl Drop for ManagedBudget {
    fn drop(&mut self) {
        // run() drops GPU/model locals before its budget parameter. Management
        // observers may retain Arcs, but must not retain this dead engine's lease.
        self.cache.release_after_free();
    }
}
impl Engine {
    pub fn open(path: PathBuf, options: Options) -> Result<Self> {
        Self::open_managed(path, options, None)
    }
    pub fn open_managed(
        path: PathBuf,
        options: Options,
        budget: Option<ManagedBudget>,
    ) -> Result<Self> {
        if options.context == 0 || options.chunk == 0 {
            return Err(Error::Invalid("context and prefill chunk must be positive"));
        }
        if !matches!(options.backend.as_str(), "cpu" | "cuda" | "vulkan") {
            return Err(Error::Invalid(
                "compute backend must be cpu, cuda or vulkan",
            ));
        }
        let (sender, receiver) = mpsc::channel();
        let (ready, initialization) = mpsc::sync_channel(1);
        let worker_options = options.clone();
        let handle = thread::Builder::new()
            .name("rustingstrata".into())
            .spawn(move || {
                let mut ready = Some(ready);
                if let Err(error) = run(path, worker_options, receiver, &mut ready, budget) {
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Err(Error::Backend(format!("{error:#}"))));
                    }
                }
            })?;
        let info = match initialization.recv() {
            Ok(Ok(info)) => info,
            result => {
                drop(sender);
                let _ = handle.join();
                return Err(match result {
                    Ok(Err(error)) => error,
                    _ => Error::Backend("inference thread stopped during initialization".into()),
                });
            }
        };
        Ok(Self {
            control: Control {
                sender: sender.clone(),
            },
            client: Mutex::new(Some(Client { sender })),
            thread: Some(handle),
            info,
            options,
        })
    }
    pub fn control(&self) -> Control {
        self.control.clone()
    }

    pub fn session(&self) -> Result<Session<'_>> {
        let client = self
            .client
            .lock()
            .map_err(|_| Error::Backend("inference session lock poisoned".into()))?;
        let mut session = Session {
            client,
            cancellation: Arc::new(|| false),
            position: 0,
            context: self.options.context,
            vocabulary: self.info.vocabulary,
        };
        session.call(Command::Reset)?;
        Ok(session)
    }

    pub fn is_alive(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.control.sender.send(Request::Shutdown);
        // Closing the only sender terminates the command loop before joining.
        match self.client.get_mut() {
            Ok(client) => {
                client.take();
            }
            Err(poisoned) => {
                poisoned.into_inner().take();
            }
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

pub struct Session<'a> {
    client: MutexGuard<'a, Option<Client>>,
    cancellation: Cancellation,
    position: usize,
    context: usize,
    vocabulary: usize,
}
impl Session<'_> {
    pub fn set_cancellation_check(&mut self, check: impl Fn() -> bool + Send + Sync + 'static) {
        self.cancellation = Arc::new(check);
    }
    fn call(&mut self, command: Command) -> Result<Vec<f32>> {
        if (self.cancellation)() {
            return Err(Error::Invalid("generation cancelled"));
        }
        let (reply, result) = mpsc::sync_channel(1);
        self.client
            .as_ref()
            .ok_or_else(|| Error::Backend("inference engine closed".into()))?
            .sender
            .send(Request::Infer { command, reply })
            .map_err(|_| Error::Backend("inference thread stopped".into()))?;
        result
            .recv()
            .map_err(|_| Error::Backend("inference thread failed".into()))?
    }
    fn admit(&self, tokens: &[u32]) -> Result<()> {
        if tokens.is_empty() {
            return Err(Error::Invalid("empty prompt"));
        }
        if tokens.iter().any(|&id| id as usize >= self.vocabulary) {
            return Err(Error::Invalid("token exceeds model vocabulary"));
        }
        if self
            .position
            .checked_add(tokens.len())
            .ok_or(Error::Overflow)?
            > self.context
        {
            return Err(Error::Capacity("context window"));
        }
        Ok(())
    }
    pub fn prefill(&mut self, tokens: &[u32]) -> Result<Vec<f32>> {
        self.admit(tokens)?;
        let logits = self.call(Command::Prefill(
            tokens.to_vec(),
            Arc::clone(&self.cancellation),
        ))?;
        self.position += tokens.len();
        Ok(logits)
    }
    pub fn step(&mut self, token: u32) -> Result<Vec<f32>> {
        self.admit(&[token])?;
        let logits = self.call(Command::Step(token))?;
        self.position += 1;
        Ok(logits)
    }
    pub fn generate_prepared_greedy_with_progress(
        &mut self,
        prepared: &PreparedPrompt,
        stop_ids: &[u32],
        mut progress: impl FnMut(usize),
    ) -> Result<GreedyGeneration> {
        crate::budget::admit_context(
            prepared.token_ids.len(),
            prepared.admission.requested_output_tokens,
            self.context,
        )?;
        let mut logits = self.prefill(&prepared.token_ids)?;
        progress(self.position);
        let mut generated = Vec::new();
        let mut stop_reason = GreedyStopReason::TokenLimit;
        for index in 0..prepared.admission.requested_output_tokens {
            if (self.cancellation)() {
                return Err(Error::Invalid("generation cancelled"));
            }
            let token = greedy_next_token(&logits)?;
            generated.push(token);
            if stop_ids.contains(&token) {
                stop_reason = GreedyStopReason::StopToken(token);
                break;
            }
            if index + 1 < prepared.admission.requested_output_tokens {
                logits = self.step(token)?;
                progress(self.position);
            }
        }
        Ok(GreedyGeneration {
            generated_token_ids: generated,
            stop_reason,
        })
    }
}

fn run(
    path: PathBuf,
    options: Options,
    receiver: mpsc::Receiver<Request>,
    ready: &mut Option<mpsc::SyncSender<Result<ModelInfo>>>,
    budget: Option<ManagedBudget>,
) -> anyhow::Result<()> {
    let gguf = Gguf::open(&path)?;
    let model = Model::load(&gguf)?;
    let trained_context = gguf
        .meta
        .get(&format!("{}.context_length", model.cfg.arch))
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| anyhow::anyhow!("missing or invalid model context_length"))?;
    anyhow::ensure!(
        options.context <= trained_context,
        "requested context {} exceeds trained context {}",
        options.context,
        trained_context
    );
    let mut state = State::new(&model.cfg);
    let mut device = "CPU reference".to_owned();
    let mut gpu = if options.backend == "cpu" {
        None
    } else {
        let backend = backend::open(Some(&options.backend), options.device)?;
        device = backend.device();
        Some(match options.expert_budget {
            Some(bytes) => GpuModel::load_budgeted(
                &model,
                backend,
                options.context,
                options.chunk,
                options.vram_margin,
                bytes,
            )?,
            None => GpuModel::load(
                &model,
                backend,
                options.context,
                options.chunk,
                options.vram_margin,
                0,
            )?,
        })
    };
    if let Some(budget) = &budget {
        budget
            .cache
            .resize_cache(gpu.as_ref().map_or(0, |g| g.expert_cache_bytes()) as u64)?;
        budget.cache.activate_floor(budget.cache.bytes())?;
        budget.peak.activate_plan()?;
    }
    let info = ModelInfo {
        architecture: model.cfg.arch.clone(),
        vocabulary: model.cfg.n_vocab,
        trained_context,
        backend: options.backend.clone(),
        device,
    };
    if let Some(ready) = ready.take() {
        let _ = ready.send(Ok(info));
    }
    let mut pending = VecDeque::new();
    loop {
        let request = match pending
            .pop_front()
            .map(Ok)
            .unwrap_or_else(|| receiver.recv())
        {
            Ok(request) => request,
            Err(_) => break,
        };
        let Some(request) = management(request, gpu.as_mut(), budget.as_ref()) else {
            continue;
        };
        let (command, reply) = match request {
            Request::Shutdown => break,
            Request::Infer { command, reply } => (command, reply),
            _ => continue,
        };
        let result = (|| -> anyhow::Result<Vec<f32>> {
            let logits = match command {
                Command::Reset => {
                    if let Some(gpu) = gpu.as_mut() {
                        gpu.reset()?;
                    } else {
                        state = State::new(&model.cfg);
                    }
                    return Ok(Vec::new());
                }
                Command::Step(token) => {
                    if let Some(gpu) = gpu.as_mut() {
                        gpu.step(token)?
                    } else {
                        model.step(&mut state, token)
                    }
                }
                Command::Prefill(tokens, cancelled) => {
                    let mut logits = Vec::new();
                    // Bound cancellation latency even when upstream's short-prompt path has no callback.
                    for chunk in tokens.chunks(options.chunk) {
                        drain_management(&receiver, &mut pending, gpu.as_mut(), budget.as_ref());
                        anyhow::ensure!(!cancelled(), "generation cancelled");
                        if let Some(gpu) = gpu.as_mut() {
                            logits = gpu.prefill_with(chunk, &mut |part| {
                                drain_management(
                                    &receiver,
                                    &mut pending,
                                    Some(part),
                                    budget.as_ref(),
                                );
                                anyhow::ensure!(!cancelled(), "generation cancelled");
                                Ok(())
                            })?;
                        } else {
                            for &token in chunk {
                                anyhow::ensure!(!cancelled(), "generation cancelled");
                                logits = model.step(&mut state, token);
                            }
                        }
                    }
                    logits
                }
            };
            anyhow::ensure!(
                logits.len() == model.cfg.n_vocab && logits.iter().all(|v| v.is_finite()),
                "invalid model logits"
            );
            Ok(logits)
        })()
        .map_err(|error| Error::Backend(format!("{error:#}")));
        let _ = reply.send(result);
    }
    Ok(())
}

fn drain_management(
    receiver: &mpsc::Receiver<Request>,
    pending: &mut VecDeque<Request>,
    mut gpu: Option<&mut GpuModel<'_>>,
    budget: Option<&ManagedBudget>,
) {
    while let Ok(request) = receiver.try_recv() {
        if let Some(request) = management(request, gpu.as_deref_mut(), budget) {
            pending.push_back(request);
        }
    }
}

fn management(
    request: Request,
    gpu: Option<&mut GpuModel<'_>>,
    budget: Option<&ManagedBudget>,
) -> Option<Request> {
    match request {
        Request::Memory { reply } => {
            let result = match gpu {
                Some(gpu) => gpu
                    .device_memory()
                    .map(|(free, total)| MemoryInfo {
                        expert_bytes: gpu.expert_cache_bytes(),
                        free_bytes: Some(free),
                        total_bytes: Some(total),
                    })
                    .map_err(|e| Error::Backend(format!("{e:#}"))),
                None => Ok(MemoryInfo {
                    expert_bytes: 0,
                    free_bytes: None,
                    total_bytes: None,
                }),
            };
            let _ = reply.send(result);
            None
        }
        Request::ExpertBudget { bytes, reply } => {
            let result = (|| -> anyhow::Result<usize> {
                if let Some(budget) = budget {
                    anyhow::ensure!(
                        bytes as u64 <= budget.cache.bytes(),
                        "expert refill exceeds its lease"
                    );
                    budget.cache.activate_floor(0)?;
                }
                let remaining = match gpu {
                    Some(gpu) => gpu.set_expert_budget(bytes)?,
                    None => 0,
                };
                if let Some(budget) = budget {
                    budget.cache.resize_cache(remaining as u64)?;
                    budget.cache.activate_floor(remaining as u64)?;
                }
                Ok(remaining)
            })()
            .map_err(|e| Error::Backend(format!("{e:#}")));
            let _ = reply.send(result);
            None
        }
        request => Some(request),
    }
}
