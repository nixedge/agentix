use agentix_gpu::{
    protocol::{self, Request, Response},
    Client, Lease,
};
use agentix_qwen::engine::{Control, ManagedBudget, Options};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub struct Managed {
    client: Client,
    cache: Arc<Lease>,
    path: PathBuf,
    ceiling: u64,
}
impl Managed {
    pub fn prepare(
        client: Option<Client>,
        model: &str,
        hashes: &str,
        options: &Options,
        path: &Path,
    ) -> Result<Option<(Self, ManagedBudget)>, String> {
        let Some(client) = client else {
            return Ok(None);
        };
        Self::validate(Some(&client), model, hashes, options)?;
        let peak = client.reserve_model(model).map_err(|e| e.to_string())?;
        let ceiling = std::env::var("AGENTIX_QWEN_EXPERT_MAX_BYTES")
            .unwrap_or_else(|_| "25769803776".into())
            .parse::<u64>()
            .map_err(|e| e.to_string())?;
        let cache = Arc::new(
            client
                .reserve_cache(model, ceiling, path)
                .map_err(|e| e.to_string())?,
        );
        let managed = Self {
            client,
            cache: Arc::clone(&cache),
            path: path.into(),
            ceiling,
        };
        Ok(Some((managed, ManagedBudget { peak, cache })))
    }
    pub fn validate(
        client: Option<&Client>,
        model: &str,
        hashes: &str,
        options: &Options,
    ) -> Result<(), String> {
        let Some(client) = client else {
            return Ok(());
        };
        if options.backend != "cuda" || options.device as u32 != client.device() {
            return Err(
                "shared NVIDIA memory requires Qwen CUDA on the coordinated physical device".into(),
            );
        }
        client
            .plan(model)
            .and_then(|plan| {
                plan.check_hashes(&hashes.split(':').map(str::to_owned).collect::<Vec<_>>())?;
                plan.check_shape(options.context, options.chunk)
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn bytes(&self) -> usize {
        self.cache.bytes().min(usize::MAX as u64) as usize
    }
    pub fn start(self, control: Control) -> std::io::Result<()> {
        super::prepare_socket_path(&self.path).map_err(|e| std::io::Error::other(e.to_string()))?;
        let listener = tokio::net::UnixListener::bind(&self.path)?;
        let recovery = Arc::new(AtomicBool::new(false));
        let refill_recovery = Arc::clone(&recovery);
        let cache = Arc::clone(&self.cache);
        let refill_control = control.clone();
        let ceiling = self.ceiling;
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(5));
            loop {
                timer.tick().await;
                if cache.is_released() {
                    break;
                }
                let cache = Arc::clone(&cache);
                let control = refill_control.clone();
                let recovery = Arc::clone(&refill_recovery);
                let _ = tokio::task::spawn_blocking(move || {
                    // Retry a failed release before allowing another refill. The
                    // native command serializes cleanup and lease reconciliation.
                    if recovery.swap(false, Ordering::AcqRel) {
                        if control.set_expert_budget(0).is_err() {
                            recovery.store(true, Ordering::Release);
                        }
                        return;
                    }
                    if cache.bytes() != 0 {
                        return;
                    }
                    if cache.resize_cache(ceiling).is_ok() {
                        let bytes = cache.bytes();
                        if bytes > 0 {
                            if let Err(error) = control.set_expert_budget(bytes.min(usize::MAX as u64) as usize) {
                                recovery.store(true, Ordering::Release);
                                tracing::error!(%error, "expert refill stopped; retaining reservation until confirmed release");
                            }
                        }
                    }
                }).await;
            }
        });
        tokio::spawn(async move {
            let slots = Arc::new(tokio::sync::Semaphore::new(4));
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
                    continue;
                };
                let Ok(mut stream) = stream.into_std() else {
                    continue;
                };
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                let control = control.clone();
                let cache = Arc::clone(&self.cache);
                let client = self.client.clone();
                let recovery = Arc::clone(&recovery);
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
                    let result = (|| -> Result<u64, String> {
                        let request: Request =
                            protocol::read(&mut stream).map_err(|e| e.to_string())?;
                        match request {
                            Request::Reclaim { id } if id == cache.id() => {
                                let released = (|| {
                                    let remaining =
                                        control.set_expert_budget(0).map_err(|e| e.to_string())?
                                            as u64;
                                    client
                                        .cache_released(&id, remaining)
                                        .map_err(|e| e.to_string())?;
                                    Ok(remaining)
                                })();
                                if released.is_err() {
                                    recovery.store(true, Ordering::Release);
                                }
                                released
                            }
                            _ => Err("invalid expert reclaim lease".into()),
                        }
                    })();
                    let response = match result {
                        Ok(bytes) => Response::success(bytes, "qwen"),
                        Err(error) => Response::failure(error, "qwen"),
                    };
                    let _ = protocol::write(&mut stream, &response);
                });
            }
        });
        Ok(())
    }
}
