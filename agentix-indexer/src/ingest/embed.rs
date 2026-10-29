use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::time::sleep;

static HTTP: OnceLock<Client> = OnceLock::new();

fn client() -> &'static Client {
    HTTP.get_or_init(Client::new)
}

const MAX_RETRIES: u32 = 5;
const INITIAL_BACKOFF: Duration = Duration::from_secs(2);

/// Embed a batch of texts via Ollama /api/embed.
/// Retries up to MAX_RETRIES times with exponential backoff on transient errors.
pub async fn embed_batch(texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let model = std::env::var("EMBED_MODEL")
        .unwrap_or_else(|_| "hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0".into());

    embed_batch_at(&host, &model, texts, INITIAL_BACKOFF).await
}

async fn embed_batch_at(
    host: &str,
    model: &str,
    texts: &[&str],
    initial_backoff: Duration,
) -> Result<Vec<Vec<f32>>> {
    let body = serde_json::json!({
        "model": model,
        "input": texts,
    });

    let mut last_err = anyhow!("no attempts made");
    let url = format!("{host}/api/embed");

    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let backoff = initial_backoff * 2u32.pow(attempt - 1);
            tracing::warn!(
                attempt,
                backoff_secs = backoff.as_secs(),
                "Ollama request failed, retrying..."
            );
            sleep(backoff).await;
        }

        let send_result = client()
            .post(&url)
            .json(&body)
            .timeout(Duration::from_secs(1800))
            .send()
            .await;

        let resp = match send_result {
            Ok(r) => r,
            Err(e) if e.is_timeout() || e.is_connect() => {
                last_err = anyhow!(e).context("Ollama request failed (is Ollama running?)");
                continue;
            }
            Err(e) => return Err(anyhow!(e).context("Ollama request failed (is Ollama running?)")),
        };

        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            let detail = detail.chars().take(1000).collect::<String>();
            let error = anyhow!("Embedding service returned HTTP {status}: {detail}");
            if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                if attempt < MAX_RETRIES {
                    eprintln!("{error}; retrying ({}/{MAX_RETRIES})", attempt + 1);
                }
                last_err = error;
                continue;
            }
            return Err(error);
        }

        let data: serde_json::Value = resp.json().await?;
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

        return Ok(embeddings);
    }

    Err(last_err.context(format!("Ollama request failed after {MAX_RETRIES} retries")))
}

/// Ensure the embed model is available locally, pulling it from the registry if needed.
/// Logs progress to stderr. Should be called once before starting ingest.
pub async fn ensure_embed_model() -> Result<()> {
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

/// Prepare bounded embedding batches before committing any file in the group.
pub async fn embed_records(
    mut records: Vec<agentix_store::Record>,
) -> Result<Vec<agentix_store::Record>> {
    let mut jobs = tokio::task::JoinSet::new();
    for offset in (0..records.len()).step_by(16) {
        let texts = records
            .iter()
            .skip(offset)
            .take(16)
            .map(|r| {
                r.row["content"]
                    .as_str()
                    .map(str::to_owned)
                    .context("missing content")
            })
            .collect::<Result<Vec<_>>>()?;
        jobs.spawn(async move {
            let refs = texts.iter().map(String::as_str).collect::<Vec<_>>();
            let vectors = embed_batch(&refs).await?;
            anyhow::ensure!(
                vectors.len() == texts.len(),
                "embedding service returned an incorrect batch length"
            );
            Ok::<_, anyhow::Error>((offset, vectors))
        });
        if jobs.len() >= 4 {
            let result = jobs
                .join_next()
                .await
                .context("missing embedding worker")?
                .context("embedding worker failed")??;
            apply_vectors(&mut records, result)?;
        }
    }
    while let Some(result) = jobs.join_next().await {
        apply_vectors(&mut records, result.context("embedding worker failed")??)?;
    }
    Ok(records)
}
fn apply_vectors(
    records: &mut [agentix_store::Record],
    (offset, vectors): (usize, Vec<Vec<f32>>),
) -> Result<()> {
    let target = records
        .get_mut(offset..offset + vectors.len())
        .context("embedding batch offset mismatch")?;
    for (record, vector) in target.iter_mut().zip(vectors) {
        record.embedding = Some(vector);
    }
    Ok(())
}

pub async fn flush_files(
    store: &agentix_store::Store,
    collection: &str,
    repo: &str,
    pending: &mut Vec<(String, Vec<agentix_store::Record>)>,
) -> Result<usize> {
    let mut files = Vec::new();
    let mut records = Vec::new();
    for (file, mut batch) in std::mem::take(pending) {
        files.push((file, batch.len()));
        records.append(&mut batch);
    }
    let records = embed_records(records).await?;
    for record in &records {
        store.validate_record(collection, record)?;
    }
    let count = records.len();
    let mut records = records.into_iter();
    for (file, size) in files {
        store
            .replace_file(
                collection,
                repo,
                &file,
                records.by_ref().take(size).collect(),
            )
            .await?;
    }
    Ok(count)
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use axum::{extract::State, http::StatusCode, routing::post, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Clone)]
    struct Responses {
        statuses: Arc<Vec<(StatusCode, &'static str)>>,
        calls: Arc<AtomicUsize>,
    }

    async fn exercise(
        statuses: Vec<(StatusCode, &'static str)>,
    ) -> Result<(Result<Vec<Vec<f32>>>, usize)> {
        let calls = Arc::new(AtomicUsize::new(0));
        let state = Responses {
            statuses: Arc::new(statuses),
            calls: calls.clone(),
        };
        let router = Router::new()
            .route(
                "/api/embed",
                post(|State(state): State<Responses>| async move {
                    let attempt = state.calls.fetch_add(1, Ordering::Relaxed);
                    state.statuses[attempt.min(state.statuses.len() - 1)]
                }),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let host = format!("http://{}", listener.local_addr()?);
        let task = tokio::spawn(async move { axum::serve(listener, router).await });
        let result = embed_batch_at(&host, "fixture", &["sample"], Duration::from_millis(1)).await;
        task.abort();
        Ok((result, calls.load(Ordering::Relaxed)))
    }

    #[tokio::test]
    async fn transient_embedding_statuses_retry_and_recover() -> Result<()> {
        let (result, calls) = exercise(vec![
            (StatusCode::BAD_GATEWAY, "upstream restarted"),
            (StatusCode::TOO_MANY_REQUESTS, "busy"),
            (StatusCode::OK, r#"{"embeddings":[[1.0,0.2,0.3,0.4]]}"#),
        ])
        .await?;
        assert_eq!(result?, vec![vec![1.0, 0.2, 0.3, 0.4]]);
        assert_eq!(calls, 3);
        Ok(())
    }

    #[tokio::test]
    async fn transient_embedding_retries_are_bounded_and_keep_error_detail() -> Result<()> {
        let (result, calls) = exercise(vec![(
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream unavailable",
        )])
        .await?;
        let error = format!("{:#}", result.err().context("expected failure")?);
        assert_eq!(calls, MAX_RETRIES as usize + 1);
        assert!(error.contains("503") && error.contains("upstream unavailable"));
        Ok(())
    }

    #[tokio::test]
    async fn permanent_embedding_errors_are_not_retried() -> Result<()> {
        let (result, calls) = exercise(vec![(StatusCode::BAD_REQUEST, "input too long")]).await?;
        let error = format!("{:#}", result.err().context("expected failure")?);
        assert_eq!(calls, 1);
        assert!(error.contains("400") && error.contains("input too long"));
        Ok(())
    }
}
