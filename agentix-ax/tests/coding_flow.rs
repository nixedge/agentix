use agentix_api::agent::{ApprovalDecision, ApprovalResponse};
use agentix_ax::{
    controller::Controller,
    tools::access::{digest, FileAccess},
};
use agentix_harness::{
    approval::{ApprovalRequired, Cancellation},
    AgentEvent,
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
fn calls(id: &str, name: &str, args: Value) -> String {
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]},"finish_reason":"tool_calls"}]})
    )
}
fn answer(text: &str) -> String {
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":"stop"}]})
    )
}
async fn server(
    replies: Vec<String>,
) -> Result<(
    String,
    Arc<Mutex<Vec<Value>>>,
    tokio::task::JoinHandle<Result<()>>,
)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let requests = Arc::new(Mutex::new(vec![]));
    let captured = requests.clone();
    let task = tokio::spawn(async move {
        let mut replies: VecDeque<_> = replies.into();
        while let Some(reply) = replies.pop_front() {
            let (mut socket, _) = listener.accept().await?;
            let mut bytes = Vec::new();
            let (start, length) = loop {
                let mut chunk = [0; 4096];
                let n = socket.read(&mut chunk).await?;
                anyhow::ensure!(n > 0, "request disconnected");
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                    let header = std::str::from_utf8(&bytes[..end])?;
                    let length = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_owned)
                        })
                        .context("length")?
                        .parse::<usize>()?;
                    break (end + 4, length);
                }
            };
            while bytes.len() < start + length {
                let mut chunk = [0; 4096];
                let n = socket.read(&mut chunk).await?;
                anyhow::ensure!(n > 0, "body disconnected");
                bytes.extend_from_slice(&chunk[..n]);
            }
            captured
                .lock()
                .map_err(|_| anyhow::anyhow!("lock"))?
                .push(serde_json::from_slice(&bytes[start..start + length])?);
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",reply.len()).as_bytes()).await?;
        }
        Ok(())
    });
    Ok((url, requests, task))
}
fn controller(root: &std::path::Path, url: String) -> Result<Controller> {
    Ok(Controller::new(
        "fixture-local".into(),
        url,
        FileAccess::new(root.into(), vec![root.into()], vec![root.into()])?,
    ))
}
#[tokio::test]
async fn approved_read_search_patch_test_and_followup_keep_matching_history() -> Result<()> {
    let root = tempfile::tempdir()?;
    std::fs::write(root.path().join("example.txt"), "old\n")?;
    let (url,requests,server)=server(vec![calls("r","read_file",json!({"path":"example.txt"})),calls("s","file_search",json!({"query":"example"})),calls("p","apply_patch",json!({"patch":"*** Begin Patch\n*** Update File: example.txt\n@@\n-old\n+new\n*** End Patch","expected_sha256":{"example.txt":digest("old\n")}})),calls("t","run_command",json!({"command":"test \"$(cat example.txt)\" = new"})),answer("Changed example.txt; test exited 0."),answer("The previous test exited 0.")]).await?;
    let controller = controller(root.path(), url)?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let agent = controller.build(Cancellation::default(), tx.clone(), true)?;
    let run = tokio::spawn(async move { agent.run_with_events("change and test", tx).await });
    let mut approvals = 0;
    while let Some(event) = rx.recv().await {
        if let AgentEvent::ApprovalRequired { request } = event {
            approvals += 1;
            controller.approvals.respond(ApprovalResponse {
                request_id: request.request_id,
                context: request.context,
                arguments_hash: request.arguments_hash,
                decision: ApprovalDecision::Allow,
            })?;
        }
    }
    let result = run.await??;
    assert_eq!(result.tool_calls_made, 4);
    assert_eq!(approvals, 2);
    assert_eq!(
        std::fs::read_to_string(root.path().join("example.txt"))?,
        "new\n"
    );
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    controller
        .build(Cancellation::default(), tx, false)?
        .run("what was the test result?")
        .await?;
    server.await??;
    let requests = requests.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let messages = requests[5]["messages"].as_array().context("messages")?;
    assert!(messages.iter().any(|m| m["role"] == "tool"
        && m["tool_call_id"] == "t"
        && m["content"]
            .as_str()
            .is_some_and(|s| s.contains("\"exit_code\":0"))));
    assert!(messages
        .iter()
        .any(|m| m["content"] == "Changed example.txt; test exited 0."));
    Ok(())
}
#[tokio::test]
async fn headless_edit_requires_approval_and_broken_stream_never_edits() -> Result<()> {
    for truncated in [false, true] {
        let root = tempfile::tempdir()?;
        let mut wire = calls(
            "w",
            "write_file",
            json!({"path":"new","content":"bad","expected_sha256":null}),
        );
        if truncated {
            wire = wire.replace("data: [DONE]\n\n", "");
        }
        let (url, _, server) = server(vec![wire]).await?;
        let controller = controller(root.path(), url)?;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let result = controller
            .build(Cancellation::default(), tx, false)?
            .run("edit")
            .await;
        let error = result.err().context("expected rejection")?;
        if !truncated {
            assert!(error.is::<ApprovalRequired>());
        }
        assert!(!root.path().join("new").exists());
        server.await??;
    }
    Ok(())
}
#[tokio::test]
async fn denied_edit_returns_matching_result_without_writing() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (url, requests, server) = server(vec![
        calls(
            "w",
            "write_file",
            json!({"path":"new","content":"bad","expected_sha256":null}),
        ),
        answer("Edit was denied; nothing changed."),
    ])
    .await?;
    let controller = controller(root.path(), url)?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let agent = controller.build(Cancellation::default(), tx.clone(), true)?;
    let run = tokio::spawn(async move { agent.run_with_events("edit", tx).await });
    while let Some(event) = rx.recv().await {
        if let AgentEvent::ApprovalRequired { request } = event {
            controller.approvals.respond(ApprovalResponse {
                request_id: request.request_id,
                context: request.context,
                arguments_hash: request.arguments_hash,
                decision: ApprovalDecision::Deny,
            })?;
        }
    }
    assert_eq!(run.await??.tool_calls_made, 0);
    assert!(!root.path().join("new").exists());
    server.await??;
    let requests = requests.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    assert!(requests[1]["messages"]
        .as_array()
        .context("messages")?
        .iter()
        .any(|m| m["tool_call_id"] == "w"));
    Ok(())
}
