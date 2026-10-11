use agentix_harness::{
    approval::Cancellation,
    upstream::{self, ProcessMode, ProcessRequest},
    AgentEvent, Tool,
};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tokio::sync::mpsc::UnboundedSender;
pub struct RunCommand {
    pub cwd: PathBuf,
    pub cancel: Cancellation,
    pub events: UnboundedSender<AgentEvent>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    command: String,
    #[serde(default)]
    stdin: Option<String>,
    #[serde(default = "timeout")]
    timeout_seconds: u64,
    #[serde(default)]
    tty: bool,
}
fn timeout() -> u64 {
    60
}
#[async_trait]
impl Tool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }
    fn description(&self) -> &str {
        "Run an approved shell command in the jailed project. Optional stdin is sent then closed; tty=true allocates an 80x24 terminal. Timeout is 1–300 seconds (default 60). Returns real exit code and up to 256 KiB of combined output; cancellation terminates the process group."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"command":{"type":"string"},"stdin":{"type":"string"},"tty":{"type":"boolean"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":300}},"required":["command"],"additionalProperties":false})
    }
    async fn call(&self, args: Value) -> Result<String> {
        let args: Args = serde_json::from_value(args)?;
        if args.command.len() > 32768
            || args.stdin.as_ref().is_some_and(|s| s.len() > 65536)
            || !(1..=300).contains(&args.timeout_seconds)
        {
            bail!("command exceeds input or time limits");
        }
        if self.cancel.is_cancelled() {
            return Err(agentix_harness::approval::Cancelled.into());
        }
        let program = std::env::var_os("AX_TEST_SHELL")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/bin/sh"));
        let mut child = upstream::spawn(&ProcessRequest {
            program,
            args: vec!["-c".into(), args.command],
            cwd: self.cwd.clone(),
            env: std::env::vars().collect(),
            mode: if args.tty {
                ProcessMode::Terminal(upstream::TerminalSize { rows: 24, cols: 80 })
            } else if args.stdin.is_some() {
                ProcessMode::Pipe
            } else {
                ProcessMode::ClosedStdin
            },
        })
        .await?;
        if let Some(input) = args.stdin {
            child
                .session
                .writer_sender()
                .send(input.into_bytes())
                .await
                .context("command stdin closed")?;
            child.session.close_stdin();
        }
        let deadline = tokio::time::sleep(Duration::from_secs(args.timeout_seconds));
        tokio::pin!(deadline);
        let mut output = Vec::new();
        let mut dropped = 0usize;
        let mut stdout_open = true;
        let mut stderr_open = true;
        let mut code = None;
        let mut stopped = None;
        let mut reap_deadline = None;
        while code.is_none() || stdout_open || stderr_open {
            tokio::select! {
             bytes=child.stdout_rx.recv(),if stdout_open=>match bytes {Some(bytes)=>append(&mut output,&mut dropped,&bytes,&self.events),None=>stdout_open=false},
             bytes=child.stderr_rx.recv(),if stderr_open=>match bytes {Some(bytes)=>append(&mut output,&mut dropped,&bytes,&self.events),None=>stderr_open=false},
             result=&mut child.exit_rx,if code.is_none()=>{code=Some(result.context("process exit waiter disconnected")?);},
             _=self.cancel.cancelled(),if stopped.is_none()=>{stopped=Some("cancelled");child.session.request_terminate();reap_deadline=Some(tokio::time::Instant::now()+Duration::from_secs(5));},
             _=&mut deadline,if stopped.is_none()=>{stopped=Some("timeout");child.session.request_terminate();reap_deadline=Some(tokio::time::Instant::now()+Duration::from_secs(5));},
             _=async {if let Some(at)=reap_deadline {tokio::time::sleep_until(at).await}else{std::future::pending::<()>().await}}=>bail!("process cleanup did not finish within 5 seconds"),
            }
        }
        Ok(json!({"exit_code":code,"output":String::from_utf8_lossy(&output),"truncated_bytes":dropped,"stopped":stopped}).to_string())
    }
}
fn append(
    output: &mut Vec<u8>,
    dropped: &mut usize,
    bytes: &[u8],
    events: &UnboundedSender<AgentEvent>,
) {
    let keep = bytes
        .len()
        .min((256 * 1024usize).saturating_sub(output.len()));
    output.extend_from_slice(&bytes[..keep]);
    *dropped = dropped.saturating_add(bytes.len() - keep);
    if keep > 0 {
        let _ = events.send(AgentEvent::ToolOutput {
            name: "run_command".into(),
            text: String::from_utf8_lossy(&bytes[..keep]).into_owned(),
        });
    }
}
