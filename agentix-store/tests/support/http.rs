use agentix_store::Store;
use anyhow::{Context, Result};

pub const TOKEN: &str = "fixture-storage-token-at-least-32-characters";
pub struct StorageServer {
    pub url: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl StorageServer {
    pub fn start(store: Store, token: Option<String>) -> Result<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}", listener.local_addr()?);
        let router = agentix_store::server::router(store, token)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            runtime
                .block_on(async move {
                    let listener =
                        tokio::net::TcpListener::from_std(listener).context("fixture listener")?;
                    axum::serve(listener, router)
                        .with_graceful_shutdown(async {
                            let _ = stopped.await;
                        })
                        .await?;
                    anyhow::Ok(())
                })
                .unwrap_or_else(|e| panic!("storage HTTP fixture failed: {e}"));
        });
        Ok(Self {
            url,
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}
impl Drop for StorageServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
