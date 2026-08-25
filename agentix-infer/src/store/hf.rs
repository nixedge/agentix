use crate::error::InferError;
use std::path::{Path, PathBuf};

/// Parsed reference to a HuggingFace model file.
#[derive(Debug, Clone)]
pub struct HfRef {
    pub repo_id: String,
    pub filename: String,
    pub revision: String,
}

/// Parse model refs:
/// - "hf.co/org/repo:filename.gguf" → HfRef { repo_id: "org/repo", filename: "filename.gguf", revision: "main" }
pub fn parse_hf_ref(model_ref: &str) -> Result<HfRef, InferError> {
    let stripped = model_ref.strip_prefix("hf.co/").unwrap_or(model_ref);
    // format: org/repo:filename
    if let Some((repo_part, filename)) = stripped.rsplit_once(':') {
        // ensure repo_part has exactly one slash (i.e. it's org/repo)
        if repo_part.contains('/') {
            return Ok(HfRef {
                repo_id: repo_part.to_string(),
                filename: filename.to_string(),
                revision: "main".to_string(),
            });
        }
    }
    // no colon — treat whole thing as repo_id with no specific file
    Err(InferError::DownloadFailed(format!(
        "HF ref '{}' must specify a filename: use hf.co/org/repo:filename.gguf",
        model_ref
    )))
}

/// Fetch the sha256 of a HuggingFace file via HTTP HEAD.
/// HF sets `x-linked-etag` (LFS files) or `etag` to the sha256 hex.
fn hf_file_sha256(hf_ref: &HfRef) -> Result<String, InferError> {
    let url = format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        hf_ref.repo_id, hf_ref.revision, hf_ref.filename
    );
    let resp = ureq::head(&url)
        .call()
        .map_err(|e| InferError::DownloadFailed(format!("HF HEAD request failed: {e}")))?;
    let etag = resp
        .header("x-linked-etag")
        .or_else(|| resp.header("etag"))
        .ok_or_else(|| {
            InferError::DownloadFailed("HF response missing x-linked-etag / etag".to_string())
        })?
        .trim_matches('"')
        .to_string();
    Ok(etag)
}

/// When an HfRef filename looks like an Ollama quantization tag (no `.gguf`
/// extension), list the repo's files and find the single GGUF whose name
/// contains that tag (case-insensitive). Errors if zero or multiple match.
fn resolve_filename(hf_ref: &HfRef) -> Result<String, InferError> {
    let lower = hf_ref.filename.to_lowercase();
    if lower.ends_with(".gguf") || lower.ends_with(".bin") {
        return Ok(hf_ref.filename.clone());
    }

    use hf_hub::api::sync::ApiBuilder;
    use hf_hub::{Repo, RepoType};

    let api = ApiBuilder::new()
        .with_progress(false)
        .build()
        .map_err(|e| InferError::DownloadFailed(e.to_string()))?;

    let repo = api.repo(Repo::with_revision(
        hf_ref.repo_id.clone(),
        RepoType::Model,
        hf_ref.revision.clone(),
    ));

    let info = repo
        .info()
        .map_err(|e| InferError::DownloadFailed(format!("HF repo info failed: {e}")))?;

    let tag_lower = hf_ref.filename.to_lowercase();
    let matches: Vec<_> = info
        .siblings
        .iter()
        .filter(|s| {
            let name = s.rfilename.to_lowercase();
            name.ends_with(".gguf") && name.contains(&tag_lower)
        })
        .collect();

    match matches.len() {
        0 => Err(InferError::DownloadFailed(format!(
            "no GGUF file matching '{}' found in {}/{}",
            hf_ref.filename, hf_ref.repo_id, hf_ref.revision
        ))),
        1 => {
            tracing::info!(
                tag = %hf_ref.filename,
                resolved = %matches[0].rfilename,
                "resolved Ollama tag to HF filename"
            );
            Ok(matches[0].rfilename.clone())
        }
        n => Err(InferError::DownloadFailed(format!(
            "{n} GGUF files match '{}' in {}; be more specific",
            hf_ref.filename, hf_ref.repo_id
        ))),
    }
}

/// Candidate directories that may contain Ollama-format blobs
/// (`sha256-{hex}` files).
fn ollama_blob_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // OLLAMA_MODELS env var takes precedence
    if let Ok(p) = std::env::var("OLLAMA_MODELS") {
        dirs.push(PathBuf::from(p).join("blobs"));
    }
    // Standard Ollama location
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".ollama").join("models").join("blobs"));
    }
    dirs
}

