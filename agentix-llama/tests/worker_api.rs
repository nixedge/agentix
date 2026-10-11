use agentix_infer::store::ModelStore;
use serde_json::{json, Value};
use std::{process::Stdio, time::Duration};

#[tokio::test]
async fn actual_worker_generates_tokenizes_and_cancels_over_uds(
) -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let store_path = temp.path().join("models");
    let info = ModelStore::new(store_path.clone())
        .pull(&std::env::var("AGENTIX_TEST_GENERATION_MODEL_PATH")?)?;
    let socket = temp.path().join("llama.sock");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentix-llama"))
        .env("AGENTIX_LLAMA_SOCKET", &socket)
        .env("AGENTIX_MODELS_DIR", &store_path)
        .env("AGENTIX_GPU_LAYERS", "0")
        .env("AGENTIX_MAX_CTX", "512")
        .env("AGENTIX_LLAMA_MODELS", "")
        .env("RUST_LOG", "error")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .unix_socket(socket)
        .timeout(Duration::from_secs(15))
        .build()?;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Ok(response) = client.get("http://localhost/v1/models").send().await {
                if response.status().is_success() {
                    return Ok::<_, Box<dyn std::error::Error>>(());
                }
            }
            if child.try_wait()?.is_some() {
                return Err("worker exited before readiness".into());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    let readiness = json!({"schema_version":1,"model_id":info.name,"load":true});
    let response = client
        .post("http://localhost/control/readiness")
        .json(&readiness)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    assert_eq!(response["residency"], "ready");
    assert_eq!(response["coding_supported"], false);
    let context_limit = response["context_limit"]
        .as_u64()
        .ok_or("missing context limit")?;
    assert!((256..=512).contains(&context_limit));
    let response = client
        .post("http://localhost/control/tokenize")
        .json(&json!({"schema_version":1,"model_id":info.name,"text":"Once upon a time"}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    assert!(response["count"].as_u64().is_some_and(|n| n > 0));
    let request = json!({"model":info.name,"messages":[{"role":"user","content":"Once upon a time"}],"max_tokens":12,"temperature":0});
    let response = client
        .post("http://localhost/v1/chat/completions")
        .json(&request)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    assert!(!response["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("missing generated text")?
        .is_empty());
    assert_eq!(response["choices"][0]["finish_reason"], "length");
    let mut tools_request = request.clone();
    tools_request["tools"] =
        json!([{"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}}]);
    let response = client
        .post("http://localhost/v1/chat/completions")
        .json(&tools_request)
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let mut streaming = request;
    streaming["stream"] = json!(true);
    streaming["max_tokens"] = json!(context_limit - 32);
    let mut response = client
        .post("http://localhost/v1/chat/completions")
        .json(&streaming)
        .send()
        .await?
        .error_for_status()?;
    assert!(response.chunk().await?.is_some());
    drop(response);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response = client
                .post("http://localhost/control/readiness")
                .json(&readiness)
                .send()
                .await?
                .error_for_status()?
                .json::<Value>()
                .await?;
            if response["residency"] == "ready" {
                return Ok::<_, reqwest::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    client
        .post("http://localhost/control/shutdown")
        .send()
        .await?
        .error_for_status()?;
    assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await??
        .success());
    Ok(())
}
