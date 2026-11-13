//! Whisper backend for in-process speech-to-text transcription.
//!
//! `WhisperContext` is `Send + Sync` (upstream `unsafe impl` in whisper-rs).
//! A persistent native worker owns transcription state and parameters. Its cleanup
//! is joined outside async worker threads before the Tokio runtime shuts down.

pub mod audio;

pub use audio::decode_audio_to_pcm;

use agentix_infer::{
    backend::{CompletionStream, InferBackend, LoadedModel},
    Capability, CompletionRequest, InferError, ModelFormat, ModelInfo,
};
use std::{path::Path, sync::Arc};
use tokio::sync::oneshot;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

// ── Backend ──────────────────────────────────────────────────────────────────

pub struct WhisperBackend;

#[async_trait::async_trait]
impl InferBackend for WhisperBackend {
    fn name(&self) -> &'static str {
        "whisper"
    }

    fn supports_format(&self, format: ModelFormat) -> bool {
        matches!(format, ModelFormat::Gguf | ModelFormat::WhisperBin)
    }

    async fn load(
        &self,
        blob_path: &Path,
        info: &ModelInfo,
    ) -> Result<Arc<dyn LoadedModel>, InferError> {
        self.load_reserved(
            blob_path,
            info,
            agentix_infer::memory::LoadReservation::unmanaged(),
        )
        .await
    }

    fn uses_gpu(&self) -> bool {
        WhisperContextParameters::default().use_gpu
    }

    async fn load_reserved(
        &self,
        blob_path: &Path,
        info: &ModelInfo,
        reservation: agentix_infer::memory::LoadReservation,
    ) -> Result<Arc<dyn LoadedModel>, InferError> {
        let path = blob_path.to_owned();
        let size_bytes = if reservation.peak_bytes == 0 {
            info.size_bytes
        } else {
            reservation.peak_bytes
        };

        tokio::task::spawn_blocking(move || {
            let path_str = path
                .to_str()
                .ok_or_else(|| InferError::Backend("non-UTF-8 model path".to_string()))?;

            tracing::info!(path = %path_str, size_bytes, "loading whisper model");

            let mut parameters = WhisperContextParameters::default();
            if let Some(shared) = &reservation.shared {
                parameters.gpu_device = i32::try_from(shared.device())
                    .map_err(|_| InferError::Backend("GPU device index overflow".into()))?;
            }
            let ctx = WhisperContext::new_with_params(path_str, parameters)
                .map_err(|e| InferError::Backend(format!("whisper context init failed: {e:?}")))?;

            let worker = WhisperWorker::new(ctx, reservation)?;

            Ok(Arc::new(WhisperLoadedModel { worker, size_bytes }) as Arc<dyn LoadedModel>)
        })
        .await
        .map_err(|e| InferError::Backend(format!("spawn_blocking join error: {e}")))?
    }
}

// ── Worker thread ────────────────────────────────────────────────────────────

struct WorkItem {
    pcm: Vec<f32>,
    reply: oneshot::Sender<Result<String, InferError>>,
}

/// Owns a `WhisperState` on a dedicated OS thread so state is created once and
/// reused across requests, avoiding the per-request GPU allocation overhead.
struct WhisperWorker {
    tx: Option<std::sync::mpsc::SyncSender<WorkItem>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WhisperWorker {
    fn new(
        ctx: WhisperContext,
        reservation: agentix_infer::memory::LoadReservation,
    ) -> Result<Self, InferError> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<WorkItem>(4);
        let (ready, initialized) = std::sync::mpsc::sync_channel(1);
        let owner = WhisperOwner { ctx, reservation };

        let thread = std::thread::Builder::new()
            .name("whisper-worker".into())
            .spawn(move || owner.run(rx, ready))
            .map_err(|e| InferError::Backend(format!("whisper thread spawn failed: {e}")))?;

        let worker = Self {
            tx: Some(tx),
            thread: Some(thread),
        };
        initialized
            .recv()
            .map_err(|_| InferError::Backend("whisper initialization stopped".into()))??;
        Ok(worker)
    }

    async fn transcribe(&self, pcm: Vec<f32>) -> Result<String, InferError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let sender = self
            .tx
            .as_ref()
            .ok_or_else(|| InferError::Backend("whisper worker is shutting down".into()))?
            .clone();
        let item = WorkItem {
            pcm,
            reply: reply_tx,
        };
        tokio::task::spawn_blocking(move || sender.send(item))
            .await
            .map_err(|e| InferError::Backend(e.to_string()))?
            .map_err(|_| InferError::Backend("whisper worker channel closed".into()))?;
        reply_rx
            .await
            .map_err(|_| InferError::Backend("whisper worker dropped".into()))?
    }
}

impl Drop for WhisperWorker {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(thread) = self.thread.take() {
            let join = move || {
                if thread.join().is_err() {
                    tracing::error!("whisper worker panicked during cleanup");
                }
            };
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                // Runtime shutdown waits for blocking tasks, keeping CUDA alive
                // until the native thread has destroyed its buffers and context.
                runtime.spawn_blocking(join);
            } else {
                join();
            }
        }
    }
}

