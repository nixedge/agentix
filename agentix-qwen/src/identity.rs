//! Structured identity and content verification for model artifacts.

use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactHash {
    /// Stable logical name, such as a shard filename or `mtp/model.safetensors`.
    pub name: String,
    /// Lowercase SHA-256 digest of the artifact bytes.
    pub sha256: String,
}

/// All data that can change model weights, tokenization, or packed runtime state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelIdentity {
    pub config_revision: String,
    pub config_sha256: String,
    pub quantization: String,
    pub tokenizer_sha256: String,
    pub template_sha256: String,
    pub shards: Vec<ArtifactHash>,
    pub mtp_artifacts: Vec<ArtifactHash>,
    pub packing_version: u32,
}

fn digest_bytes(value: &str) -> Result<Vec<u8>> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid(
            "artifact SHA-256 must be 64 lowercase hex digits",
        ));
    }
    let mut digest = Vec::with_capacity(32);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = (pair[0] as char)
            .to_digit(16)
            .ok_or(Error::Invalid("invalid SHA-256 digit"))?;
        let low = (pair[1] as char)
            .to_digit(16)
            .ok_or(Error::Invalid("invalid SHA-256 digit"))?;
        digest.push(((high << 4) | low) as u8);
    }
    Ok(digest)
}

fn validate_artifacts(artifacts: &[ArtifactHash], allow_empty: bool) -> Result<()> {
    if artifacts.is_empty() && !allow_empty {
        return Err(Error::Invalid("model identity has no weight shards"));
    }
    let mut names: Vec<&str> = Vec::new();
    names
        .try_reserve_exact(artifacts.len())
        .map_err(|_| Error::Capacity("artifact identity metadata"))?;
    for artifact in artifacts {
        validate_artifact_name(&artifact.name)?;
        digest_bytes(&artifact.sha256)?;
        names.push(&artifact.name);
    }
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Error::Invalid("duplicate artifact identity name"));
    }
    Ok(())
}

fn validate_artifact_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains('\0')
        || name
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        || Path::new(name).is_absolute()
        || Path::new(name)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(Error::Invalid(
            "artifact identity name must be a canonical relative path",
        ));
    }
    Ok(())
}

impl ModelIdentity {
    pub fn validate(&self) -> Result<()> {
        if self.config_revision.is_empty()
            || self.quantization.is_empty()
            || self.packing_version == 0
        {
            return Err(Error::Invalid(
                "model identity is missing required metadata",
            ));
        }
        digest_bytes(&self.config_sha256)?;
        digest_bytes(&self.tokenizer_sha256)?;
        digest_bytes(&self.template_sha256)?;
        validate_artifacts(&self.shards, false)?;
        validate_artifacts(&self.mtp_artifacts, true)
    }

    /// Stable, order-independent fingerprint over all model-affecting inputs.
    pub fn fingerprint(&self) -> Result<[u8; 32]> {
        self.validate()?;
        let mut hasher = Sha256::new();
        hasher.update(b"agentix-qwen-model-identity-v1\0");
        hash_field(&mut hasher, self.config_revision.as_bytes());
        hash_field(&mut hasher, &digest_bytes(&self.config_sha256)?);
        hash_field(&mut hasher, self.quantization.as_bytes());
        hash_field(&mut hasher, &digest_bytes(&self.tokenizer_sha256)?);
        hash_field(&mut hasher, &digest_bytes(&self.template_sha256)?);
        hasher.update(self.packing_version.to_be_bytes());
        hash_artifacts(&mut hasher, &self.shards)?;
        hash_artifacts(&mut hasher, &self.mtp_artifacts)?;
        Ok(hasher.finalize().into())
    }
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn hash_artifacts(hasher: &mut Sha256, artifacts: &[ArtifactHash]) -> Result<()> {
    let mut ordered: Vec<&ArtifactHash> = Vec::new();
    ordered
        .try_reserve_exact(artifacts.len())
        .map_err(|_| Error::Capacity("artifact identity metadata"))?;
    ordered.extend(artifacts);
    ordered.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    hasher.update((ordered.len() as u64).to_be_bytes());
    for artifact in ordered {
        hash_field(hasher, artifact.name.as_bytes());
        hash_field(hasher, &digest_bytes(&artifact.sha256)?);
    }
    Ok(())
}

/// Hashes a file incrementally. Call from a blocking thread for large model shards.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(encoded)
}

