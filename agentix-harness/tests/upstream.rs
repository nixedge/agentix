use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Duration;

use agentix_harness::upstream::{self, ProcessMode, ProcessRequest, TerminalSize};
use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio::time::timeout;

#[test]
fn patch_preserves_line_endings_and_rejects_stale_content() -> Result<()> {
    let patch = upstream::parse_patch(
        "*** Begin Patch\n*** Update File: example.rs\n@@\n-old\n+new\n*** End Patch",
    )?;
    let [upstream::Hunk::UpdateFile { chunks, .. }] = patch.hunks.as_slice() else {
        anyhow::bail!("expected update hunk");
    };
    assert_eq!(
        upstream::update_text("example.rs", "old\r\n", "old\r\n", chunks)?,
        "new\r\n"
    );
    assert!(upstream::update_text("example.rs", "old\n", "user edit\n", chunks).is_err());
    assert!(upstream::update_text("example.rs", "different\n", "different\n", chunks).is_err());
    assert!(
        upstream::parse_patch("*** Begin Patch\n*** Update File: example.rs\n@@\n-old\n+new")
            .is_err()
    );
    assert!(upstream::parse_patch(
        "*** Begin Patch\n*** Environment ID: remote\n*** Add File: a\n+x\n*** End Patch"
    )
    .is_err());
    Ok(())
}

#[test]
fn policy_uses_upstream_rules_and_prompts_for_unmatched_commands() -> Result<()> {
    use upstream::{CommandDecision, CommandPolicy};
    let policy = CommandPolicy::parse(
        "operator.rules",
        r#"
prefix_rule(pattern=["git", "status"], decision="allow")
prefix_rule(pattern=["git", "push"], decision="forbidden")
prefix_rule(pattern=["cargo", "test"], decision="prompt")
"#,
    )?;
    let argv = |args: &[&str]| args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        policy.check(&argv(&["git", "status", "--short"])),
        CommandDecision::Allow
    );
    assert_eq!(
        policy.check(&argv(&["git", "push"])),
        CommandDecision::Forbidden
    );
    assert_eq!(
        policy.check(&argv(&["cargo", "test"])),
        CommandDecision::Prompt
    );
    assert_eq!(
        policy.check(&argv(&["bash", "-c", "git status"])),
        CommandDecision::Prompt
    );
    assert_eq!(policy.check(&[]), CommandDecision::Forbidden);
    assert!(CommandPolicy::parse("invalid.rules", "unknown_rule()").is_err());
    Ok(())
}

fn request(script: &str, mode: ProcessMode) -> Result<ProcessRequest> {
    Ok(ProcessRequest {
        program: PathBuf::from(std::env::var("AX_TEST_SHELL").context("run through nix develop")?),
        args: vec![
            "--noprofile".into(),
            "--norc".into(),
            "-c".into(),
            script.into(),
        ],
        cwd: std::env::temp_dir(),
        env: HashMap::from([
            ("PATH".into(), std::env::var("AX_TEST_PATH")?),
            ("AX_MARKER".into(), "explicit".into()),
        ]),
        mode,
    })
}

async fn collect(mut rx: mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    let mut output = Vec::new();
    while let Some(chunk) = rx.recv().await {
        output.extend(chunk);
    }
    output
}

#[tokio::test]
async fn pipe_preserves_streams_exit_and_explicit_environment() -> Result<()> {
    let process = upstream::spawn(&request(
        "printf '%s:%s' \"$AX_MARKER\" \"${HOME-unset}\"; printf error >&2; exit 7",
        ProcessMode::ClosedStdin,
    )?)
    .await?;
    let (stdout, stderr, exit) = timeout(Duration::from_secs(10), async {
        tokio::join!(
            collect(process.stdout_rx),
            collect(process.stderr_rx),
            process.exit_rx
        )
    })
    .await?;
    assert_eq!(stdout, b"explicit:unset");
    assert_eq!(stderr, b"error");
    assert_eq!(exit?, 7);
    assert!(process.session.has_exited());
    Ok(())
}

