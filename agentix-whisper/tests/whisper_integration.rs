//! Integration test for WhisperBackend.
//!
//! Requires the AGENTIX_TEST_WHISPER_MODEL_PATH environment variable to be set
//! to the path of a whisper model file (ggml-tiny.en.bin or similar).
//! Explicitly ignored; supply the fixture and select this test for acceptance.

use agentix_infer::{InferConfig, InferEngine};
use agentix_whisper::WhisperBackend;
use std::sync::Arc;

fn make_sine_wav(sample_rate: u32, duration_secs: f32) -> Vec<u8> {
    let freq_hz = 440.0f32;
    let n_samples = (sample_rate as f32 * duration_secs) as u32;
    let data_bytes = n_samples * 2;

    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    for i in 0..n_samples {
        let t = i as f32 / sample_rate as f32;
        let s = (2.0 * std::f32::consts::PI * freq_hz * t).sin();
        let sample = (s * i16::MAX as f32) as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

#[tokio::test]
#[ignore = "requires AGENTIX_TEST_WHISPER_MODEL_PATH to point to a whisper model"]
async fn whisper_transcribes_sine_wave() -> Result<(), Box<dyn std::error::Error>> {
    let model_path = match std::env::var("AGENTIX_TEST_WHISPER_MODEL_PATH") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("AGENTIX_TEST_WHISPER_MODEL_PATH not set — skipping");
            return Ok(());
        }
    };

    let tmp = tempfile::tempdir()?;
    let cfg = InferConfig::new(tmp.path().to_path_buf(), None, 2, 0);
    let engine = InferEngine::new(cfg).await?;
    engine.register_backend(Arc::new(WhisperBackend));

    // Register the local whisper model
    engine.pull(&model_path).await?;
    let model_name = std::path::Path::new(&model_path)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("model path has no UTF-8 filename")?;

    // Synthesize 3 seconds of silence (sine wave — whisper may produce empty or noise text)
    let wav = make_sine_wav(16_000, 3.0);
    let pcm = agentix_whisper::decode_audio_to_pcm(wav).await?;

    let result = engine.transcribe_pcm(model_name, &pcm).await?;
    // Not checking the actual text — a sine wave may produce empty or noise text.
    println!("transcript: {:?}", result);
    Ok(())
}