/// Verifies that file contents match a lowercase SHA-256 pinned in a manifest.
pub fn verify_sha256_file(path: &Path, expected: &str) -> Result<()> {
    digest_bytes(expected)?;
    if sha256_file(path)? == expected {
        Ok(())
    } else {
        Err(Error::Invalid("artifact SHA-256 does not match manifest"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn identity() -> ModelIdentity {
        ModelIdentity {
            config_revision: "revision-a".into(),
            config_sha256: "01".repeat(32),
            quantization: "iq3_s".into(),
            tokenizer_sha256: "02".repeat(32),
            template_sha256: "03".repeat(32),
            shards: vec![
                ArtifactHash {
                    name: "model-00002-of-00002.gguf".into(),
                    sha256: "05".repeat(32),
                },
                ArtifactHash {
                    name: "model-00001-of-00002.gguf".into(),
                    sha256: "04".repeat(32),
                },
            ],
            mtp_artifacts: Vec::new(),
            packing_version: 1,
        }
    }

    #[test]
    fn fingerprint_sorts_shards_and_includes_packing_version() -> Result<()> {
        let mut first = identity();
        let mut reordered = identity();
        reordered.shards.reverse();
        assert_eq!(first.fingerprint()?, reordered.fingerprint()?);
        first.packing_version = 2;
        assert_ne!(first.fingerprint()?, reordered.fingerprint()?);

        let baseline = reordered.fingerprint()?;
        let mut changed = reordered.clone();
        changed.config_revision.push_str("-changed");
        assert_ne!(changed.fingerprint()?, baseline);
        let mut changed = reordered.clone();
        changed.config_sha256 = "06".repeat(32);
        assert_ne!(changed.fingerprint()?, baseline);
        let mut changed = reordered.clone();
        changed.quantization = "iq2_xs".into();
        assert_ne!(changed.fingerprint()?, baseline);
        let mut changed = reordered.clone();
        changed.tokenizer_sha256 = "07".repeat(32);
        assert_ne!(changed.fingerprint()?, baseline);
        let mut changed = reordered.clone();
        changed.template_sha256 = "08".repeat(32);
        assert_ne!(changed.fingerprint()?, baseline);
        let mut changed = reordered.clone();
        changed.shards[0].sha256 = "09".repeat(32);
        assert_ne!(changed.fingerprint()?, baseline);
        let mut changed = reordered;
        changed.mtp_artifacts.push(ArtifactHash {
            name: "mtp.gguf".into(),
            sha256: "0a".repeat(32),
        });
        assert_ne!(changed.fingerprint()?, baseline);
        Ok(())
    }

    #[test]
    fn rejects_duplicate_or_malformed_artifacts() {
        let mut model = identity();
        model.shards[1].name = model.shards[0].name.clone();
        assert!(model.validate().is_err());
        let mut model = identity();
        model.tokenizer_sha256 = "not-a-digest".into();
        assert!(model.validate().is_err());
        for name in [
            "../outside.gguf",
            "/absolute.gguf",
            "folder/../outside.gguf",
            "folder//model.gguf",
            "folder\\model.gguf",
        ] {
            let mut model = identity();
            model.shards[0].name = name.to_owned();
            assert!(model.validate().is_err(), "accepted {name:?}");
        }
    }

    #[test]
    fn hashes_file_with_sha256() -> Result<()> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Invalid("system clock before Unix epoch"))?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("agentix-qwen-hash-{nonce}"));
        std::fs::write(&path, b"abc")?;
        let digest = sha256_file(&path);
        let valid = verify_sha256_file(
            &path,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
        let invalid = verify_sha256_file(&path, &"00".repeat(32));
        std::fs::remove_file(&path)?;
        assert_eq!(
            digest?,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(valid.is_ok());
        assert!(invalid.is_err());
        Ok(())
    }
}
