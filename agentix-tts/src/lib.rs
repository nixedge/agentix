use std::io::Cursor;
use std::path::Path;

use hound::{SampleFormat, WavSpec, WavWriter};
use kokoro_tts::{KokoroTts, Voice};

#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("TTS model load failed: {0}")]
    ModelLoad(String),
    #[error("synthesis failed: {0}")]
    Synthesis(String),
    #[error("WAV encoding failed: {0}")]
    Wav(#[from] hound::Error),
}

pub struct TtsEngine {
    inner: KokoroTts,
    voice: String,
    max_text_len: usize,
}

impl TtsEngine {
    pub async fn new(
        model_path: &Path,
        voices_path: &Path,
        voice: &str,
        max_text_len: usize,
    ) -> Result<Self, TtsError> {
        let inner = KokoroTts::new(model_path, voices_path)
            .await
            .map_err(|e| TtsError::ModelLoad(e.to_string()))?;
        Ok(TtsEngine {
            inner,
            voice: voice.to_owned(),
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

    pub async fn synthesize(&self, text: &str) -> Result<Vec<f32>, TtsError> {
        let text = if text.len() > self.max_text_len {
            &text[..self.max_text_len]
        } else {
            text
        };
        let voice = parse_voice(&self.voice, 1.0);
        let (samples, _duration) = self
            .inner
            .synth(text, voice)
            .await
            .map_err(|e| TtsError::Synthesis(e.to_string()))?;
        Ok(samples)
    }

    pub fn to_wav(&self, samples: &[f32]) -> Result<Vec<u8>, TtsError> {
        encode_wav(samples, self.sample_rate()).map_err(TtsError::Wav)
    }
}

/// Encode f32 PCM samples into a WAV file at the given sample rate.
pub fn encode_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>, hound::Error> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
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

fn parse_voice(name: &str, speed: f32) -> Voice {
    match name.to_lowercase().replace('-', "_").as_str() {
        "af_heart" => Voice::AfHeart(speed),
        "af_jessica" => Voice::AfJessica(speed),
        "af_bella" => Voice::AfBella(speed),
        "af_nova" => Voice::AfNova(speed),
        "af_sarah" => Voice::AfSarah(speed),
        "af_nicole" => Voice::AfNicole(speed),
        "af_alloy" => Voice::AfAlloy(speed),
        "am_puck" => Voice::AmPuck(speed),
        "am_eric" => Voice::AmEric(speed),
        "am_liam" => Voice::AmLiam(speed),
        "am_fenrir" => Voice::AmFenrir(speed),
        "am_onyx" => Voice::AmOnyx(speed),
        "bf_emma" => Voice::BfEmma(speed),
        "bf_alice" => Voice::BfAlice(speed),
        "bf_lily" => Voice::BfLily(speed),
        "bf_isabella" => Voice::BfIsabella(speed),
        "bm_daniel" => Voice::BmDaniel(speed),
        "bm_george" => Voice::BmGeorge(speed),
        "bm_fable" => Voice::BmFable(speed),
        "bm_lewis" => Voice::BmLewis(speed),
        _ => Voice::AfHeart(speed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_wav_produces_valid_wav_header() {
        let samples = vec![0.0f32; 500];
        let wav = encode_wav(&samples, 24_000).expect("encode_wav should succeed");
        let cursor = Cursor::new(wav);
        let reader = hound::WavReader::new(cursor).expect("valid WAV header");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 24_000);
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_format, SampleFormat::Float);
    }
}
