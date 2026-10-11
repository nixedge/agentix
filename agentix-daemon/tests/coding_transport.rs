use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{process::Stdio, time::Duration};
#[tokio::test]
async fn real_daemon_proxies_fixture_generation_and_readiness_to_real_worker() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let socket = temp.path().join("llama.sock");
    let daemon_path = std::env::var_os("AGENTIX_TEST_DAEMON_BIN")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_BIN_EXE_agentix-daemon")));
    let worker_path = std::env::var_os("AGENTIX_TEST_LLAMA_BIN")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| daemon_path.with_file_name("agentix-llama"));
    anyhow::ensure!(
        worker_path.is_file(),
        "build agentix-llama before this transport test"
    );
    let mut worker = tokio::process::Command::new(worker_path)
        .env("AGENTIX_LLAMA_SOCKET", &socket)
        .env("AGENTIX_MODELS_DIR", temp.path().join("models"))
        .env(
            "AGENTIX_LLAMA_MODELS",
            std::env::var("AGENTIX_TEST_GENERATION_MODEL_PATH")?,
        )
        .env("AGENTIX_GPU_LAYERS", "0")
        .env("AGENTIX_MAX_CTX", "256")
        .env("RUST_LOG", "error")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let reserve = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = reserve.local_addr()?.port();
    drop(reserve);
    let mut daemon = tokio::process::Command::new(&daemon_path)
        .env("AGENTIX_GATEWAY_HOST", "127.0.0.1")
        .env("AGENTIX_GATEWAY_PORT", port.to_string())
        .env("AGENTIX_LLAMA_SOCKET", &socket)
        .env("AGENTIX_WHISPER_SOCKET", temp.path().join("none"))
        .env("AGENTIX_ALLOW_CLOUD_ESCALATION", "0")
        .env("RUST_LOG", "error")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()?;
    let base = format!("http://127.0.0.1:{port}");
    let model = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(response) = client.get(format!("{base}/v1/models")).send().await {
                if let Ok(value) = response.json::<Value>().await {
                    if let Some(model) = value["data"][0]["id"].as_str() {
                        break Ok::<_, anyhow::Error>(model.to_owned());
                    }
                }
            }
            anyhow::ensure!(
                worker.try_wait()?.is_none() && daemon.try_wait()?.is_none(),
                "fixture service exited early"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    let readiness: agentix_api::models::ModelReadiness = client
        .post(format!("{base}/v1/agentix/models/readiness"))
        .json(&json!({"schema_version":1,"model_id":model,"load":true}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(!readiness.coding_supported);
    let catalog: agentix_api::models::ModelCatalog = client
        .get(format!("{base}/v1/agentix/models?limit=1"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(catalog.items.len(), 1);
    assert_eq!(catalog.items[0].id.as_str(), model);
    assert!(catalog.items[0].installed);
    assert_eq!(
        catalog.items[0].resident,
        agentix_api::models::Residency::Ready
    );
    assert!(matches!(
        catalog.items[0].coding_support,
        agentix_api::models::CodingSupport::Unsupported { .. }
    ));
    assert_eq!(
        catalog.items[0].sha256.as_str(),
        "6151b1929d7f5aa3385d9ddef3393e55587c0a55de661562322bc51dfda93a04"
    );
    assert!(catalog.next_cursor.is_none());
    assert_eq!(
        client
            .get(format!("{base}/v1/agentix/models?limit=0"))
            .send()
            .await?
            .status(),
        reqwest::StatusCode::BAD_REQUEST
    );
    let request = json!({"model":model,"messages":[{"role":"user","content":"Once upon a time"}],"max_tokens":8,"temperature":0,"stream":true});
    let response = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&request)
        .send()
        .await?
        .error_for_status()?;
    assert!(response
        .headers()
        .get("content-type")
        .context("SSE type")?
        .to_str()?
        .starts_with("text/event-stream"));
    let text = response.text().await?;
    assert!(text.contains("[DONE]"));
    assert!(text.contains("\"finish_reason\":\"length\""));
    let rejected = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({"model":"gpt-4o","messages":[]}))
        .send()
        .await?;
    assert_eq!(rejected.status(), reqwest::StatusCode::FORBIDDEN);
    daemon.kill().await?;
    daemon.wait().await?;
    worker.kill().await?;
    worker.wait().await?;
    Ok(())
}
