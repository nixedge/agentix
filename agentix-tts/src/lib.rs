use std::io::Cursor;
use std::path::Path;

use hound::{SampleFormat, WavSpec, WavWriter};

#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("TTS not available: no backend compiled in")]
    NotAvailable,
    #[error("synthesis failed: {0}")]
    Synthesis(String),
    #[error("WAV encoding failed: {0}")]
    Wav(#[from] hound::Error),
}

/// Kokoro TTS engine. Currently a stub — synthesize() always returns NotAvailable.
/// Replace inner implementation once a kokoro crate compatible with ort =2.0.0-rc.13
/// is available (candidates: kokoro-en, sayd-kokoro).
pub struct TtsEngine {
    voice: String,
    max_text_len: usize,
}

impl TtsEngine {
    pub async fn new(
        _model_path: &Path,
        _voices_dir: &Path,
        voice: &str,
        max_text_len: usize,
    ) -> Result<Self, TtsError> {
        Ok(TtsEngine {
            voice: voice.to_string(),
            max_text_len,
        })
    }

    pub fn voice(&self) -> &str {
        &self.voice
    }

    pub fn max_text_len(&self) -> usize {
        self.max_text_len
    }

    pub fn sample_rate(&self) -> u32 {
        24_000
    }

    pub async fn synthesize(&self, _text: &str) -> Result<Vec<f32>, TtsError> {
        Err(TtsError::NotAvailable)
    }

    pub fn to_wav(&self, samples: &[f32]) -> Result<Vec<u8>, TtsError> {
        let spec = WavSpec {
            channels: 1,
            sample_rate: self.sample_rate(),
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut writer = WavWriter::new(cursor, spec)?;
            for &s in samples {
                writer.write_sample(s)?;
            }
            writer.finalize()?;
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_wav_produces_valid_wav_header() {
        let engine = TtsEngine {
            voice: "af_heart".to_string(),
            max_text_len: 500,
        };
        let samples = vec![0.0f32; 500];
        let wav = engine.to_wav(&samples).expect("to_wav should succeed");
        let cursor = Cursor::new(wav);
        let reader = hound::WavReader::new(cursor).expect("valid WAV header");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 24_000);
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_format, SampleFormat::Float);
    }
}