struct WhisperOwner {
    // Keep native ownership ahead of its lease on a failed thread spawn too.
    ctx: WhisperContext,
    reservation: agentix_infer::memory::LoadReservation,
}
impl WhisperOwner {
    fn run(
        self,
        receiver: std::sync::mpsc::Receiver<WorkItem>,
        ready: std::sync::mpsc::SyncSender<Result<(), InferError>>,
    ) {
        let allocation = self.reservation;
        let ctx = self.ctx;
        let mut state = match ctx.create_state() {
            Ok(state) => state,
            Err(error) => {
                let _ = ready.send(Err(InferError::Backend(format!(
                    "whisper state init failed: {error:?}"
                ))));
                return;
            }
        };
        if let Err(error) = allocation.activate() {
            let _ = ready.send(Err(error));
            return;
        }
        if ready.send(Ok(())).is_err() {
            return;
        }
        while let Ok(work) = receiver.recv() {
            let result = run_transcription(&mut state, &work.pcm);
            let _ = work.reply.send(result);
        }
        drop(state);
        drop(ctx);
        drop(allocation);
    }
}

fn run_transcription(
    state: &mut whisper_rs::WhisperState,
    pcm: &[f32],
) -> Result<String, InferError> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_print_special(false);
    params.set_single_segment(false);

    state
        .full(params, pcm)
        .map_err(|e| InferError::Transcription(format!("whisper full() failed: {e:?}")))?;

    let n = state
        .full_n_segments()
        .map_err(|e| InferError::Transcription(format!("full_n_segments failed: {e:?}")))?;

    let mut text = String::new();
    for i in 0..n {
        let seg = state
            .full_get_segment_text(i)
            .map_err(|e| InferError::Transcription(format!("segment {i} text failed: {e:?}")))?;
        text.push_str(&seg);
    }

    Ok(text.trim().to_string())
}

// ── Loaded model ─────────────────────────────────────────────────────────────

pub struct WhisperLoadedModel {
    worker: WhisperWorker,
    size_bytes: u64,
}

#[async_trait::async_trait]
impl LoadedModel for WhisperLoadedModel {
    fn vram_bytes(&self) -> u64 {
        self.size_bytes
    }

    async fn embed(&self, _input: &str) -> Result<Vec<f32>, InferError> {
        Err(InferError::CapabilityMissing(
            "whisper".to_string(),
            Capability::Embedding,
        ))
    }

    async fn embed_batch(&self, _inputs: &[&str]) -> Result<Vec<Vec<f32>>, InferError> {
        Err(InferError::CapabilityMissing(
            "whisper".to_string(),
            Capability::Embedding,
        ))
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionStream, InferError> {
        Err(InferError::CapabilityMissing(
            "whisper".to_string(),
            Capability::Completion,
        ))
    }

    async fn tokenize(&self, _text: &str) -> Result<Vec<i32>, InferError> {
        Err(InferError::CapabilityMissing(
            "whisper".to_string(),
            Capability::Completion,
        ))
    }

    async fn transcribe(&self, audio_pcm: &[f32]) -> Result<String, InferError> {
        self.worker.transcribe(audio_pcm.to_vec()).await
    }
}

#[cfg(test)]
mod tests {
    use super::{WhisperWorker, WorkItem};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    };
    use std::time::Duration;

    #[test]
    fn worker_drop_joins_cleanup_without_a_runtime() -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = mpsc::sync_channel::<WorkItem>(4);
        let cleaned = Arc::new(AtomicBool::new(false));
        let worker_cleaned = Arc::clone(&cleaned);
        let thread = std::thread::spawn(move || {
            while rx.recv().is_ok() {}
            worker_cleaned.store(true, Ordering::SeqCst);
        });
        drop(WhisperWorker {
            tx: Some(tx),
            thread: Some(thread),
        });
        assert!(cleaned.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn runtime_waits_for_worker_cleanup_without_blocking_async_tasks(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (tx, rx) = mpsc::sync_channel::<WorkItem>(4);
        let (cleanup_started, cleanup_started_rx) = tokio::sync::oneshot::channel();
        let (finish_cleanup, finish_cleanup_rx) = mpsc::channel();
        let (begin_shutdown, begin_shutdown_rx) = mpsc::channel();
        let cleaned = Arc::new(AtomicBool::new(false));
        let worker_cleaned = Arc::clone(&cleaned);
        let thread = std::thread::spawn(move || {
            while rx.recv().is_ok() {}
            let _ = cleanup_started.send(());
            let _ = finish_cleanup_rx.recv_timeout(Duration::from_secs(2));
            worker_cleaned.store(true, Ordering::SeqCst);
        });
        let release = std::thread::spawn(move || {
            if begin_shutdown_rx.recv().is_ok() {
                // Keep cleanup pending as runtime shutdown begins, so merely
                // detaching the native worker cannot satisfy the assertion.
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = finish_cleanup.send(());
        });
        runtime.block_on(async {
            drop(WhisperWorker {
                tx: Some(tx),
                thread: Some(thread),
            });
            tokio::time::timeout(Duration::from_secs(1), cleanup_started_rx).await??;
            assert!(!cleaned.load(Ordering::SeqCst));
            Ok::<_, Box<dyn std::error::Error>>(())
        })?;
        begin_shutdown.send(())?;
        drop(runtime);
        assert!(cleaned.load(Ordering::SeqCst));
        release
            .join()
            .map_err(|_| "cleanup release thread panicked")?;
        Ok(())
    }
}
