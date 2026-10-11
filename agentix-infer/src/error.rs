use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum InferError {
    #[error("invalid tool exchange: {0}")]
    InvalidToolExchange(&'static str),

    #[error("backend does not support typed tool completion")]
    ToolCompletionUnsupported,

    #[error("inference request cancelled")]
    Cancelled,

    #[error("model is already in use: {0}")]
    ModelBusy(String),

    #[error("model capacity is occupied by active requests")]
    CapacityExhausted,

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
