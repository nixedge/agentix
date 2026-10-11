#[path = "../src/tools/shell.rs"]
mod shell;
use agentix_harness::{approval::Cancellation, AgentEvent, Tool};
use anyhow::{Context, Result};
use serde_json::{json, Value};
#[tokio::test]
async fn stdin_is_delivered_exit_is_real_and_output_is_bounded() -> Result<()> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let tool = shell::RunCommand {
        cwd: std::env::temp_dir(),
        cancel: Cancellation::default(),
        events: tx,
    };
    let output: Value = serde_json::from_str(
        &tool
            .call(json!({"command":"cat; exit 7","stdin":"hello\n"}))
            .await?,
    )?;
    assert_eq!(output["exit_code"], 7);
    assert_eq!(output["output"], "hello\n");
    let output: Value = serde_json::from_str(
        &tool
            .call(json!({"command":"head -c 300000 /dev/zero"}))
            .await?,
    )?;
    assert_eq!(
        output["output"].as_str().context("output")?.len(),
        256 * 1024
    );
    assert_eq!(output["truncated_bytes"], 300000 - 256 * 1024);
    Ok(())
}
#[tokio::test]
async fn cancellation_reaps_parent_and_stops_descendants() -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = Cancellation::default();
    let tool = shell::RunCommand {
        cwd: std::env::temp_dir(),
        cancel: cancel.clone(),
        events: tx,
    };
    let task = tokio::spawn(async move {
        tool.call(json!({"command":"sleep 60 & printf ready; wait"}))
            .await
    });
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::ToolOutput { .. }) {
            break;
        }
    }
    cancel.cancel();
    let output: Value = serde_json::from_str(
        &tokio::time::timeout(std::time::Duration::from_secs(6), task).await???,
    )?;
    assert_eq!(output["stopped"], "cancelled");
    assert_ne!(output["exit_code"], 0);
    Ok(())
}

#[tokio::test]
async fn terminal_mode_and_timeout_keep_real_process_results() -> Result<()> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let tool = shell::RunCommand {
        cwd: std::env::temp_dir(),
        cancel: Cancellation::default(),
        events: tx,
    };
    let terminal: Value = serde_json::from_str(
        &tool
            .call(json!({"command":"if test -t 0 && test -t 1; then exit 9; else exit 8; fi","tty":true}))
            .await?,
    )?;
    assert_eq!(terminal["exit_code"], 9);
    let timed: Value = serde_json::from_str(
        &tool
            .call(json!({"command":"sleep 60 & wait","timeout_seconds":1}))
            .await?,
    )?;
    assert_eq!(timed["stopped"], "timeout");
    assert_ne!(timed["exit_code"], 0);
    Ok(())
}
