use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn fixture(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    Ok(std::env::var_os(name)
        .ok_or_else(|| format!("missing pinned fixture {name}"))?
        .into())
}
fn start(socket: &Path, log: &Path) -> Result<Worker, Box<dyn Error>> {
    let model = fixture("AGENTIX_QWEN_TEST_MODEL")?;
    let hash = format!("{:x}", Sha256::digest(std::fs::read(&model)?));
    Ok(Worker(
        Command::new(env!("CARGO_BIN_EXE_agentix-qwen-worker"))
            .env("AGENTIX_QWEN_COMPUTE", "cpu")
            .env("AGENTIX_QWEN_SOCKET", socket)
            .env("AGENTIX_QWEN_GGUF_SHARDS", model)
            .env("AGENTIX_QWEN_EXPECTED_SHARD_SHA256", hash)
            .env(
                "AGENTIX_QWEN_TOKENIZER",
                fixture("AGENTIX_QWEN_TOKENIZER_PATH")?,
            )
            .env(
                "AGENTIX_QWEN_CHAT_TEMPLATE",
                fixture("AGENTIX_QWEN_TEMPLATE_PATH")?,
            )
            .env("AGENTIX_QWEN_MODEL", "qwen3.8-flash-next")
            .env("AGENTIX_QWEN_CONTEXT", "256")
            .env("AGENTIX_QWEN_PREFILL_CHUNK", "8")
            .env("RAYON_NUM_THREADS", "2")
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(log)?)
            .spawn()?,
    ))
}
fn request(stream: bool, count: usize) -> Value {
    json!({"model":"qwen3.8-flash-next", "messages":[{"role":"user", "content":"Hi"}],
        "max_tokens":count, "temperature":0, "stream":stream})
}

#[tokio::test]
async fn real_library_worker_http_stream_reset_admission_and_disconnect(
) -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let socket = temp.path().join("qwen.sock");
    let log = temp.path().join("worker.log");
    let mut worker = start(&socket, &log)?;
    let client = reqwest::Client::builder()
        .unix_socket(socket.clone())
        .timeout(Duration::from_secs(60))
        .build()?;
    let ready = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(status) = worker.0.try_wait()? {
                return Err(
                    format!("worker exited {status}: {}", std::fs::read_to_string(&log)?).into(),
                );
            }
            if let Ok(response) = client.get("http://localhost/health").send().await {
                if response.status().is_success() {
                    return Ok::<_, Box<dyn Error>>(response.json::<Value>().await?);
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    assert_eq!(ready["engine"], "rustingstrata");
    let models: Value = client
        .get("http://localhost/v1/models")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(models["data"][0]["id"], "qwen3.8-flash-next");
    let endpoint = "http://localhost/v1/chat/completions";
    let first: Value = client
        .post(endpoint)
        .json(&request(false, 3))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(first["usage"]["completion_tokens"], 3);
    assert_eq!(first["choices"][0]["finish_reason"], "length");
    let text = first["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("missing content")?;
    assert!(!text.is_empty());
    let stream = client
        .post(endpoint)
        .json(&request(true, 3))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert!(stream.ends_with("data: [DONE]\n\n"), "{stream}");
    let mut streamed = String::new();
    for line in stream
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
    {
        if line == "[DONE]" {
            continue;
        }
        let chunk: Value = serde_json::from_str(line)?;
        assert!(chunk.get("error").is_none(), "{chunk}");
        if let Some(delta) = chunk["choices"][0]["delta"]["content"].as_str() {
            streamed.push_str(delta);
        }
    }
    assert_eq!(streamed, text);
    let again: Value = client
        .post(endpoint)
        .json(&request(false, 3))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(again["choices"], first["choices"]);
    let oversized = client
        .post(endpoint)
        .json(&request(false, 256))
        .send()
        .await?;
    assert_eq!(oversized.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        oversized.json::<Value>().await?["error"]["code"],
        "context_length_exceeded"
    );
    let mut unknown = request(false, 1);
    unknown["model"] = json!("missing");
    assert_eq!(
        client.post(endpoint).json(&unknown).send().await?.status(),
        reqwest::StatusCode::NOT_FOUND
    );
    // Stop reading a long SSE response. Backpressure keeps generation active while we probe busy.
    let mut active = client
        .post(endpoint)
        .json(&request(true, 128))
        .send()
        .await?
        .error_for_status()?;
    let _first_chunk = active
        .chunk()
        .await?
        .ok_or("stream ended before first chunk")?;
    let busy = client
        .post(endpoint)
        .json(&request(false, 1))
        .send()
        .await?;
    assert_eq!(busy.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(busy.json::<Value>().await?["error"]["code"], "busy");
    drop(active);
    let after_disconnect = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let response = client
                .post(endpoint)
                .json(&request(false, 1))
                .send()
                .await?;
            if response.status() != reqwest::StatusCode::SERVICE_UNAVAILABLE {
                return response.error_for_status()?.json::<Value>().await;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    assert_eq!(
        after_disconnect["choices"][0]["message"]["content"],
        &text[..text.len() / 3]
    );
    Ok(())
}
