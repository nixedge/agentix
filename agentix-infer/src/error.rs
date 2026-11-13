use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum InferError {
    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("model {0} does not support capability {1:?}")]
    CapabilityMissing(String, crate::Capability),

    #[error("no registered backend supports format {0:?}")]
    NoBackend(crate::ModelFormat),

    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("download failed: {0}")]
    DownloadFailed(String),

    #[error("VRAM exhausted: cannot load {model} ({required_bytes} bytes required)")]
    VramExhausted { model: String, required_bytes: u64 },

    #[error("shared GPU capacity unavailable: {0}")]
    GpuCapacity(String),

    #[error("transcription error: {0}")]
    Transcription(String),

    #[error("backend error: {0}")]
    Backend(String),

    #[error("manifest error: {0}")]
    Manifest(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("context window exceeded: prompt is {prompt_tokens} tokens, max_new_tokens is {max_new_tokens}, but context window is only {context_window}")]
    ContextExceeded {
        prompt_tokens: u32,
        max_new_tokens: u32,
        context_window: u32,
    },
}

impl InferError {
    pub fn is_capacity(&self) -> bool {
        matches!(self, Self::VramExhausted { .. } | Self::GpuCapacity(_))
    }
}
