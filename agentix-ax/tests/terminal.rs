#[path = "../../agentix-jails/src/ax_jail/profile.rs"]
mod profile;
use agentix_harness::upstream::{self, ProcessMode, ProcessRequest, TerminalSize};
use anyhow::{Context, Result};
use std::{collections::HashMap, path::PathBuf, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[tokio::test]
async fn real_terminal_accepts_unicode_approval_followup_and_restores_screen() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(25), scenario(false, false)).await?
}
#[tokio::test]
async fn simultaneous_resize_and_approval_key_remain_responsive() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(25), scenario(true, false)).await?
}
#[tokio::test]
async fn model_picker_loads_selection_then_runs_approved_coding_turn() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(25), scenario(true, true)).await?
}
async fn scenario(resize: bool, picker: bool) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let root = home.join("project");
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(&root)?;
    std::fs::create_dir_all(&bin)?;
    for (name, source) in [
        ("bash", std::env::var("AX_TEST_SHELL")?),
        ("git", std::env::var("AX_TEST_GIT")?),
        ("env", format!("{}/env", std::env::var("AX_TEST_PATH")?)),
    ] {
        std::os::unix::fs::symlink(source, bin.join(name))?;
    }
    let ax_binary = std::env::var_os("AX_TEST_AX_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_ax")));
    std::fs::copy(ax_binary, bin.join("ax"))?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        let mut captured = Vec::new();
        for request_index in 0..(if picker { 6 } else { 3 }) {
            let (mut socket, _) = listener.accept().await?;
            let mut request = Vec::new();
            let (start, length) = loop {
                let mut chunk = [0; 4096];
                let n = socket.read(&mut chunk).await?;
                anyhow::ensure!(n > 0, "request disconnected");
                request.extend_from_slice(&chunk[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = std::str::from_utf8(&request[..end])?;
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
                anyhow::ensure!(n > 0, "request disconnected");
                request.extend_from_slice(&chunk[..n]);
            }
            if picker && request_index < 3 {
                let value = match request_index {
                    0 => {
                        serde_json::json!({"schema_version":1,"items":[{"id":"fixture","display_name":"fixture","aliases":[],"sha256":"a".repeat(64),"byte_size":1234,"source":null,"format":"Gguf","architecture":"qwen2","context_limit":32768,"capabilities":["completion"],"coding_support":{"status":"unknown"},"installed":true,"resident":"unloaded"}],"next_cursor":null})
                    }
                    1 => {
                        serde_json::json!({"schema_version":1,"model_id":"fixture","residency":"ready","coding_supported":true,"context_limit":32768})
                    }
                    _ => serde_json::json!({"count":1,"context_limit":32768}),
                };
                let body = value.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await?;
                continue;
            }
            let round = request_index - if picker { 3 } else { 0 };
            captured.push(serde_json::from_slice::<serde_json::Value>(
                &request[start..start + length],
            )?);
            let (delta, finish) = if round == 0 {
                (
                    serde_json::json!({"tool_calls":[{"index":0,"id":"w","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"new.txt\",\"content\":\"approved\",\"expected_sha256\":null}"}}]}),
                    "tool_calls",
                )
            } else {
                (
                    serde_json::json!({"content":if round==1{"checkpoint complete"}else{"followup complete"}}),
                    "stop",
                )
            };
            let wire = format!(
                "data: {}\n\ndata: [DONE]\n\n",
                serde_json::json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
            );
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{wire}",wire.len()).as_bytes()).await?;
        }
        Ok::<_, anyhow::Error>(captured)
    });
    let profile = profile::Profile::build(profile::ProfileOptions {
        home,
        cwd: root.clone(),
        bin_dir: bin,
        nix_socket: PathBuf::from("/missing"),
        ro_paths: vec![],
        rw_paths: vec![],
        all_worktrees: false,
        environment: HashMap::from([
            ("TERM".into(), "xterm-256color".into()),
            ("AGENTIX_GATEWAY_URL".into(), url),
        ]),
    })?;
    assert_eq!(profile.root, root);
    let mut args = profile
        .args
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    args.extend(["--".into(), "ax".into()]);
    if !picker {
        args.extend(["--model".into(), "fixture".into()]);
    }
    let mut child = upstream::spawn(&ProcessRequest {
        program: std::env::var("AX_TEST_BWRAP")?.into(),
        args,
        cwd: root.clone(),
        env: std::env::vars().collect(),
        mode: ProcessMode::Terminal(TerminalSize {
            rows: 40,
            cols: 120,
        }),
    })
    .await?;
    if picker {
        wait_for(&mut child.stdout_rx, "Unloaded").await?;
        child
            .session
            .writer_sender()
            .send(b"fixt\r".to_vec())
            .await?;
        wait_for(&mut child.stdout_rx, "Selected fixture").await?;
    } else {
        wait_for(&mut child.stdout_rx, "Idle").await?;
    }
    // Bracketed paste plus UTF-8 cursor movement/backspace exercises byte boundaries.
    child
        .session
        .writer_sender()
        .send("\x1b[200~hé🦀\x1b[201~\x1b[D\x7f\r".as_bytes().to_vec())
        .await?;
    wait_for(&mut child.stdout_rx, "approve").await?;
    assert!(!root.join("new.txt").exists());
    if resize {
        child.session.resize(TerminalSize { rows: 30, cols: 80 })?;
    }
    child.session.writer_sender().send(b"y".to_vec()).await?;
    wait_for(&mut child.stdout_rx, "checkpoint complete").await?;
    assert_eq!(std::fs::read_to_string(root.join("new.txt"))?, "approved");
    tokio::time::sleep(Duration::from_millis(150)).await;
    child
        .session
        .writer_sender()
        .send(b"what changed?\r".to_vec())
        .await?;
    wait_for(&mut child.stdout_rx, "followup complete").await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    child.session.writer_sender().send(vec![3]).await?;
    let mut tail = Vec::new();
    while let Some(bytes) = child.stdout_rx.recv().await {
        tail.extend(bytes);
    }
    assert_eq!(child.exit_rx.await?, 0);
    assert!(tail.windows(8).any(|w| w == b"\x1b[?1049l"));
    let captured = server.await??;
    let messages = captured[0]["messages"].as_array().context("messages")?;
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(
        messages
            .iter()
            .find(|m| m["role"] == "user")
            .context("user message")?["content"],
        "h🦀"
    );
    assert!(captured[2]["messages"]
        .as_array()
        .context("messages")?
        .iter()
        .any(|m| m["tool_call_id"] == "w"));
    Ok(())
}
async fn wait_for(rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>, needle: &str) -> Result<()> {
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while let Some(bytes) = tokio::time::timeout_at(deadline, rx.recv())
        .await
        .with_context(|| format!("waiting for {needle}; received {text:?}"))?
    {
        text.push_str(&String::from_utf8_lossy(&bytes));
        if text.contains(needle) {
            return Ok(());
        }
        anyhow::ensure!(
            text.len() < 1024 * 1024,
            "terminal output exceeded fixture limit"
        );
    }
    anyhow::bail!("terminal closed before {needle}: {text}")
}
