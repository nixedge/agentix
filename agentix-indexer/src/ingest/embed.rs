use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::time::sleep;

static HTTP: OnceLock<Client> = OnceLock::new();

fn client() -> &'static Client {
    HTTP.get_or_init(Client::new)
}

/// Active when LLAMACPP_HOST is set (non-empty) and OLLAMA_HOST is not - Ollama takes priority.
// NOTE: duplicated in agentix-search's `embed`; see the comment there for why it is not shared.
fn llamacpp_host() -> Option<String> {
    match (std::env::var("LLAMACPP_HOST"), std::env::var("OLLAMA_HOST")) {
        (Ok(host), Err(_)) if !host.is_empty() => Some(host),
        _ => None,
    }
}

const MAX_RETRIES: u32 = 5;
const INITIAL_BACKOFF: Duration = Duration::from_secs(2);

/// Shared retry loop for the embedding backends. POSTs `body` to `url`, retrying transient
/// (timeout/connect) failures up to MAX_RETRIES with exponential backoff, then hands the
/// decoded JSON response to `parse`. `label` names the backend in log/error messages
/// (e.g. "Ollama"); `daemon` names the process to check in the "is X running?" hint
/// (e.g. "llama-server"). The response shape lives entirely in `parse`.
async fn retry_embed(
    url: &str,
    body: &serde_json::Value,
    label: &str,
    daemon: &str,
    parse: impl Fn(serde_json::Value) -> Result<Vec<Vec<f32>>>,
) -> Result<Vec<Vec<f32>>> {
    let send_fail = format!("{label} request failed (is {daemon} running?)");
    let mut last_err = anyhow!("no attempts made");

    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let backoff = INITIAL_BACKOFF * 2u32.pow(attempt - 1);
            tracing::warn!(
                attempt,
                backoff_secs = backoff.as_secs(),
                "{label} request failed, retrying..."
            );
            sleep(backoff).await;
        }

        let send_result = client()
            .post(url)
            .json(body)
            .timeout(Duration::from_secs(1800))
            .send()
            .await;

        let resp = match send_result {
            Ok(r) => r,
            Err(e) if e.is_timeout() || e.is_connect() => {
                last_err = anyhow!(e).context(send_fail.clone());
                continue;
            }
            Err(e) => return Err(anyhow!(e).context(send_fail)),
        };

        if let Err(e) = resp.error_for_status_ref() {
            return Err(anyhow!(e).context(format!("{label} returned error status")));
        }

        let data: serde_json::Value = resp.json().await?;
        return parse(data);
    }

    Err(last_err.context(format!(
        "{label} request failed after {MAX_RETRIES} retries"
    )))
}

/// Decode an Ollama `/api/embed` response: `{ "embeddings": [[..], ..] }`.
fn parse_ollama_response(data: serde_json::Value) -> Result<Vec<Vec<f32>>> {
    let embeddings = data["embeddings"]
        .as_array()
        .context("missing 'embeddings' in Ollama response")?
        .iter()
        .map(|e| {
            e.as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect()
        })
        .collect();
    Ok(embeddings)
}

