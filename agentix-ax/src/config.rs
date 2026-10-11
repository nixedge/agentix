use crate::tools::access::FileAccess;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchProfile {
    schema_version: u32,
    root: PathBuf,
    read_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
}
pub fn file_access(cwd: PathBuf) -> Result<FileAccess> {
    let marker = std::fs::read("/run/agentix/ax-profile.json")
        .context("coding tools require ax-jail; launch ax-jail instead of running ax directly")?;
    if marker.len() > 65536 {
        bail!("launcher profile exceeds limit");
    }
    let profile: LaunchProfile = serde_json::from_slice(&marker)?;
    let cwd = cwd.canonicalize()?;
    if profile.schema_version != 1 || !cwd.starts_with(&profile.root) {
        bail!("working directory is outside the launcher project");
    }
    FileAccess::new(cwd, profile.read_roots, profile.write_roots)
}
