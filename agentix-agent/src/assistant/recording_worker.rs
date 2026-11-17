use agentix_assistant_core::StoredRecording;
use agentix_assistant_sled::RecordingArchive;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio_util::sync::CancellationToken;

struct Scratch(Option<tempfile::TempDir>);
impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(directory) = self.0.take() {
            tokio::task::spawn_blocking(move || drop(directory));
        }
    }
}

pub(super) fn spawn(archive: RecordingArchive, gateway: String, cancellation: CancellationToken) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = cancellation.cancelled() => break,
                _ = interval.tick() => {
                    let Ok(pending) = archive.pending(5).await else { continue; };
                    for recording in pending {
                        tokio::select! {
                            _ = cancellation.cancelled() => return,
                            result = process(&archive, &gateway, recording) => {
                                if result.is_err() { tracing::warn!("recording transcription unavailable; original retained for durable retry"); }
                            }
                        }
                    }
                }
            }
        }
    });
}

pub(super) async fn process(
    archive: &RecordingArchive,
    gateway: &str,
    recording: StoredRecording,
) -> anyhow::Result<()> {
    archive.begin_attempt(recording.metadata.id).await?;
    let result = async {
        let audio = archive.audio_path(recording.metadata.id).await?;
        let text = transcribe(gateway, audio, recording.metadata.duration_ms).await?;
        archive
            .finish_transcript(
                recording.metadata.id,
                text,
                "local-whisper/gateway-default".into(),
            )
            .await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        archive.defer_attempt(recording.metadata.id).await?;
    }
    result
}

pub(super) async fn transcribe(
    gateway: &str,
    audio: PathBuf,
    duration_ms: u64,
) -> anyhow::Result<String> {
    let temporary = tokio::task::spawn_blocking(tempfile::tempdir).await??;
    let directory = temporary.path().to_path_buf();
    let _scratch = Scratch(Some(temporary));
    let binary = std::env::var_os("AGENTIX_RECORDING_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    let mut command = tokio::process::Command::new(binary);
    command
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(audio)
        .args([
            "-map",
            "0:a:0",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            "-f",
            "segment",
            "-segment_time",
            "30",
            "-segment_format",
            "wav",
        ])
        .arg(directory.join("part-%06d.wav"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(120), command.status()).await??;
    anyhow::ensure!(status.success(), "local recording decode failed");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()?;
    let url = format!("{}/v1/audio/transcriptions", gateway.trim_end_matches('/'));
    let count = duration_ms.div_ceil(30_000);
    anyhow::ensure!(
        (1..=480).contains(&count),
        "recording duration exceeds transcription limit"
    );
    let mut text = String::new();
    for index in 0..count {
        let chunk = tokio::fs::read(directory.join(format!("part-{index:06}.wav"))).await?;
        anyhow::ensure!(
            chunk.len() <= 1_100_000,
            "decoded recording chunk exceeds limit"
        );
        let part = reqwest::multipart::Part::bytes(chunk)
            .file_name("recording.wav")
            .mime_str("audio/wav")?;
        let mut response = client
            .post(&url)
            .multipart(
                reqwest::multipart::Form::new()
                    .part("file", part)
                    .text("response_format", "json"),
            )
            .send()
            .await?
            .error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= 131_072,
                "transcription response exceeds limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let json: serde_json::Value = serde_json::from_slice(&bytes)?;
        let segment = json["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid local transcription response"))?
            .trim();
        anyhow::ensure!(
            text.len() + segment.len() < 2 * 1024 * 1024,
            "transcript exceeds limit"
        );
        if !text.is_empty() && !segment.is_empty() {
            text.push(' ');
        }
        text.push_str(segment);
    }
    Ok(text)
}
