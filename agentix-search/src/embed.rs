use anyhow::{Context, Result};
use reqwest::Client;
use std::sync::OnceLock;
use std::time::Duration;

static HTTP: OnceLock<Client> = OnceLock::new();

fn client() -> &'static Client {
    HTTP.get_or_init(Client::new)
}

/// Timeout for query embedding. Must be shorter than the MCP client's request timeout
/// so we can return a descriptive error rather than a generic -32001 timeout.
/// Overridable via EMBED_TIMEOUT_SECS env var.
/// Active when LLAMACPP_HOST is set (non-empty) and OLLAMA_HOST is not - Ollama takes priority.
// NOTE: duplicated in agentix-indexer's ingest::embed. `agentix-indexer` does not depend on
// `agentix-search`, so there is no shared crate to host it. Keep the two copies in sync.
pub fn llamacpp_host() -> Option<String> {
    match (std::env::var("LLAMACPP_HOST"), std::env::var("OLLAMA_HOST")) {
        (Ok(host), Err(_)) if !host.is_empty() => Some(host),
        _ => None,
    }
}

fn embed_timeout() -> Duration {
    let secs = std::env::var("EMBED_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20u64);
    Duration::from_secs(secs)
}

async fn embed_ollama(text: &str, host: &str) -> Result<Vec<f32>> {
    let model = std::env::var("EMBED_MODEL")
        .unwrap_or_else(|_| "hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0".into());
    tracing::debug!(model = %model, host = %host, "Embedding query via Ollama");
    let resp: serde_json::Value = client()
        .post(format!("{host}/api/embed"))
        .json(&serde_json::json!({ "model": model, "input": [text] }))
        .timeout(embed_timeout())
        .send()
        .await
        .with_context(|| {
            format!("Ollama unreachable at {host} (is Ollama running? model: {model})")
        })?
        .error_for_status()
        .with_context(|| format!("Ollama returned error status (model: {model})"))?
        .json()
        .await
        .context("Failed to parse Ollama embed response")?;
    resp["embeddings"][0]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no embeddings in Ollama response"))
        .map(|arr| {
            arr.iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect()
        })
}

async fn embed_llamacpp(text: &str, host: &str) -> Result<Vec<f32>> {
    let model = std::env::var("EMBED_MODEL").unwrap_or_else(|_| "jina-code-embeddings-1.5b".into());
    tracing::debug!(model = %model, host = %host, "Embedding query via llama.cpp");
    let resp: serde_json::Value = client()
        .post(format!("{host}/v1/embeddings"))
        .json(&serde_json::json!({ "model": model, "input": [text] }))
        .timeout(embed_timeout())
        .send()
        .await
        .with_context(|| {
            format!("llama.cpp server unreachable at {host} (is llama-server running?)")
        })?
        .error_for_status()
        .with_context(|| format!("llama.cpp server returned error status (model: {model})"))?
        .json()
        .await
        .context("Failed to parse llama.cpp embed response")?;
    resp["data"][0]["embedding"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no embedding in llama.cpp response"))
        .map(|arr| {
            arr.iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect()
        })
}

pub async fn embed(text: &str) -> Result<Vec<f32>> {
    if let Some(host) = llamacpp_host() {
        embed_llamacpp(text, &host).await
    } else {
        let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
        embed_ollama(text, &host).await
    }
}
