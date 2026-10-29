use super::AppState;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;
use std::path::PathBuf;

const PIPER_VOICES_BASE: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main";

#[derive(Deserialize)]
pub struct PullRequest {
    pub model: String,
}

/// GET /v1/audio/voices — list installed piper voice models.
pub async fn list_handler(State(state): State<AppState>) -> impl IntoResponse {
    let dir = &state.piper_voices_dir;
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("reading voices dir: {e}")})),
            )
                .into_response();
        }
    };

    let mut voices: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name();
            let s = name.to_string_lossy();
            s.ends_with(".onnx")
                .then(|| s.trim_end_matches(".onnx").to_owned())
        })
        .collect();
    voices.sort();

    Json(serde_json::json!({"voices": voices})).into_response()
}

/// POST /v1/audio/voices — download a piper voice model from HuggingFace.
///
/// Body: `{"model": "en_US-lessac-medium"}`
///
/// Model name format: `{lang_REGION}-{voice}-{quality}`
/// Maps to: `{lang}/{lang_REGION}/{voice}/{quality}/` on the voices repo.
pub async fn pull_handler(
    State(state): State<AppState>,
    Json(req): Json<PullRequest>,
) -> impl IntoResponse {
    let model = req.model.trim().to_owned();

    let url_dir = match model_to_url_dir(&model) {
        Some(d) => d,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!(
                        "cannot parse model name '{model}'; expected format: lang_REGION-voice-quality (e.g. en_US-lessac-medium)"
                    )
                })),
            )
            .into_response();
        }
    };

    let voices_dir = state.piper_voices_dir.clone();
    let client = state.http.clone();

    match download_voice(&client, &voices_dir, &model, &url_dir).await {
        Ok(()) => Json(serde_json::json!({"status": "ok", "model": model})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("download failed: {e}")})),
        )
            .into_response(),
    }
}

/// Parse `en_US-lessac-medium` → `en/en_US/lessac/medium`
fn model_to_url_dir(model: &str) -> Option<String> {
    // Split on '-' but the region code contains '_', not '-'.
    // Format is: {lang_REGION}-{voice}-{quality}
    // Example:   en_US-lessac-medium  →  parts = ["en_US", "lessac", "medium"]
    let parts: Vec<&str> = model.splitn(3, '-').collect();
    if parts.len() != 3 {
        return None;
    }
    let lang_region = parts[0]; // e.g. "en_US"
    let voice = parts[1]; // e.g. "lessac"
    let quality = parts[2]; // e.g. "medium"

    let lang = lang_region.split('_').next()?; // e.g. "en"
    Some(format!("{lang}/{lang_region}/{voice}/{quality}"))
}

async fn download_voice(
    client: &reqwest::Client,
    voices_dir: &PathBuf,
    model: &str,
    url_dir: &str,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(voices_dir)?;

    for ext in &[".onnx", ".onnx.json"] {
        let filename = format!("{model}{ext}");
        let url = format!("{PIPER_VOICES_BASE}/{url_dir}/{filename}");
        let dest = voices_dir.join(&filename);

        if dest.exists() {
            tracing::info!(file = %filename, "voice file already present, skipping");
            continue;
        }

        tracing::info!(url = %url, dest = %dest.display(), "downloading piper voice file");

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("GET {url}: {e}"))?;

        if !resp.status().is_success() {
            anyhow::bail!("GET {url} returned {}", resp.status());
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("reading {url}: {e}"))?;

        std::fs::write(&dest, &bytes)
            .map_err(|e| anyhow::anyhow!("writing {}: {e}", dest.display()))?;

        tracing::info!(file = %filename, bytes = bytes.len(), "downloaded");
    }

    Ok(())
}
