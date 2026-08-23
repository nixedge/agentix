use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceConfig {
    pub agent_url: String,
    pub daemon_url: String,
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: f32,
    #[serde(default = "default_vad_silence_ms")]
    pub vad_silence_ms: u64,
    pub tts: TtsCfg,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TtsCfg {
    pub model_path: PathBuf,
    pub voices_dir: PathBuf,
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_max_text")]
    pub max_text_len: usize,
}

fn default_vad_threshold() -> f32 {
    0.02
}
fn default_vad_silence_ms() -> u64 {
    800
}
fn default_voice() -> String {
    "af_heart".to_string()
}
fn default_max_text() -> usize {
    500
}
impl VoiceConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config {}: {e}", path.display()))?;
        toml::from_str(&raw).map_err(|e| anyhow::anyhow!("parsing config TOML: {e}"))
    }
}
