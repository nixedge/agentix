use agentix_gpu::{
    broker::{Broker, Probe},
    policy::Observation,
    protocol::{self, Request},
    Client, ModelPlan, Result,
};
use std::{
    collections::HashMap,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
struct FakeProbe(AtomicU64);
impl Probe for FakeProbe {
    fn observe(&self, device: u32) -> Result<Observation> {
        Ok(Observation {
            device,
            uuid: "test".into(),
            total: 100,
            free: self.0.load(Ordering::SeqCst),
            processes: HashMap::new(),
        })
    }
}
struct Server {
    path: PathBuf,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    fn new(path: &Path, probe: Arc<FakeProbe>, roles: &[&str]) -> Result<Self> {
        let broker = Arc::new(Broker::new(
            probe,
            roles.iter().map(|r| r.to_string()).collect(),
            5,
            Duration::from_secs(30),
        )?);
        let listener = UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stopped);
        let thread = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let broker = Arc::clone(&broker);
                        handlers.push(std::thread::spawn(move || {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            if let Ok(request) = protocol::read::<Request>(&mut stream) {
                                let response = broker.handle(request);
                                let _ = protocol::write(&mut stream, &response);
                            }
                        }));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
            for handler in handlers {
                let _ = handler.join();
            }
        });
        Ok(Self {
            path: path.into(),
            stopped,
            thread: Some(thread),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}
fn plans(bytes: u64) -> HashMap<String, ModelPlan> {
    [(
        "model".into(),
        ModelPlan {
            peak_bytes: bytes,
            model_sha256: vec![],
            resident_floor_bytes: 0,
            context_tokens: 256,
            batch_tokens: 64,
            priority: 50,
        },
    )]
    .into_iter()
    .collect()
}
#[test]
fn client_roundtrip_drop_and_coordinator_restart_restore_live_leases() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("coordinator.sock");
    let probe = Arc::new(FakeProbe(AtomicU64::new(100)));
    let server = Server::new(&socket, Arc::clone(&probe), &["test"])?;
    let client = Client::new(
        socket.clone(),
        "test".into(),
        0,
        plans(60),
        Duration::from_millis(100),
    )?;
    let lease = client.reserve_model("model")?;
    assert_eq!(lease.bytes(), 60);
    assert!(client.reserve_model("model").is_err());
    drop(server);
    let _restarted = Server::new(&socket, probe, &["test"])?;
    client.sync()?;
    assert!(
        client.reserve_model("model").is_err(),
        "restart must retain the original reservation"
    );
    drop(lease);
    let _replacement = client.reserve_model("model")?;
    Ok(())
}
struct ChildOwner(Child);
impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn worker_crash_releases_ownership_only_after_process_death_and_device_confirmation() -> Result<()>
{
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("coordinator.sock");
    let ready = dir.path().join("child-ready");
    let probe = Arc::new(FakeProbe(AtomicU64::new(100)));
    let _server = Server::new(&socket, Arc::clone(&probe), &["parent", "child"])?;
    let client = Client::new(
        socket.clone(),
        "parent".into(),
        0,
        plans(40),
        Duration::from_millis(100),
    )?;
    let mut child = ChildOwner(
        Command::new(std::env::current_exe()?)
            .args(["--ignored", "--exact", "child_client_process"])
            .env("AGENTIX_GPU_IPC_SOCKET", &socket)
            .env("AGENTIX_GPU_IPC_READY", &ready)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(ready.exists(), "child failed to acquire its allocation");
    assert!(client.reserve_model("model").is_err());
    child.0.kill()?;
    child.0.wait()?;
    probe.0.store(20, Ordering::SeqCst);
    assert!(
        client.reserve_model("model").is_err(),
        "dead process does not prove driver memory was freed"
    );
    probe.0.store(100, Ordering::SeqCst);
    let _lease = client.reserve_model("model")?;
    Ok(())
}
#[test]
#[ignore = "subprocess helper, invoked by the parent IPC test"]
fn child_client_process() -> Result<()> {
    let socket = std::env::var_os("AGENTIX_GPU_IPC_SOCKET")
        .ok_or_else(|| agentix_gpu::Error::Invalid("missing child socket".into()))?;
    let ready = std::env::var_os("AGENTIX_GPU_IPC_READY")
        .ok_or_else(|| agentix_gpu::Error::Invalid("missing child marker".into()))?;
    let client = Client::new(
        socket.into(),
        "child".into(),
        0,
        plans(70),
        Duration::from_secs(1),
    )?;
    let _lease = client.reserve_model("model")?;
    std::fs::write(ready, "ready")?;
    std::thread::sleep(Duration::from_secs(5));
    Ok(())
}

#[test]
fn native_release_can_unblock_an_acquisition_in_the_same_client() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("coordinator.sock");
    let _server = Server::new(&socket, Arc::new(FakeProbe(AtomicU64::new(100))), &["test"])?;
    let client = Client::new(socket, "test".into(), 0, plans(60), Duration::from_secs(1))?;
    let lease = client.reserve_model("model")?;
    let waiting = client.clone();
    let (entered, entry) = std::sync::mpsc::channel();
    let next = std::thread::spawn(move || {
        let _ = entered.send(());
        waiting.reserve_model("model")
    });
    entry
        .recv_timeout(Duration::from_secs(1))
        .map_err(|e| agentix_gpu::Error::Invalid(e.to_string()))?;
    std::thread::sleep(Duration::from_millis(50));
    drop(lease);
    let _new = next
        .join()
        .map_err(|_| agentix_gpu::Error::Invalid("acquisition thread panicked".into()))??;
    Ok(())
}

#[test]
fn observers_cannot_keep_a_destroyed_native_cache_lease_alive() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("coordinator.sock");
    let _server = Server::new(&socket, Arc::new(FakeProbe(AtomicU64::new(100))), &["test"])?;
    let client = Client::new(
        socket,
        "test".into(),
        0,
        plans(60),
        Duration::from_millis(100),
    )?;
    let lease =
        Arc::new(client.reserve_cache("model", 60, &dir.path().join("unused-control.sock"))?);
    let observer = Arc::clone(&lease);
    lease.release_after_free();
    assert_eq!(observer.bytes(), 0);
    assert!(observer.resize_cache(60).is_err());
    let _replacement = client.reserve_model("model")?;
    Ok(())
}