#[tokio::test]
async fn pipe_accepts_input_and_closes_stdin() -> Result<()> {
    let process = upstream::spawn(&request(
        "IFS= read -r line; printf 'received:%s' \"$line\"; if read -r extra; then exit 8; fi",
        ProcessMode::Pipe,
    )?)
    .await?;
    process
        .session
        .writer_sender()
        .send(b"hello\n".to_vec())
        .await?;
    // Observe consumption before closing: close_stdin may abort queued writes.
    let mut stdout = process.stdout_rx;
    let first = timeout(Duration::from_secs(10), stdout.recv())
        .await?
        .context("missing output")?;
    assert_eq!(first, b"received:hello");
    process.session.close_stdin();
    assert_eq!(timeout(Duration::from_secs(10), process.exit_rx).await??, 0);
    Ok(())
}

#[tokio::test]
async fn terminal_supports_input_resize_and_exit() -> Result<()> {
    let process = upstream::spawn(&request(
        "IFS= read -r line; printf 'terminal:%s\n' \"$line\"; exit 3",
        ProcessMode::Terminal(TerminalSize { rows: 24, cols: 80 }),
    )?)
    .await?;
    process.session.resize(TerminalSize {
        rows: 40,
        cols: 120,
    })?;
    process
        .session
        .writer_sender()
        .send(b"hello\n".to_vec())
        .await?;
    let (stdout, stderr, exit) = timeout(Duration::from_secs(10), async {
        tokio::join!(
            collect(process.stdout_rx),
            collect(process.stderr_rx),
            process.exit_rx
        )
    })
    .await?;
    assert!(String::from_utf8(stdout)?.contains("terminal:hello"));
    assert!(stderr.is_empty());
    assert_eq!(exit?, 3);
    Ok(())
}

#[tokio::test]
async fn cancellation_stops_descendants_holding_output_open() -> Result<()> {
    for mode in [
        ProcessMode::Pipe,
        ProcessMode::Terminal(TerminalSize::default()),
    ] {
        let process = upstream::spawn(&request("sleep 60 & printf ready; wait", mode)?).await?;
        let mut stdout = process.stdout_rx;
        let ready = timeout(Duration::from_secs(10), stdout.recv())
            .await?
            .context("missing readiness")?;
        assert!(String::from_utf8(ready)?.contains("ready"));
        process.session.request_terminate();
        let (_, _, exit) = timeout(Duration::from_secs(10), async {
            tokio::join!(collect(stdout), collect(process.stderr_rx), process.exit_rx)
        })
        .await?;
        assert_ne!(exit?, 0);
        assert!(process.session.has_exited());
    }
    Ok(())
}

#[tokio::test]
async fn search_matches_files_and_preserves_upstream_symlink_behavior() -> Result<()> {
    let root = tempfile::tempdir()?;
    let external = tempfile::tempdir()?;
    std::fs::write(root.path().join("local_unique.rs"), "local")?;
    std::fs::write(external.path().join("linked_unique.rs"), "linked")?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(external.path(), root.path().join("linked"))?;
    let results = timeout(
        Duration::from_secs(10),
        upstream::search_files(
            root.path(),
            "unique",
            NonZeroUsize::new(10).context("nonzero limit")?,
            Arc::new(AtomicBool::new(false)),
        ),
    )
    .await??;
    assert!(results
        .matches
        .iter()
        .any(|m| m.path.ends_with("local_unique.rs")));
    #[cfg(unix)]
    assert!(results
        .matches
        .iter()
        .any(|m| m.path.ends_with("linked_unique.rs")));
    assert!(results.matches.len() <= 10);
    Ok(())
}

#[tokio::test]
async fn invalid_requests_do_not_spawn() -> Result<()> {
    let mut req = request("exit 0", ProcessMode::ClosedStdin)?;
    req.program = "relative-shell".into();
    assert!(upstream::spawn(&req).await.is_err());
    let req = request(
        "exit 0",
        ProcessMode::Terminal(TerminalSize { rows: 0, cols: 80 }),
    )?;
    assert!(upstream::spawn(&req).await.is_err());
    Ok(())
}
