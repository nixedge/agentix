use crate::tools::access::FileAccess;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize, Serialize)]
struct ModelPreference {
    schema_version: u32,
    gateway: String,
    model: String,
}

fn preference_path(home: &std::path::Path, gateway: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(gateway.trim_end_matches('/').as_bytes());
    home.join(".ax").join(format!("model-{digest:x}.json"))
}

pub fn load_selected_model(gateway: &str) -> Result<Option<String>> {
    let home = std::env::var_os("HOME").context("HOME not set")?;
    load_model_at(std::path::Path::new(&home), gateway)
}

fn load_model_at(home: &std::path::Path, gateway: &str) -> Result<Option<String>> {
    use std::io::Read;
    let file = match std::fs::File::open(preference_path(home, gateway)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut data = Vec::new();
    file.take(16385).read_to_end(&mut data)?;
    anyhow::ensure!(data.len() <= 16384, "model preference exceeds limit");
    let preference: ModelPreference = serde_json::from_slice(&data)?;
    anyhow::ensure!(
        preference.schema_version == 1 && preference.gateway == gateway.trim_end_matches('/'),
        "invalid model preference"
    );
    anyhow::ensure!(
        !preference.model.is_empty()
            && preference.model.len() <= 4096
            && !preference.model.chars().any(char::is_control),
        "invalid saved model ID"
    );
    Ok(Some(preference.model))
}

pub fn save_selected_model(gateway: &str, model: &str) -> Result<()> {
    let home = std::env::var_os("HOME").context("HOME not set")?;
    save_model_at(std::path::Path::new(&home), gateway, model)
}

fn save_model_at(home: &std::path::Path, gateway: &str, model: &str) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    anyhow::ensure!(
        !model.is_empty() && model.len() <= 4096 && !model.chars().any(char::is_control),
        "invalid model ID"
    );
    let path = preference_path(home, gateway);
    let parent = path.parent().context("model preference directory")?;
    std::fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec(&ModelPreference {
        schema_version: 1,
        gateway: gateway.trim_end_matches('/').into(),
        model: model.into(),
    })?;
    anyhow::ensure!(bytes.len() <= 16384, "model preference exceeds limit");
    let temporary = parent.join(format!(".model-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
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

#[cfg(test)]
mod preference_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn persists_per_gateway_and_replaces_atomically() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let gateway = "http://ai.lan.disasm.us:11434";
        assert!(load_model_at(dir.path(), gateway)?.is_none());
        save_model_at(dir.path(), gateway, "first")?;
        save_model_at(dir.path(), &format!("{gateway}/"), "second")?;
        assert_eq!(
            load_model_at(dir.path(), gateway)?.as_deref(),
            Some("second")
        );
        assert!(load_model_at(dir.path(), "http://another-server")?.is_none());
        assert_eq!(
            std::fs::metadata(preference_path(dir.path(), gateway))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(save_model_at(dir.path(), gateway, "").is_err());
        assert_eq!(
            load_model_at(dir.path(), gateway)?.as_deref(),
            Some("second")
        );
        Ok(())
    }
    #[test]
    fn malformed_and_oversized_preferences_are_rejected() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let gateway = "http://localhost";
        save_model_at(dir.path(), gateway, "valid")?;
        let path = preference_path(dir.path(), gateway);
        std::fs::write(&path, "invalid json")?;
        assert!(load_model_at(dir.path(), gateway).is_err());
        std::fs::write(&path, vec![b' '; 16385])?;
        assert!(load_model_at(dir.path(), gateway).is_err());
        save_model_at(dir.path(), gateway, "recovered")?;
        assert_eq!(
            load_model_at(dir.path(), gateway)?.as_deref(),
            Some("recovered")
        );
        Ok(())
    }
}
