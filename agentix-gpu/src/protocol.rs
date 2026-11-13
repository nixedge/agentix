use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

pub const MAX_MESSAGE: u64 = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GPU coordinator unavailable: {0}")]
    Io(#[from] std::io::Error),
    #[error("GPU protocol: {0}")]
    Json(#[from] serde_json::Error),
    #[error("GPU capacity: {0}")]
    Capacity(String),
    #[error("GPU management: {0}")]
    Invalid(String),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub start_ticks: u64,
}
impl Process {
    pub fn current() -> Result<Self> {
        let pid = std::process::id();
        Ok(Self {
            pid,
            start_ticks: Self::start(pid)?,
        })
    }
    fn start(pid: u32) -> Result<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        stat.rsplit_once(") ")
            .and_then(|(_, fields)| fields.split_whitespace().nth(19))
            .and_then(|ticks| ticks.parse().ok())
            .ok_or_else(|| Error::Invalid("invalid process incarnation".into()))
    }
    pub fn alive(&self) -> bool {
        match Self::start(self.pid) {
            Ok(ticks) => ticks == self.start_ticks,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => false,
            // An unreadable proc entry does not prove a worker died.
            Err(_) => true,
        }
    }
    pub fn verified_alive(&self) -> bool {
        Self::start(self.pid).is_ok_and(|ticks| ticks == self.start_ticks)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub role: String,
    pub process: Process,
    pub session: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Allocation {
    pub id: String,
    pub model: String,
    pub device: u32,
    pub bytes: u64,
    #[serde(default)]
    pub resident_floor: u64,
    pub cache: bool,
    pub control: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Sync {
        owner: Owner,
        allocations: Vec<Allocation>,
    },
    Reserve {
        owner: Owner,
        allocation: Allocation,
        priority: u32,
        wait_ms: u64,
    },
    Release {
        owner: Owner,
        id: String,
    },
    ResizeCache {
        owner: Owner,
        id: String,
        bytes: u64,
    },
    Reclaim {
        id: String,
    },
    Status,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub error: Option<String>,
    pub bytes: u64,
    pub epoch: String,
}
impl Response {
    pub fn success(bytes: u64, epoch: &str) -> Self {
        Self {
            ok: true,
            bytes,
            epoch: epoch.into(),
            error: None,
        }
    }
    pub fn failure(error: impl ToString, epoch: &str) -> Self {
        Self {
            ok: false,
            error: Some(error.to_string()),
            epoch: epoch.into(),
            bytes: 0,
        }
    }
    pub fn check(self) -> Result<Self> {
        if self.ok {
            Ok(self)
        } else {
            Err(Error::Capacity(
                self.error.unwrap_or_else(|| "request refused".into()),
            ))
        }
    }
}

pub fn read<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut data = Vec::new();
    BufReader::new(stream.take(MAX_MESSAGE + 1)).read_until(b'\n', &mut data)?;
    if data.len() as u64 > MAX_MESSAGE || data.last() != Some(&b'\n') {
        return Err(Error::Invalid(
            "oversized or incomplete management message".into(),
        ));
    }
    Ok(serde_json::from_slice(&data)?)
}
pub fn write<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<()> {
    let mut data = serde_json::to_vec(value)?;
    if data.len() as u64 >= MAX_MESSAGE {
        return Err(Error::Invalid("oversized management message".into()));
    }
    data.push(b'\n');
    stream.write_all(&data)?;
    Ok(())
}
pub fn call(path: &Path, request: &Request, timeout: Duration) -> Result<Response> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    write(&mut stream, request)?;
    read::<Response>(&mut stream)?.check()
}
