#[path = "../../agentix-jails/src/ax_jail/profile.rs"]
mod profile;
use anyhow::{Context, Result};
use std::{collections::HashMap, path::PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[tokio::test]
async fn real_jail_headless_streams_and_refuses_unapproved_edits() -> Result<()> {
    for edit in [false, true] {
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
        std::fs::copy(env!("CARGO_BIN_EXE_ax"), bin.join("ax"))?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = vec![0; 65536];
            let n = socket.read(&mut request).await?;
            anyhow::ensure!(n > 0, "request missing");
            let (delta, finish) = if edit {
                (
                    serde_json::json!({"tool_calls":[{"index":0,"id":"w","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"new\",\"content\":\"bad\",\"expected_sha256\":null}"}}]}),
                    "tool_calls",
                )
            } else {
                (serde_json::json!({"content":"Hello from the jail"}), "stop")
            };
            let wire = format!(
                "data: {}\n\ndata: [DONE]\n\n",
                serde_json::json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
            );
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{wire}",wire.len()).as_bytes()).await?;
            Ok::<(), anyhow::Error>(())
        });
        let profile = profile::Profile::build(profile::ProfileOptions {
            home,
            cwd: root.clone(),
            bin_dir: bin,
            nix_socket: PathBuf::from("/missing"),
            ro_paths: vec![],
            rw_paths: vec![],
            all_worktrees: false,
            environment: HashMap::new(),
        })?;
        assert_eq!(profile.root, root);
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            tokio::process::Command::new(std::env::var("AX_TEST_BWRAP")?)
                .args(&profile.args)
                .args([
                    "--",
                    "ax",
                    "--model",
                    "fixture",
                    "--gateway",
                    &url,
                    "--json",
                    "task",
                ])
                .output(),
        )
        .await??;
        server.await??;
        assert_eq!(
            output.status.code(),
            Some(if edit { 3 } else { 0 }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let events: Vec<serde_json::Value> = String::from_utf8(output.stdout)?
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        for (sequence, event) in events.iter().enumerate() {
            let envelope: agentix_api::agent::EventEnvelope =
                serde_json::from_value(event.clone())?;
            assert_eq!(envelope.sequence, sequence as u64);
        }
        assert_eq!(
            events.last().context("terminal event")?["type"],
            if edit {
                "turn_failed"
            } else {
                "turn_completed"
            }
        );
        assert!(!root.join("new").exists());
    }
    Ok(())
}
#[test]
fn direct_ax_refuses_to_enable_tools_without_launcher() -> Result<()> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ax"))
        .arg("task")
        .output()?;
    // This integration test is run outside ax-jail, which creates its own /run marker.
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ax-jail"));
    Ok(())
}

#[tokio::test]
async fn interrupted_headless_preserves_partial_events_and_exits_130() -> Result<()> {
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
    std::fs::copy(env!("CARGO_BIN_EXE_ax"), bin.join("ax"))?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let marker = root.join("connected");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut bytes = [0; 65536];
        anyhow::ensure!(socket.read(&mut bytes).await? > 0, "missing request");
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial answer\"},\"finish_reason\":null}]}\n\n").await?;
        // Give the stream consumer one tick before signaling this fixture's child.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        std::fs::write(marker, "")?;
        while socket.read(&mut bytes).await? > 0 {}
        Ok::<(), anyhow::Error>(())
    });
    let profile = profile::Profile::build(profile::ProfileOptions {
        home,
        cwd: root,
        bin_dir: bin,
        nix_socket: "/missing".into(),
        ro_paths: vec![],
        rw_paths: vec![],
        all_worktrees: false,
        environment: HashMap::new(),
    })?;
    let script = r#"ax --model fixture --gateway "$1" --json task & child=$!; while [ ! -e connected ]; do "$2" 0.01; done; kill -INT "$child"; wait "$child""#;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new(std::env::var("AX_TEST_BWRAP")?)
            .args(&profile.args)
            .args([
                "--",
                "/bin/sh",
                "-c",
                script,
                "fixture",
                &url,
                &format!("{}/sleep", std::env::var("AX_TEST_PATH")?),
            ])
            .output(),
    )
    .await??;
    server.await??;
    assert_eq!(
        output.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events: Vec<agentix_api::agent::EventEnvelope> = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    assert!(events
        .iter()
        .any(|e| matches!(e.event, agentix_api::agent::AgentEvent::TextDelta { .. })));
    assert!(matches!(
        events.last().context("terminal event")?.event,
        agentix_api::agent::AgentEvent::TurnCancelled {}
    ));
    Ok(())
}