/// Try to find a blob by sha256 in any Ollama blobs directory.
fn find_in_ollama(sha256: &str) -> Option<PathBuf> {
    let filename = format!("sha256-{sha256}");
    for dir in ollama_blob_dirs() {
        let candidate = dir.join(&filename);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Link `src` into `dest`, trying hard-link first, then copy.
fn link_or_copy(src: &Path, dest: &Path) -> Result<(), InferError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::hard_link(src, dest)
        .or_else(|_| std::fs::copy(src, dest).map(|_| ()))
        .map_err(InferError::Io)
}

/// Download a file from HuggingFace Hub and write it to the blob store.
/// Returns (sha256_hash, size_bytes).
///
/// Priority:
/// 1. Blob already in agentix store → return immediately (no I/O)
/// 2. Blob found in an Ollama blobs dir → hard-link into agentix store
/// 3. Download from HuggingFace Hub
pub fn download_to_blob_store(
    hf_ref: &HfRef,
    models_dir: &Path,
) -> Result<(String, u64), InferError> {
    // Resolve Ollama-style quantization tags (e.g. "Q8_0") to the actual
    // GGUF filename in the HF repo before making any blob requests.
    let resolved_filename = resolve_filename(hf_ref)?;
    let hf_ref = HfRef {
        filename: resolved_filename,
        ..hf_ref.clone()
    };
    let hf_ref = &hf_ref;

    // 1. Fetch sha256 without downloading the file body
    let sha256 = hf_file_sha256(hf_ref)?;
    tracing::debug!(sha256 = %sha256, repo = %hf_ref.repo_id, file = %hf_ref.filename, "resolved HF file sha256");

    let dest = super::blob::blob_path(models_dir, &sha256);

    // 2. Already in agentix store
    if dest.exists() {
        let size = dest.metadata()?.len();
        tracing::info!(hash = %sha256, "blob already in agentix store, skipping download");
        return Ok((sha256, size));
    }

    // 3. Reuse blob from Ollama store (avoids re-download)
    if let Some(ollama_src) = find_in_ollama(&sha256) {
        tracing::info!(
            hash = %sha256,
            src = %ollama_src.display(),
            "hardlinking blob from Ollama store"
        );
        link_or_copy(&ollama_src, &dest)?;
        let size = dest.metadata()?.len();
        return Ok((sha256, size));
    }

    // 4. Download from HuggingFace Hub
    tracing::info!(
        repo = %hf_ref.repo_id,
        filename = %hf_ref.filename,
        "downloading from HuggingFace Hub"
    );

    use hf_hub::api::sync::ApiBuilder;
    use hf_hub::{Repo, RepoType};

    let cache_dir = tempfile::tempdir().map_err(InferError::Io)?;
    let api = ApiBuilder::new()
        .with_cache_dir(cache_dir.path().to_path_buf())
        .with_progress(false)
        .build()
        .map_err(|e| InferError::DownloadFailed(e.to_string()))?;

    let repo = api.repo(Repo::with_revision(
        hf_ref.repo_id.clone(),
        RepoType::Model,
        hf_ref.revision.clone(),
    ));

    let cached_path = repo
        .get(&hf_ref.filename)
        .map_err(|e| InferError::DownloadFailed(format!("HF download failed: {}", e)))?;

    let file = std::fs::File::open(&cached_path)?;
    let (hash, size) = super::blob::write_blob(models_dir, file)?;

    tracing::info!(
        repo = %hf_ref.repo_id,
        filename = %hf_ref.filename,
        hash = %hash,
        size,
        "downloaded blob from HuggingFace"
    );

    Ok((hash, size))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_hf_ref() {
        let r = parse_hf_ref(
            "hf.co/second-state/All-MiniLM-L6-v2-Embedding-GGUF:all-MiniLM-L6-v2-Q8_0.gguf",
        )
        .unwrap();
        assert_eq!(r.repo_id, "second-state/All-MiniLM-L6-v2-Embedding-GGUF");
        assert_eq!(r.filename, "all-MiniLM-L6-v2-Q8_0.gguf");
        assert_eq!(r.revision, "main");
    }

    #[test]
    fn parse_hf_ref_without_prefix() {
        let r = parse_hf_ref("org/repo:model.gguf").unwrap();
        assert_eq!(r.repo_id, "org/repo");
        assert_eq!(r.filename, "model.gguf");
    }

    #[test]
    fn parse_hf_ref_missing_filename_returns_err() {
        assert!(parse_hf_ref("hf.co/org/repo-only").is_err());
    }
}
