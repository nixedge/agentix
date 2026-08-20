pub mod backend;
pub mod engine;
pub mod error;
pub mod meta;
pub mod pool;
pub mod store;

pub use engine::InferEngine;
pub use error::InferError;

use std::path::PathBuf;

// ── Enums ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ModelFormat {
    Gguf,
    Safetensors,
    /// Legacy ggml binary format used by whisper.cpp `ggml-*.bin` files.
    WhisperBin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BackendHint {
    LlamaCpp,
    Candle,
    Whisper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Capability {
    Completion,
    Embedding,
    Vision,
    Transcription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FinishReason {
    Stop,
    Length,
    Error,
}

// ── Structs ──────────────────────────────────────────────────────────────────

#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InferConfig {
    pub models_dir: PathBuf,
    pub vram_limit_bytes: Option<u64>,
    pub max_loaded_models: usize,
    pub max_ctx: u32,
}

impl InferConfig {
    pub fn new(
        models_dir: PathBuf,
        vram_limit_bytes: Option<u64>,
        max_loaded_models: usize,
        max_ctx: u32,
    ) -> Self {
        Self {
            models_dir,
            vram_limit_bytes,
            max_loaded_models,
            max_ctx,
        }
    }
}

impl Default for InferConfig {
    fn default() -> Self {
        Self {
            models_dir: PathBuf::from("/var/lib/agentix/models"),
            vram_limit_bytes: None,
            max_loaded_models: 2,
            max_ctx: 32768,
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub architecture: String,
    pub format: ModelFormat,
    pub backend: BackendHint,
    pub context_length: u32,
    pub embedding_length: u32,
    pub capabilities: Vec<Capability>,
    pub quantization: Option<String>,
    pub parameter_count: u64,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum GrammarConstraint {
    Gbnf(String),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompletionRequest {
    pub messages: Vec<CompletionMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stop: Vec<String>,
    pub grammar: Option<GrammarConstraint>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompletionMessage {
    pub role: String,
    pub content: String,
}

impl CompletionRequest {
    pub fn new(
        messages: Vec<CompletionMessage>,
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        top_p: Option<f32>,
        stop: Vec<String>,
    ) -> Self {
        Self {
            messages,
            max_tokens,
            temperature,
            top_p,
            stop,
            grammar: None,
        }
    }
}

impl CompletionMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompletionChunk {
    pub delta: String,
    pub finish_reason: Option<FinishReason>,
}

impl CompletionChunk {
    pub fn new(delta: String, finish_reason: Option<FinishReason>) -> Self {
        Self {
            delta,
            finish_reason,
        }
    }
}