/// Decode an OpenAI-compatible llama.cpp `/v1/embeddings` response:
/// `{ "data": [{ "index": i, "embedding": [..] }, ..] }`, for `n` inputs.
fn parse_llamacpp_response(data: serde_json::Value, n: usize) -> Result<Vec<Vec<f32>>> {
    let items = data["data"]
        .as_array()
        .context("missing 'data' in llama.cpp response")?;

    // Items may arrive out of order; reconstruct by index field.
    let mut result = vec![vec![]; n];
    for item in items {
        let idx = item["index"].as_u64().unwrap_or(0) as usize;
        if idx < result.len() {
            result[idx] = item["embedding"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect();
        }
    }

    // Every input must map to exactly one non-empty embedding. A short response,
    // a missing/duplicate/out-of-range `index`, or a missing `embedding` field would
    // otherwise leave empty slots that callers silently turn into zero-vectors,
    // NULL-padded UNNEST rows, or zip-truncated inserts. Fail loudly instead.
    if items.len() != n {
        return Err(anyhow!(
            "llama.cpp returned {} embeddings for {n} inputs",
            items.len()
        ));
    }
    if let Some(i) = result.iter().position(|e| e.is_empty()) {
        return Err(anyhow!(
            "llama.cpp response has no embedding for input index {i}"
        ));
    }
    Ok(result)
}

async fn embed_batch_ollama(texts: &[&str], host: &str) -> Result<Vec<Vec<f32>>> {
    let model = std::env::var("EMBED_MODEL")
        .unwrap_or_else(|_| "hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0".into());
    let body = serde_json::json!({ "model": model, "input": texts });
    let url = format!("{host}/api/embed");

    retry_embed(&url, &body, "Ollama", "Ollama", parse_ollama_response).await
}

async fn embed_batch_llamacpp(texts: &[&str], host: &str) -> Result<Vec<Vec<f32>>> {
    let model = std::env::var("EMBED_MODEL").unwrap_or_else(|_| "jina-code-embeddings-1.5b".into());
    let body = serde_json::json!({ "model": model, "input": texts });
    let url = format!("{host}/v1/embeddings");
    let n = texts.len();

    retry_embed(&url, &body, "llama.cpp", "llama-server", move |data| {
        parse_llamacpp_response(data, n)
    })
    .await
}

/// Embed a batch of texts via the active backend (llama.cpp or Ollama).
/// Retries up to MAX_RETRIES times with exponential backoff on transient errors.
pub async fn embed_batch(texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    let result = if let Some(host) = llamacpp_host() {
        embed_batch_llamacpp(texts, &host).await?
    } else {
        let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
        embed_batch_ollama(texts, &host).await?
    };

    // Callers rely on a 1:1 correspondence between inputs and embeddings (zip inserts,
    // equal-length UNNEST arrays). A backend returning a different count would silently
    // drop or misalign rows, so enforce the invariant here for both backends.
    if result.len() != texts.len() {
        return Err(anyhow!(
            "embed backend returned {} embeddings for {} inputs",
            result.len(),
            texts.len()
        ));
    }

    Ok(result)
}

/// Ensure the embed model is available. For Ollama, pulls the model if not present.
/// For llama.cpp, probes the server and returns an error if unreachable.
/// Should be called once before starting ingest.
pub async fn ensure_embed_model() -> Result<()> {
    if let Some(host) = llamacpp_host() {
        return ensure_llamacpp(&host).await;
    }

    let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let model = std::env::var("EMBED_MODEL")
        .unwrap_or_else(|_| "hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0".into());

    // Probe with a minimal embed — fast if model is already loaded.
    let probe = client()
        .post(format!("{host}/api/embed"))
        .json(&serde_json::json!({"model": model, "input": [""]}))
        .timeout(Duration::from_secs(30))
        .send()
        .await;

    let needs_pull = match probe {
        Ok(r) if r.status().is_success() => return Ok(()), // already available
        Ok(r) if r.status().as_u16() == 404 => true,
        Ok(_) => true, // any other error — try pulling anyway
        Err(_) => return Err(anyhow!("Ollama unreachable at {host}")),
    };

    if needs_pull {
        eprintln!(
            "[agentic-nix] Embed model '{model}' not found locally — pulling from registry..."
        );

        // Stream the pull so we can log progress milestones.
        let mut resp = client()
            .post(format!("{host}/api/pull"))
            .json(&serde_json::json!({"model": model, "stream": true}))
            .timeout(Duration::from_secs(1800)) // 30 min — large model
            .send()
            .await
            .context("Ollama pull request failed")?;

        let mut last_status = String::new();
        let mut buf = Vec::new();

        while let Some(chunk) = resp.chunk().await? {
            buf.extend_from_slice(&chunk);
            // Each newline-delimited JSON object is one progress event.
            while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=nl).collect();
                if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&line) {
                    let status = val["status"].as_str().unwrap_or("").to_string();
                    if status != last_status {
                        if let (Some(completed), Some(total)) =
                            (val["completed"].as_u64(), val["total"].as_u64())
                        {
                            let pct = completed * 100 / total.max(1);
                            eprintln!("  {status}: {pct}%");
                        } else {
                            eprintln!("  {status}");
                        }
                        last_status = status;
                    }
                }
            }
        }

        eprintln!("[agentic-nix] Pull complete: {model}");
    }

    // Check whether the model landed on GPU.
    if let Ok(resp) = client()
        .get(format!("{host}/api/ps"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        if let Ok(data) = resp.json::<serde_json::Value>().await {
            let on_cpu = data["models"]
                .as_array()
                .map(|ms| {
                    ms.iter().any(|m| {
                        m["size"].as_u64().unwrap_or(0) > 0
                            && m["size_vram"].as_u64().unwrap_or(0) == 0
                    })
                })
                .unwrap_or(false);
            if on_cpu {
                eprintln!(
                    "WARNING: Ollama embed model is running on CPU (size_vram=0). \
                     Indexing will be slow. Check GPU/CUDA configuration."
                );
            }
        }
    }

    Ok(())
}

