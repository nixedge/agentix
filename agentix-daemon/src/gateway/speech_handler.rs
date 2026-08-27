use super::AppState;
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

#[derive(Deserialize)]
pub struct SpeechRequest {
    pub input: String,
    #[serde(default = "default_model")]
    pub model: String,
}

fn default_model() -> String {
    "en_US-lessac-medium".to_string()
}

pub async fn handler(
    State(state): State<AppState>,
    Json(req): Json<SpeechRequest>,
) -> impl IntoResponse {
    let model_path = state.piper_voices_dir.join(format!("{}.onnx", req.model));
    let json_path = state.piper_voices_dir.join(format!("{}.onnx.json", req.model));

    if !model_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("voice model not found: {}", model_path.display())
            })),
        )
        .into_response();
    }

    let sample_rate = read_sample_rate(&json_path).unwrap_or(22050);

    let mut child = match tokio::process::Command::new(&state.piper_bin)
        .args(["--model", &model_path.to_string_lossy(), "--output-raw"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("piper spawn failed: {e}")})),
            )
            .into_response();
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(req.input.as_bytes()).await;
        // dropping stdin signals EOF to piper
    }

    let output = match child.wait_with_output().await {
        Ok(o) => o,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("piper wait failed: {e}")})),
            )
            .into_response();
        }
    };

    if !output.status.success() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "piper synthesis failed"})),
        )
        .into_response();
    }

    match encode_wav_i16(&output.stdout, sample_rate) {
        Ok(wav) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "audio/wav")
            .body(axum::body::Body::from(wav))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("WAV encoding failed: {e}")})),
        )
        .into_response(),
    }
}

fn read_sample_rate(json_path: &std::path::Path) -> Option<u32> {
    let raw = std::fs::read_to_string(json_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&raw).ok()?;
    val["audio"]["sample_rate"].as_u64().map(|v| v as u32)
}

// piper --output-raw writes raw i16 PCM, little-endian, mono
fn encode_wav_i16(raw: &[u8], sample_rate: u32) -> anyhow::Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut buf = Vec::new();
    {
        let mut writer = hound::WavWriter::new(std::io::Cursor::new(&mut buf), spec)?;
        for chunk in raw.chunks_exact(2) {
            writer.write_sample(i16::from_le_bytes([chunk[0], chunk[1]]))?;
        }
        writer.finalize()?;
    }
    Ok(buf)
}
