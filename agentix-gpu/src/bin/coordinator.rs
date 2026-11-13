use agentix_gpu::{
    broker::{Broker, NvidiaProbe},
    protocol::{self, Request, Response},
    Result,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

fn serve_connection(mut stream: UnixStream, broker: &Broker) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = protocol::read::<Request>(&mut stream)?;
    let granted = match &request {
        Request::Reserve {
            owner, allocation, ..
        } => Some((owner.clone(), allocation.id.clone())),
        _ => None,
    };
    let response = broker.handle(request);
    if protocol::write::<Response>(&mut stream, &response).is_err() && response.ok {
        // A grant that could not be delivered cannot have started allocation.
        if let Some((owner, id)) = granted {
            broker.handle(Request::Release { owner, id });
        }
    }
    Ok(())
}
fn main() -> Result<()> {
    let socket = PathBuf::from(
        std::env::var_os("AGENTIX_GPU_COORDINATOR")
            .unwrap_or_else(|| "/run/agentix/gpu.sock".into()),
    );
    let roles = std::env::var("AGENTIX_GPU_WORKERS")
        .unwrap_or_else(|_| "qwen,llama,whisper".into())
        .split(',')
        .map(str::to_owned)
        .collect();
    let safety = std::env::var("AGENTIX_GPU_SAFETY_BYTES")
        .unwrap_or_else(|_| "536870912".into())
        .parse()
        .map_err(|_| agentix_gpu::Error::Invalid("invalid GPU safety reserve".into()))?;
    let broker = Arc::new(Broker::new(
        Arc::new(NvidiaProbe),
        roles,
        safety,
        Duration::from_secs(30),
    )?);
    // RuntimeDirectory cleanup belongs to systemd. Never unlink an existing
    // listener here: a second broker must fail rather than split admission.
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660))?;
    let metrics_address =
        std::env::var("AGENTIX_GPU_METRICS_ADDRESS").unwrap_or_else(|_| "127.0.0.1:9488".into());
    let metrics_listener = TcpListener::bind(metrics_address)?;
    let metrics_broker = Arc::clone(&broker);
    std::thread::Builder::new().name("gpu-metrics".into()).spawn(move || {
        for mut stream in metrics_listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
            let mut request = [0; 2048];
            if let Ok(size) = stream.read(&mut request) {
                let (status, body) = if request[..size].starts_with(b"GET /metrics ") {
                    ("200 OK", metrics_broker.metrics())
                } else { ("404 Not Found", "not found\n".into()) };
                let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        }
    })?;
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let stream = stream?;
        if active.fetch_add(1, Ordering::AcqRel) >= 128 {
            active.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let active = Arc::clone(&active);
        let broker = Arc::clone(&broker);
        std::thread::spawn(move || {
            let _ = serve_connection(stream, &broker);
            active.fetch_sub(1, Ordering::AcqRel);
        });
    }
    Ok(())
}