async fn ensure_llamacpp(host: &str) -> Result<()> {
    let model = std::env::var("EMBED_MODEL").unwrap_or_else(|_| "jina-code-embeddings-1.5b".into());
    // A non-empty placeholder: some llama-server builds reject an empty string with
    // "Input content cannot be empty" (400), which would otherwise fail this probe
    // even when the server is reachable and the model is loaded.
    client()
        .post(format!("{host}/v1/embeddings"))
        .json(&serde_json::json!({"model": model, "input": ["ping"]}))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .with_context(|| {
            format!("llama.cpp server unreachable at {host} (is llama-server running?)")
        })?
        .error_for_status()
        .context("llama.cpp server returned error (is the model loaded?)")?;
    Ok(())
}

/// Format a float vector for PostgreSQL vector literal.
pub fn vec_literal(v: &[f32]) -> String {
    format!(
        "[{}]",
        v.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llamacpp_host_selection() {
        // This is the only test that touches these env vars, so mutating the
        // process env sequentially here is safe. Save and restore regardless.
        let saved_llama = std::env::var("LLAMACPP_HOST").ok();
        let saved_ollama = std::env::var("OLLAMA_HOST").ok();

        std::env::remove_var("LLAMACPP_HOST");
        std::env::remove_var("OLLAMA_HOST");
        assert_eq!(llamacpp_host(), None, "neither set -> Ollama default");

        std::env::set_var("LLAMACPP_HOST", "http://127.0.0.1:8080");
        assert_eq!(
            llamacpp_host(),
            Some("http://127.0.0.1:8080".to_string()),
            "llama.cpp set, Ollama unset -> llama.cpp"
        );

        std::env::set_var("OLLAMA_HOST", "http://127.0.0.1:11434");
        assert_eq!(llamacpp_host(), None, "both set -> Ollama takes priority");

        std::env::remove_var("OLLAMA_HOST");
        std::env::set_var("LLAMACPP_HOST", "");
        assert_eq!(
            llamacpp_host(),
            None,
            "empty LLAMACPP_HOST -> treated as unset"
        );

        match saved_llama {
            Some(v) => std::env::set_var("LLAMACPP_HOST", v),
            None => std::env::remove_var("LLAMACPP_HOST"),
        }
        match saved_ollama {
            Some(v) => std::env::set_var("OLLAMA_HOST", v),
            None => std::env::remove_var("OLLAMA_HOST"),
        }
    }

    #[test]
    fn ollama_parse_extracts_vectors() {
        let data = serde_json::json!({ "embeddings": [[1.0, 2.0], [3.0, 4.0]] });
        let out = parse_ollama_response(data).unwrap();
        assert_eq!(out, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    }

    #[test]
    fn llamacpp_parse_in_order() {
        let data = serde_json::json!({
            "data": [
                { "index": 0, "embedding": [1.0, 2.0] },
                { "index": 1, "embedding": [3.0, 4.0] },
            ]
        });
        assert_eq!(
            parse_llamacpp_response(data, 2).unwrap(),
            vec![vec![1.0, 2.0], vec![3.0, 4.0]]
        );
    }

    #[test]
    fn llamacpp_parse_reorders_by_index() {
        let data = serde_json::json!({
            "data": [
                { "index": 1, "embedding": [3.0, 4.0] },
                { "index": 0, "embedding": [1.0, 2.0] },
            ]
        });
        assert_eq!(
            parse_llamacpp_response(data, 2).unwrap(),
            vec![vec![1.0, 2.0], vec![3.0, 4.0]]
        );
    }

    #[test]
    fn llamacpp_parse_rejects_short_response() {
        let data = serde_json::json!({ "data": [ { "index": 0, "embedding": [1.0] } ] });
        assert!(
            parse_llamacpp_response(data, 2).is_err(),
            "1 item for 2 inputs must error"
        );
    }

    #[test]
    fn llamacpp_parse_rejects_missing_embedding() {
        // Count matches (2 items, 2 inputs) but one item lacks an 'embedding' field.
        let data = serde_json::json!({
            "data": [
                { "index": 0, "embedding": [1.0, 2.0] },
                { "index": 1 },
            ]
        });
        assert!(
            parse_llamacpp_response(data, 2).is_err(),
            "missing embedding must error"
        );
    }

    #[test]
    fn llamacpp_parse_rejects_duplicate_index() {
        // Both items claim index 0, so slot 1 stays empty -> error.
        let data = serde_json::json!({
            "data": [
                { "index": 0, "embedding": [1.0, 2.0] },
                { "index": 0, "embedding": [3.0, 4.0] },
            ]
        });
        assert!(
            parse_llamacpp_response(data, 2).is_err(),
            "duplicate index must error"
        );
    }
}
