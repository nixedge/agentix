use anyhow::Result;
use tokio::sync::mpsc;

use crate::audio::{play_f32, AudioRecorder};
use crate::config::VoiceConfig;
use crate::vad::{Vad, VadResult};
use crate::wake::WakeDetector;

const SAMPLE_RATE: u32 = 16_000;

pub struct VoiceLoop {
    config: VoiceConfig,
    client: reqwest::Client,
}

impl VoiceLoop {
    pub async fn new(config: VoiceConfig) -> Result<Self> {
        Ok(Self {
            config,
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

        let mut awake = false;
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
                                    // Drain audio buffered during processing, then wait for
                                    // speaker echo to clear the mic before re-arming wake word.
                                    while rx.try_recv().is_ok() {}
                                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                    while rx.try_recv().is_ok() {}
                                    ww_buf.clear();
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
        let wav = encode_wav(&samples, SAMPLE_RATE)?;

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

        let speakable = tts_clean(&response_text);
        tracing::debug!(speakable = %speakable, "TTS input after cleaning");

        let tts_resp = self
            .client
            .post(format!("{daemon_url}/v1/audio/speech"))
            .json(&serde_json::json!({
                "input": speakable,
                "model": self.config.tts_voice
            }))
            .send()
            .await?;

        if !tts_resp.status().is_success() {
            tracing::warn!(status = %tts_resp.status(), "TTS request failed");
            return Ok(());
        }

        let wav_bytes = tts_resp.bytes().await?;
        let (pcm, sample_rate) = decode_wav(&wav_bytes)?;

        tokio::task::spawn_blocking(move || {
            if let Err(e) = play_f32(pcm, sample_rate) {
                tracing::warn!(error = %e, "playback failed");
            }
        })
        .await?;

        Ok(())
    }
}

fn encode_wav(samples: &[f32], sample_rate: u32) -> anyhow::Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut buf = Vec::new();
    {
        let mut writer = hound::WavWriter::new(std::io::Cursor::new(&mut buf), spec)?;
        for &s in samples {
            writer.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
        }
        writer.finalize()?;
    }
    Ok(buf)
}

fn decode_wav(bytes: &[u8]) -> anyhow::Result<(Vec<f32>, u32)> {
    let cursor = std::io::Cursor::new(bytes);
    let mut reader = hound::WavReader::new(cursor)?;
    let sample_rate = reader.spec().sample_rate;
    let samples = match reader.spec().sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32768.0))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()?,
    };
    Ok((samples, sample_rate))
}

fn tts_clean(text: &str) -> String {
    let mut lines_out: Vec<&str> = Vec::new();
    let mut in_code_block = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block || trimmed.is_empty() {
            continue;
        }
        lines_out.push(trimmed);
    }

    let joined = lines_out.join(" ");

    let mut out = String::with_capacity(joined.len());
    let mut depth = 0usize;
    for ch in joined.chars() {
        match ch {
            '[' | '{' => depth += 1,
            ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }

    let out = out.replace('`', "").replace("**", "").replace("__", "");
    out.split_whitespace()
        .filter(|w| {
            let looks_like_date = w.len() == 10
                && w.chars().filter(|c| *c == '-').count() == 2
                && w.chars().all(|c| c.is_ascii_digit() || c == '-');
            let looks_like_time = w.len() <= 8
                && w.chars().filter(|c| *c == ':').count() == 1
                && w.chars().all(|c| c.is_ascii_digit() || c == ':');
            !looks_like_date && !looks_like_time && *w != "-" && *w != "*"
        })
        .collect::<Vec<_>>()
        .join(" ")
}
