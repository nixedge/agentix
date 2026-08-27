use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::audio::{play_f32, AudioRecorder};
use crate::config::VoiceConfig;
use crate::vad::{Vad, VadResult};
use crate::wake::WakeDetector;
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
        let mut wake = WakeDetector::new(&self.config.wake_word)?;
        let recorder = AudioRecorder::new(SAMPLE_RATE);
        let (tx, mut rx) = mpsc::channel::<Vec<f32>>(128);

        tracing::info!(
            model = %self.config.wake_word.model,
            "Voice loop started; listening for wake word"
        );

        let _guard = recorder.start(tx)?;
        let mut vad = Vad::new(self.config.vad_threshold, self.config.vad_silence_ms);
        let mut recording_buf: Vec<f32> = Vec::new();

        // State: are we currently recording after wake word?
        let mut awake = false;
        // Accumulate samples into 1280-sample chunks for oww-rs
        let mut ww_buf: Vec<f32> = Vec::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!("Voice loop cancelled");
                    break;
                }
                chunk = rx.recv() => {
                    let Some(chunk) = chunk else { break };

                    if !awake {
                        // Feed samples into wake word detector in 1280-sample chunks
                        ww_buf.extend_from_slice(&chunk);
                        while ww_buf.len() >= WakeDetector::chunk_size() {
                            let oww_chunk: Vec<f32> =
                                ww_buf.drain(..WakeDetector::chunk_size()).collect();
                            if wake.process_chunk(oww_chunk) {
                                tracing::info!("Wake word detected — recording");
                                awake = true;
                                recording_buf.clear();
                                vad.reset();
                                ww_buf.clear();
                                break;
                            }
                        }
                    } else {
                        // Recording: run VAD on each chunk
                        let vad_result = vad.process(&chunk);
                        match vad_result {
                            VadResult::Speaking | VadResult::Silence => {
                                recording_buf.extend_from_slice(&chunk);
                            }
                            VadResult::EndOfUtterance => {
                                if !recording_buf.is_empty() {
                                    awake = false;
                                    let audio = std::mem::take(&mut recording_buf);
                                    vad.reset();
                                    tracing::info!(
                                        samples = audio.len(),
                                        "Utterance complete — transcribing"
                                    );
                                    if let Err(e) = self.handle_utterance(audio).await {
                                        tracing::warn!(error = %e, "utterance handling failed");
                                    }
                                    // Discard audio that accumulated during processing to
                                    // prevent false wake word triggers from the backlog.
                                    ww_buf.clear();
                                    while rx.try_recv().is_ok() {}
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
        let wav = agentix_tts::encode_wav(&samples, SAMPLE_RATE)?;

        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.config.stt_model.clone());

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
            .header("x-session-id", &session_id)
            .json(&serde_json::json!({ "message": text }))
            .send()
            .await?;

        let chat_body: serde_json::Value = chat_resp.json().await?;
        let response_text = chat_body["reply"]
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
            let speakable = tts_clean(&response_text);
            match tts.synthesize(&speakable).await {
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

/// Strip content that kokoro can't pronounce: bracketed annotations like
/// `[After checking the calendar]`, markdown, and leading/trailing whitespace.
fn tts_clean(text: &str) -> String {
    // Remove [...] blocks (tool-use annotations, markdown links, etc.)
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for ch in text.chars() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    // Collapse excess whitespace left by removed blocks
    let out = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    out
}
