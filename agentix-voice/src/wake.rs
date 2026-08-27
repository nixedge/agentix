use anyhow::Result;
use oww_rs::{
    config::SpeechUnlockType,
    oww::{OwwModel, OWW_MODEL_CHUNK_SIZE},
};
use std::path::Path;

use crate::config::WakeWordConfig;

pub struct WakeDetector {
    model: OwwModel,
}

impl WakeDetector {
    pub fn new(config: &WakeWordConfig) -> Result<Self> {
        let model = if Path::new(&config.model).exists() {
            OwwModel::from_file(&config.model, config.model.clone(), config.threshold).map_err(
                |e| anyhow::anyhow!("loading wake word model from {}: {e}", config.model),
            )?
        } else {
            let model_type = parse_builtin(&config.model)?;
            OwwModel::new(model_type, config.threshold).map_err(|e| {
                anyhow::anyhow!("loading built-in wake word model '{}': {e}", config.model)
            })?
        };
        Ok(Self { model })
    }

    /// Feed one chunk of 16 kHz mono f32 audio. Returns true if the wake word was detected.
    /// Chunk must be exactly `OWW_MODEL_CHUNK_SIZE` (1280) samples.
    pub fn process_chunk(&mut self, chunk: Vec<f32>) -> bool {
        debug_assert_eq!(chunk.len(), OWW_MODEL_CHUNK_SIZE);
        self.model.detection(chunk).detected
    }

    pub fn chunk_size() -> usize {
        OWW_MODEL_CHUNK_SIZE
    }
}

fn parse_builtin(name: &str) -> Result<SpeechUnlockType> {
    match name.to_lowercase().as_str() {
        "alexa" => Ok(SpeechUnlockType::OpenWakeWordAlexa),
        "hey_mycroft" | "mycroft" => Ok(SpeechUnlockType::OpenWakeWordHeyMycroft),
        "hey_jarvis" | "jarvis" => Ok(SpeechUnlockType::OpenWakeWordHeyJarvis),
        "ahoj_hugo" | "hugo" => Ok(SpeechUnlockType::OpenWakeWordAhojHugo),
        other => anyhow::bail!(
            "unknown built-in wake word '{}'; use: alexa, hey_jarvis, hey_mycroft, ahoj_hugo, or a path to a custom .onnx model",
            other
        ),
    }
}
