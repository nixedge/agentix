use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceConfig {
    pub agent_url: String,
    pub daemon_url: String,
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: f32,
    #[serde(default = "default_vad_silence_ms")]
    pub vad_silence_ms: u64,
    #[serde(default = "default_tts_voice")]
    pub tts_voice: String,
    pub wake_word: WakeWordConfig,
    pub session_id: Option<String>,
    #[serde(default = "default_stt_model")]
    pub stt_model: String,
}

fn default_stt_model() -> String {
    "ggerganov/whisper.cpp:ggml-base.en-q5_1.bin".to_string()
}

/// Wake word model configuration for oww-rs.
/// `model` can be a built-in keyword ("hey_jarvis", "alexa", "hey_mycroft", "ahoj_hugo")
/// or a path to a custom openWakeWord ONNX model file.
#[derive(Debug, Clone, Deserialize)]
pub struct WakeWordConfig {
    #[serde(default = "default_ww_model")]
    pub model: String,
    #[serde(default = "default_ww_threshold")]
    pub threshold: f32,
}

fn default_ww_model() -> String {
    "hey_jarvis".to_string()
}
fn default_ww_threshold() -> f32 {
    0.3
}
fn default_vad_threshold() -> f32 {
    0.02
}
fn default_vad_silence_ms() -> u64 {
    800
}
fn default_tts_voice() -> String {
    "en_US-lessac-medium".to_string()
}

impl VoiceConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config {}: {e}", path.display()))?;
        toml::from_str(&raw).map_err(|e| anyhow::anyhow!("parsing config TOML: {e}"))
    }
}
