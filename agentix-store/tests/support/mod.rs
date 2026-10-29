use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};
pub struct Embeddings {
    pub url: String,
    pub requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Embeddings {
    pub fn start() -> Result<Self> {
        Self::start_with_delay(Duration::ZERO)
    }
    pub fn start_with_delay(delay: Duration) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let stopped = stop.clone();
        let count = requests.clone();
        let handle = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        thread::sleep(delay);
                        let _ = respond(&mut stream, &count);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            url: format!("http://{address}"),
            requests,
            stop,
            thread: Some(handle),
        })
    }
}
impl Drop for Embeddings {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}
fn respond(stream: &mut TcpStream, count: &AtomicUsize) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let (body_start, length) = loop {
        let n = stream.read(&mut buffer)?;
        ensure!(n > 0, "closed request");
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end])?;
            let length = headers
                .lines()
                .find_map(|l| {
                    l.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, n)| n.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            break (end + 4, length);
        }
    };
    while bytes.len() < body_start + length {
        let n = stream.read(&mut buffer)?;
        ensure!(n > 0, "closed body");
        bytes.extend_from_slice(&buffer[..n]);
    }
    let request: Value = serde_json::from_slice(&bytes[body_start..body_start + length])?;
    let inputs = request["input"].as_array().context("missing inputs")?;
    count.fetch_add(
        inputs
            .iter()
            .filter(|s| s.as_str().is_some_and(|s| !s.is_empty()))
            .count(),
        Ordering::Relaxed,
    );
    let response =
        json!({"embeddings":inputs.iter().map(|_|vec![1.0,0.2,0.3,0.4]).collect::<Vec<_>>()})
            .to_string();
    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response)?;
    Ok(())
}
