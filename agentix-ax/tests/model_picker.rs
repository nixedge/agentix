use agentix_ax::tui::model_picker::{prepare_selection, ModelPicker};
use agentix_harness::client::GatewayClient;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn record(id: &str) -> Value {
    json!({"id":id,"display_name":id,"aliases":[],"sha256":"a".repeat(64),"byte_size":1234,
        "source":null,"format":"Gguf","architecture":"qwen2","context_limit":32768,
        "capabilities":["completion"],"coding_support":{"status":"unknown"},"installed":true,"resident":"unloaded"})
}
fn ready(id: &str) -> Value {
    json!({"schema_version":1,"model_id":id,"residency":"ready","coding_supported":true,"context_limit":32768})
}
async fn server(
    responses: Vec<(&'static str, u16, Value, u64)>,
) -> Result<(String, tokio::task::JoinHandle<Result<()>>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let handle = tokio::spawn(async move {
        for (path, status, response, delay) in responses {
            let (mut socket, _) = listener.accept().await?;
            let mut request = Vec::new();
            let (start, length) = loop {
                let mut chunk = [0; 4096];
                let n = socket.read(&mut chunk).await?;
                anyhow::ensure!(n > 0, "disconnected request");
                request.extend_from_slice(&chunk[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = std::str::from_utf8(&request[..end])?;
                    assert!(
                        header
                            .lines()
                            .next()
                            .context("request line")?
                            .contains(path),
                        "{header}"
                    );
                    let length = header
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_owned)
                        })
                        .unwrap_or_else(|| "0".into())
                        .parse::<usize>()?;
                    break (end + 4, length);
                }
            };
            while request.len() < start + length {
                let mut chunk = [0; 4096];
                let n = socket.read(&mut chunk).await?;
                anyhow::ensure!(n > 0, "disconnected body");
                request.extend_from_slice(&chunk[..n]);
            }
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let body = response.to_string();
            socket.write_all(format!("HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
        }
        Ok(())
    });
    Ok((url, handle))
}
#[tokio::test]
async fn selection_checks_readiness_and_conversation_capacity() -> Result<()> {
    for (residency, coding, context, expected) in [
        ("ready", true, 32768, true),
        ("busy", true, 32768, false),
        ("ready", false, 32768, false),
        ("ready", true, 4096, false),
    ] {
        let mut value = ready("qwen");
        value["residency"] = json!(residency);
        value["coding_supported"] = json!(coding);
        value["context_limit"] = json!(context);
        let mut replies = vec![("/readiness", 200, value, 0)];
        if residency == "ready" && coding {
            replies.push((
                "/tokenize",
                200,
                json!({"count":100,"context_limit":context}),
                0,
            ));
        }
        let (url, task) = server(replies).await?;
        let result = prepare_selection(&url, "qwen", "conversation").await;
        assert_eq!(result.is_ok(), expected, "{result:?}");
        task.await??;
    }
    for code in [404, 409, 500, 503] {
        let (url, task) = server(vec![("/readiness", code, json!({}), 0)]).await?;
        assert!(prepare_selection(&url, "qwen", "[]").await.is_err());
        task.await??;
    }
    Ok(())
}
#[tokio::test]
async fn catalog_paginates_without_fabricating_residency() -> Result<()> {
    let (url, task) = server(vec![
        (
            "limit=100",
            200,
            json!({"schema_version":1,"items":[record("a")],"next_cursor":"a"}),
            0,
        ),
        (
            "cursor=a",
            200,
            json!({"schema_version":1,"items":[record("b")],"next_cursor":null}),
            0,
        ),
    ])
    .await?;
    let models = GatewayClient::new(&format!("{url}/v1/"))?.models().await?;
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].resident, agentix_api::models::Residency::Unloaded);
    task.await??;
    Ok(())
}
async fn poll_catalog(picker: &mut ModelPicker) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(2), async {
        while picker.loading() {
            picker.poll();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}
#[tokio::test]
async fn cancelled_selection_cannot_overwrite_reopened_picker() -> Result<()> {
    let (old_url, old_task) = server(vec![
        (
            "/models?",
            200,
            json!({"schema_version":1,"items":[record("old")],"next_cursor":null}),
            0,
        ),
        ("/readiness", 200, ready("old"), 150),
        (
            "/tokenize",
            200,
            json!({"count":1,"context_limit":32768}),
            0,
        ),
    ])
    .await?;
    let mut picker = ModelPicker::open(old_url.clone());
    poll_catalog(&mut picker).await?;
    picker.choose(old_url, "[]".into());
    drop(picker);
    let (url, task) = server(vec![
        (
            "/models?",
            200,
            json!({"schema_version":1,"items":[record("new")],"next_cursor":null}),
            0,
        ),
        ("/readiness", 200, ready("new"), 0),
        (
            "/tokenize",
            200,
            json!({"count":1,"context_limit":32768}),
            0,
        ),
    ])
    .await?;
    let mut picker = ModelPicker::open(url.clone());
    poll_catalog(&mut picker).await?;
    picker.choose(url, "[]".into());
    let selected = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(ready) = picker.poll() {
                break ready;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    assert_eq!(selected.model_id.as_str(), "new");
    old_task.await??;
    task.await??;
    assert!(picker.poll().is_none());
    Ok(())
}
