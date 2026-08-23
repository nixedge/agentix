use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::audio::{play_f32, AudioRecorder};
use crate::config::VoiceConfig;
use crate::vad::{Vad, VadResult};
use agentix_tts::TtsEngine;

const SAMPLE_RATE: u32 = 16_000;

pub struct VoiceLoop {
    config: VoiceConfig,
    tts: Option<Arc<TtsEngine>>,
    client: reqwest::Client,
}

impl VoiceLoop {
    pub async fn new(config: VoiceConfig) -> Result<Self> {
        let tts = match agentix_tts::TtsEngine::new(
            &config.tts.model_path,
            &config.tts.voices_dir,
            &config.tts.voice,
            config.tts.max_text_len,
        )
        .await
        {
            Ok(e) => {
                tracing::info!(voice = %config.tts.voice, "TTS engine initialized");
                Some(Arc::new(e))
            }
            Err(e) => {
                tracing::warn!(error = %e, "TTS unavailable; responses will be text-only");
                None
            }
        };

        Ok(Self {
            config,
            tts,
            client: reqwest::Client::new(),
        })
    }

    /// Run the voice loop until the cancellation token fires.
    pub async fn run(&self, cancel: tokio_util::sync::CancellationToken) -> Result<()> {
        let recorder = AudioRecorder::new(SAMPLE_RATE);
        let (tx, mut rx) = mpsc::channel::<Vec<f32>>(64);

        tracing::info!(
            "Voice loop started; wake word detection is passive (button-triggered in this build)"
        );

        // Without a wake word detector, we run in push-to-talk mode:
        // the loop waits for a trigger on a oneshot channel that can be
        // sent from a future UI or HTTP endpoint. For now, just demonstrate
        // the VAD→STT→chat→TTS pipeline by starting to record immediately.
        let _guard = recorder.start(tx)?;
        let mut vad = Vad::new(self.config.vad_threshold, self.config.vad_silence_ms);
        let mut recording_buf: Vec<f32> = Vec::new();
        let mut recording = false;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!("Voice loop cancelled");
                    break;
                }
                chunk = rx.recv() => {
                    let Some(chunk) = chunk else { break };
                    let vad_result = vad.process(&chunk);

                    match vad_result {
                        VadResult::Speaking => {
                            if !recording {
                                tracing::info!("Speech detected — recording");
                                recording = true;
                            }
                            recording_buf.extend_from_slice(&chunk);
                        }
                        VadResult::Silence => {
                            if recording {
                                recording_buf.extend_from_slice(&chunk);
                            }
                        }
                        VadResult::EndOfUtterance => {
                            if recording && !recording_buf.is_empty() {
                                recording = false;
                                let audio = std::mem::take(&mut recording_buf);
                                vad.reset();
                                tracing::info!(samples = audio.len(), "Utterance complete — transcribing");
                                if let Err(e) = self.handle_utterance(audio).await {
                                    tracing::warn!(error = %e, "utterance handling failed");
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_utterance(&self, samples: Vec<f32>) -> Result<()> {
        let wav = f32_to_wav(&samples, SAMPLE_RATE)?;

        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")?;
        let form = reqwest::multipart::Form::new().part("file", part);

        let daemon_url = &self.config.daemon_url;
        let resp = self
            .client
            .post(format!("{daemon_url}/v1/audio/transcriptions"))
            .multipart(form)
            .send()
            .await?;

        let body: serde_json::Value = resp.json().await?;
        let text = body["text"].as_str().unwrap_or("").trim().to_owned();

        if text.is_empty() {
            tracing::debug!("empty transcription, skipping");
            return Ok(());
        }

        tracing::info!(text = %text, "Transcription ready — sending to agent");

        let agent_url = &self.config.agent_url;
        let session_id = self
            .config
            .session_id
            .clone()
            .unwrap_or_else(|| "voice".to_string());

        let chat_resp = self
            .client
            .post(format!("{agent_url}/chat"))
            .json(&serde_json::json!({
                "message": text,
                "session_id": session_id
            }))
            .send()
            .await?;

        let chat_body: serde_json::Value = chat_resp.json().await?;
        let response_text = chat_body["response"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_owned();

        if response_text.is_empty() {
            tracing::warn!("empty agent response");
            return Ok(());
        }

        tracing::info!(response = %response_text, "Agent response received");

        if let Some(tts) = &self.tts {
            match tts.synthesize(&response_text).await {
                Ok(pcm) => {
                    let sr = tts.sample_rate();
                    tokio::task::spawn_blocking(move || {
                        if let Err(e) = play_f32(pcm, sr) {
                            tracing::warn!(error = %e, "playback failed");
                        }
                    })
                    .await?;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "TTS synthesis failed");
                }
            }
        }

        Ok(())
    }
}

fn f32_to_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    use hound::{SampleFormat, WavSpec, WavWriter};
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut writer = WavWriter::new(&mut buf, spec)?;
    for &s in samples {
        writer.write_sample(s)?;
    }
    writer.finalize()?;
    Ok(buf.into_inner())
}
