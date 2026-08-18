pub mod blob;
pub mod hf;
pub mod manifest;

use crate::meta;
use crate::{error::InferError, BackendHint, ModelFormat, ModelInfo};
use manifest::{AgentixExtension, Manifest, ManifestLayer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct ModelStore {
    pub models_dir: PathBuf,
}

impl ModelStore {
    pub fn new(models_dir: PathBuf) -> Self {
        Self { models_dir }
    }

    /// Pull a model from a remote source.
    /// Supported refs:
    /// - "hf.co/org/repo:filename.gguf" — HuggingFace Hub GGUF (with prefix)
    /// - "org/repo:filename.gguf" — HuggingFace Hub GGUF (without prefix)
    /// - "org/repo:Q4_K_M" — fuzzy tag: lists repo, picks matching GGUF
    /// - "/local/path/to/model.gguf" — local file path
    pub fn pull(&self, model_ref: &str) -> Result<ModelInfo, InferError> {
        if model_ref.starts_with('/') || model_ref.starts_with("./") {
            self.register_local(model_ref)
        } else {
            self.pull_hf(model_ref)
        }
    }

    fn pull_hf(&self, model_ref: &str) -> Result<ModelInfo, InferError> {
        let hf_ref = hf::parse_hf_ref(model_ref)?;
        let format = detect_format(&hf_ref.filename);
        let (hash, size) = hf::download_to_blob_store(&hf_ref, &self.models_dir)?;

        let blob_path = blob::blob_path(&self.models_dir, &hash);
        let detected = meta::detect_capabilities(&blob_path, format)?;
        let backend = select_backend(format, &detected);

        // Use the original model ref as the canonical name so that
        // store.info(model_ref) finds the manifest after a pull.
        let name = model_ref.to_string();

        let manifest = build_manifest(&hash, size, format, backend, &detected, &name);
        let manifest_path = self.manifest_path_for(&name);
        manifest::write_manifest(&manifest_path, &manifest)?;

        Ok(manifest::manifest_to_model_info(name, &manifest))
    }

    fn register_local(&self, path_str: &str) -> Result<ModelInfo, InferError> {
        let path = Path::new(path_str);
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| InferError::DownloadFailed("invalid local path".to_string()))?;
        let format = detect_format(filename);

        let file = std::fs::File::open(path)?;
        let (hash, size) = blob::write_blob(&self.models_dir, file)?;

        let blob_path = blob::blob_path(&self.models_dir, &hash);
        let detected = meta::detect_capabilities(&blob_path, format)?;
        let backend = select_backend(format, &detected);

        // Use the filename as the canonical name so that the manifest path stays
        // within the store (absolute path_str in PathBuf::join would replace the base).
        let name = filename.to_string();

        let manifest = build_manifest(&hash, size, format, backend, &detected, &name);
        let manifest_path = self.manifest_path_for(&name);
        manifest::write_manifest(&manifest_path, &manifest)?;

        Ok(manifest::manifest_to_model_info(name, &manifest))
    }

    pub fn list(&self) -> Vec<ModelInfo> {
        let manifests_dir = self.models_dir.join("manifests");
        if !manifests_dir.exists() {
            return vec![];
        }
        let mut result = Vec::new();
        self.walk_manifests(&manifests_dir, &mut result);
        result
    }

    fn walk_manifests(&self, dir: &Path, out: &mut Vec<ModelInfo>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                self.walk_manifests(&path, out);
            } else if path.is_file()
                && !path
                    .file_name()
                    .map(|n| n == "_aliases.json")
                    .unwrap_or(false)
            {
                if let Ok(m) = manifest::read_manifest(&path) {
                    // Reconstruct name: strip "manifests/agentix/" prefix and "/latest" suffix.
                    // Layout on disk: manifests/agentix/<name>/latest
                    let agentix_dir = self.models_dir.join("manifests").join("agentix");
                    let name = path
                        .strip_prefix(&agentix_dir)
                        .ok()
                        .and_then(|p| p.parent()) // drop the "latest" filename
                        .and_then(|p| p.to_str())
                        .unwrap_or("")
                        .to_string();
                    if !name.is_empty() {
                        out.push(manifest::manifest_to_model_info(name, &m));
                    }
                }
            }
        }
    }

    pub fn info(&self, name: &str) -> Option<ModelInfo> {
        let path = self.find_manifest(name)?;
        let m = manifest::read_manifest(&path).ok()?;
        Some(manifest::manifest_to_model_info(name.to_string(), &m))
    }

    pub fn remove(&self, name: &str) -> Result<(), InferError> {
        let manifest_path = self.manifest_path_for(name);
        let manifest = manifest::read_manifest(&manifest_path)?;

        // Remove manifest file
        std::fs::remove_file(&manifest_path)?;

        // Remove blobs if not referenced by other manifests
        for layer in &manifest.layers {
            let hash = layer
                .digest
                .strip_prefix("sha256:")
                .unwrap_or(&layer.digest);
            if !self.blob_referenced_elsewhere(hash, name) {
                let blob = blob::blob_path(&self.models_dir, hash);
                if blob.exists() {
                    std::fs::remove_file(&blob)?;
                }
            }
        }
        Ok(())
    }

    /// Resolve a model name to its primary GGUF/safetensors blob path.
    pub fn resolve(&self, name: &str) -> Option<PathBuf> {
        let manifest_path = self.find_manifest(name)?;
        let manifest = manifest::read_manifest(&manifest_path).ok()?;
        let layer = manifest.layers.iter().find(|l| {
            l.media_type == "application/vnd.ollama.image.model"
                || l.media_type == "application/vnd.ollama.image.tensor"
        })?;
        let hash = layer.digest.strip_prefix("sha256:")?;
        Some(blob::blob_path(&self.models_dir, hash))
    }

    fn find_manifest(&self, name: &str) -> Option<PathBuf> {
        let p = self.manifest_path_for(name);
        if p.exists() {
            return Some(p);
        }
        // Alias lookup: check _aliases.json for an alternate canonical name.
        let canonical = self.read_aliases().remove(name)?;
        let p2 = self.manifest_path_for(&canonical);
        if p2.exists() {
            Some(p2)
        } else {
            None
        }
    }

    fn aliases_path(&self) -> PathBuf {
        self.models_dir.join("manifests").join("_aliases.json")
    }

    fn read_aliases(&self) -> HashMap<String, String> {
        let path = self.aliases_path();
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => return HashMap::new(),
        };
        serde_json::from_slice(&data).unwrap_or_default()
    }

    fn write_alias(&self, alias: &str, canonical: &str) -> Result<(), InferError> {
        let path = self.aliases_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut aliases = self.read_aliases();
        aliases.insert(alias.to_string(), canonical.to_string());
        let data =
            serde_json::to_vec_pretty(&aliases).map_err(|e| InferError::Manifest(e.to_string()))?;
        // Atomic write: write to a temp file, then rename.
        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &data)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    /// Register an alias pointing to a canonical model name.
    /// Future daemon handlers can call this to expose short names (e.g. "mistral" →
    /// "bartowski/Mistral-7B-v0.1-GGUF:Q4_K_M"). Only writes when alias != canonical.
    pub fn add_alias(&self, alias: &str, canonical: &str) -> Result<(), InferError> {
        if alias == canonical {
            return Ok(());
        }
        self.write_alias(alias, canonical)
    }

    fn manifest_path_for(&self, name: &str) -> PathBuf {
        // Agentix write layout: manifests/agentix/<name>/latest
        self.models_dir
            .join("manifests")
            .join("agentix")
            .join(name)
            .join("latest")
    }

    fn blob_referenced_elsewhere(&self, hash: &str, exclude_name: &str) -> bool {
        let manifests_dir = self.models_dir.join("manifests");
        if !manifests_dir.exists() {
            return false;
        }
        let mut referenced = false;
        self.check_blob_refs(&manifests_dir, hash, exclude_name, &mut referenced);
        referenced
    }

    fn check_blob_refs(&self, dir: &Path, hash: &str, exclude_name: &str, found: &mut bool) {
        if *found {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                self.check_blob_refs(&path, hash, exclude_name, found);
            } else if path.is_file() {
                // Skip the manifest we're removing
                let manifest_base = self.manifest_path_for(exclude_name);
                if path == manifest_base {
                    continue;
                }
                if let Ok(m) = manifest::read_manifest(&path) {
                    for layer in &m.layers {
                        let h = layer
                            .digest
                            .strip_prefix("sha256:")
                            .unwrap_or(&layer.digest);
                        if h == hash {
                            *found = true;
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn detect_format(filename: &str) -> ModelFormat {
    if filename.ends_with(".gguf") {
        ModelFormat::Gguf
    } else if filename.ends_with(".safetensors") {
        ModelFormat::Safetensors
    } else if filename.ends_with(".bin") {
        // whisper.cpp legacy ggml binary format (e.g. ggml-tiny.en.bin)
        ModelFormat::WhisperBin
    } else {
        ModelFormat::Gguf // default
    }
}

fn select_backend(format: ModelFormat, meta: &meta::DetectedMeta) -> BackendHint {
    // Whisper capability overrides format-based default — both LlamaCpp and Whisper
    // accept GGUF, so we must disambiguate by capability rather than format.
    if meta
        .capabilities
        .contains(&crate::Capability::Transcription)
    {
        return BackendHint::Whisper;
    }
    match format {
        ModelFormat::Gguf | ModelFormat::WhisperBin => BackendHint::LlamaCpp,
        ModelFormat::Safetensors => BackendHint::Candle,
    }
}

fn build_manifest(
    hash: &str,
    size: u64,
    _format: ModelFormat,
    backend: BackendHint,
    meta: &meta::DetectedMeta,
    _name: &str,
) -> Manifest {
    Manifest {
        schema_version: 2,
        media_type: "application/vnd.docker.distribution.manifest.v2+json".to_string(),
        config: ManifestLayer {
            media_type: "application/vnd.docker.container.image.v1+json".to_string(),
            digest: format!("sha256:{}", hash),
            size: 0,
            from: None,
        },
        layers: vec![ManifestLayer {
            media_type: "application/vnd.ollama.image.model".to_string(),
            digest: format!("sha256:{}", hash),
            size,
            from: None,
        }],
        _agentix: Some(AgentixExtension {
            backend,
            capabilities: meta.capabilities.clone(),
            architecture: if meta.architecture.is_empty() {
                None
            } else {
                Some(meta.architecture.clone())
            },
            context_length: if meta.context_length > 0 {
                Some(meta.context_length)
            } else {
                None
            },
            embedding_length: if meta.embedding_length > 0 {
                Some(meta.embedding_length)
            } else {
                None
            },
            quantization: None,
            parameter_count: if meta.parameter_count > 0 {
                Some(meta.parameter_count)
            } else {
                None
            },
        }),
    }
}
