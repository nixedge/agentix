use clap::Parser;
use serde::Serialize;
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    error::Error,
    ffi::OsString,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const PINNED_TASK_SHA256: &str = "423e6a8de7187fd20fdc4e7e80ce1f17b4997759f57a3a7ad7e187c1e24297c3";

#[derive(Debug, Parser)]
#[command(about = "Run the pinned ten-task coding benchmark in disposable Git worktrees")]
struct Args {
    /// Repository whose history contains the pinned benchmark baseline.
    #[arg(long)]
    repo: PathBuf,
    /// Full commit ID recorded in benchmarks.md.
    #[arg(long, default_value = "13f2a5cb52ebc801eab154b009b0ab01f19e7a6a")]
    baseline: String,
    /// Frozen task fixture Markdown file.
    #[arg(
        long,
        default_value = "specs/021-strata-inference-research/coding-task-fixtures.md"
    )]
    tasks: PathBuf,
    /// Engine executable. It receives the task prompt on stdin and runs with the task worktree as cwd.
    #[arg(long)]
    engine: PathBuf,
    /// Additional argument passed to the engine. Repeat as needed.
    #[arg(long = "engine-arg")]
    engine_args: Vec<OsString>,
    /// Human-readable engine and version label included in the result manifest.
    #[arg(long)]
    engine_label: String,
    /// Directory for transcripts, diffs and machine-readable results.
    #[arg(long)]
    output: PathBuf,
    /// Maximum wall time for each engine and acceptance command.
    #[arg(long, default_value_t = 3600)]
    timeout_seconds: u64,
    /// Run only this task number (1 through 10).
    #[arg(long)]
    task: Option<u8>,
}

#[derive(Clone, Debug)]
struct Task {
    id: u8,
    title: String,
    prompt: String,
    acceptance: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CommandResult {
    command: String,
    exit_code: Option<i32>,
    success: bool,
    timed_out: bool,
    elapsed_ms: u128,
    stdout_file: String,
    stderr_file: String,
}

#[derive(Debug, Serialize)]
struct TaskResult {
    task: u8,
    title: String,
    baseline: String,
    engine_exit_code: Option<i32>,
    engine_committed: bool,
    engine_timed_out: bool,
    engine_elapsed_ms: u128,
    acceptance: Vec<CommandResult>,
    all_acceptance_passed: bool,
    worktree_status: String,
    diff_file: String,
    diff_review_required: bool,
}

#[derive(Debug, Serialize)]
struct RunSummary {
    task_fixture_sha256: String,
    baseline: String,
    engine: String,
    engine_label: String,
    engine_arguments: Vec<String>,
    timeout_seconds: u64,
    tasks: Vec<TaskResult>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    if args.timeout_seconds == 0 {
        return Err("timeout must be greater than zero".into());
    }
    let repo = args.repo.canonicalize()?;
    let task_path = if args.tasks.is_absolute() {
        args.tasks.clone()
    } else {
        repo.join(&args.tasks)
    };
    let task_bytes = fs::read(&task_path)?;
    let task_hash = format!("{:x}", Sha256::digest(&task_bytes));
    if task_hash != PINNED_TASK_SHA256 {
        return Err(format!(
            "task fixture hash mismatch: expected {PINNED_TASK_SHA256}, got {task_hash}"
        )
        .into());
    }
    let text = std::str::from_utf8(&task_bytes)?;
    let tasks = parse_tasks(text)?;
    if tasks.len() != 10 {
        return Err(format!("expected 10 frozen tasks, found {}", tasks.len()).into());
    }
    let selected = tasks
        .into_iter()
        .filter(|task| match args.task {
            Some(id) => task.id == id,
            None => true,
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err("requested task number is not in the frozen suite".into());
    }

    let baseline = git_output(
        &repo,
        [
            "rev-parse",
            "--verify",
            &format!("{}^{{commit}}", args.baseline),
        ],
    )?;
    let baseline = baseline.trim().to_owned();
    fs::create_dir_all(&args.output)?;
    let output = args.output.canonicalize()?;
    if fs::read_dir(&output)?.next().is_some() {
        return Err("output directory must be empty".into());
    }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let run_root =
        std::env::temp_dir().join(format!("agentix-bench-{}-{nonce}", std::process::id()));
    fs::create_dir(&run_root)?;
    let _run_root_guard = RunRootGuard(run_root.clone());

    let mut results = Vec::new();
    for task in selected {
        results.push(run_task(
            &repo,
            &baseline,
            &run_root,
            &output,
            &args.engine,
            &args.engine_args,
            &task,
            Duration::from_secs(args.timeout_seconds),
        )?);
    }
    let summary = RunSummary {
        task_fixture_sha256: task_hash,
        baseline,
        engine: args.engine.display().to_string(),
        engine_label: args.engine_label,
        engine_arguments: redact_arguments(&args.engine_args),
        timeout_seconds: args.timeout_seconds,
        tasks: results,
    };
    let summary_path = output.join("summary.json");
    fs::write(&summary_path, serde_json::to_vec_pretty(&summary)?)?;
    println!("{}", summary_path.display());
    Ok(())
}

fn parse_tasks(markdown: &str) -> Result<Vec<Task>, Box<dyn Error>> {
    let mut tasks = Vec::new();
    let mut current: Option<(u8, String, Vec<String>)> = None;
    for line in markdown.lines() {
        if let Some(header) = line.strip_prefix("## Task ") {
            if let Some(previous) = current.take() {
                tasks.push(finish_task(previous)?);
            }
            let (number, title) = header.split_once(" — ").ok_or("malformed task heading")?;
            current = Some((number.parse()?, title.to_owned(), Vec::new()));
            continue;
        }
        if let Some((_, _, lines)) = current.as_mut() {
            lines.push(line.to_owned());
        }
    }
    if let Some(previous) = current {
        tasks.push(finish_task(previous)?);
    }
    Ok(tasks)
}

fn finish_task((id, title, lines): (u8, String, Vec<String>)) -> Result<Task, Box<dyn Error>> {
    let acceptance_start = lines
        .iter()
        .position(|line| line.trim_start().starts_with("Acceptance:"))
        .ok_or_else(|| format!("task {id} has no Acceptance section"))?;
    let mut acceptance_text = lines[acceptance_start..].join(" ");
    acceptance_text = acceptance_text
        .split("Acceptance:")
        .nth(1)
        .ok_or("malformed Acceptance section")?
        .trim()
        .to_owned();
    let acceptance = backtick_commands(&acceptance_text);
    if acceptance.is_empty() {
        return Err(format!("task {id} has no acceptance commands").into());
    }
    let prompt = lines[..acceptance_start].join("\n").trim().to_owned();
    let acceptance_prompt = acceptance
        .iter()
        .map(|command| format!("- `{command}`"))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "Task {id} — {title}\n\n{prompt}\n\nAcceptance commands:\n{acceptance_prompt}\n\nWork only in this checkout. Do not commit. Make the requested changes and stop."
    );
    Ok(Task {
        id,
        title,
        prompt,
        acceptance,
    })
}

fn backtick_commands(text: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('`') else {
            break;
        };
        let command = rest[..close].trim().trim_end_matches('.').to_owned();
        if !command.is_empty() {
            commands.push(command);
        }
        rest = &rest[close + 1..];
    }
    commands
}

fn redact_arguments(arguments: &[OsString]) -> Vec<String> {
    let mut redact_next = false;
    let mut redacted = Vec::with_capacity(arguments.len());
    for argument in arguments {
        let value = argument.to_string_lossy();
        if redact_next {
            redacted.push("<redacted>".to_owned());
            redact_next = false;
            continue;
        }
        let (flag, value_part) = match value.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (value.as_ref(), None),
        };
        let normalized = flag.to_ascii_lowercase().replace('_', "-");
        let sensitive = normalized.contains("api-key")
            || normalized.ends_with("-token")
            || normalized.ends_with("-secret")
            || normalized.ends_with("-password")
            || normalized.ends_with("-credential")
            || matches!(
                normalized.as_str(),
                "--token" | "--auth" | "--authorization"
            );
        if sensitive {
            if value_part.is_some() {
                redacted.push(format!("{flag}=<redacted>"));
            } else {
                redacted.push(flag.to_owned());
                redact_next = true;
            }
        } else {
            redacted.push(value.into_owned());
        }
    }
    redacted
}

#[allow(clippy::too_many_arguments)]
fn run_task(
    repo: &Path,
    baseline: &str,
    run_root: &Path,
    output: &Path,
    engine: &Path,
    engine_args: &[OsString],
    task: &Task,
    timeout: Duration,
) -> Result<TaskResult, Box<dyn Error>> {
    let task_dir = output.join(format!("task-{:02}", task.id));
    fs::create_dir_all(&task_dir)?;
    let worktree = run_root.join(format!("task-{:02}", task.id));
    git_status_ok(
        repo,
        [
            "worktree",
            "add",
            "--detach",
            worktree.to_str().ok_or("worktree path is not UTF-8")?,
            baseline,
        ],
    )?;
    let mut worktree_guard = WorktreeGuard {
        repo: repo.to_owned(),
        path: worktree.clone(),
        active: true,
    };

    let prompt_file = task_dir.join("prompt.md");
    fs::write(&prompt_file, &task.prompt)?;
    let engine_stdout = task_dir.join("engine.stdout");
    let engine_stderr = task_dir.join("engine.stderr");
    let engine_start = Instant::now();
    let engine_run = run_process(
        engine,
        engine_args,
        &worktree,
        Some(task.prompt.as_bytes()),
        &engine_stdout,
        &engine_stderr,
        timeout,
    )?;
    let engine_elapsed = engine_start.elapsed().as_millis();
    let engine_head = git_output(&worktree, ["rev-parse", "HEAD"])?;
    let engine_committed = engine_head.trim() != baseline;

    let mut acceptance = Vec::new();
    for (index, command) in task.acceptance.iter().enumerate() {
        let stdout = task_dir.join(format!("acceptance-{index:02}.stdout"));
        let stderr = task_dir.join(format!("acceptance-{index:02}.stderr"));
        acceptance.push(run_shell_command(
            command, &worktree, &stdout, &stderr, timeout,
        )?);
    }
    // Intent-to-add makes new files appear in the final binary diff as tracked edits.
    git_status_ok(&worktree, ["add", "--intent-to-add", "--all"])?;
    let diff_file = task_dir.join("diff.patch");
    let mut diff = File::create(&diff_file)?;
    let diff_output = Command::new("git")
        .args(["diff", "--binary", "--no-ext-diff", baseline])
        .current_dir(&worktree)
        .output()?;
    diff.write_all(&diff_output.stdout)?;
    if !diff_output.status.success() {
        return Err("git diff failed in task worktree".into());
    }
    let status = git_output(&worktree, ["status", "--short"])?;
    worktree_guard.remove()?;
    Ok(TaskResult {
        task: task.id,
        title: task.title.clone(),
        baseline: baseline.to_owned(),
        engine_exit_code: engine_run.status.code(),
        engine_committed,
        engine_timed_out: engine_run.timed_out,
        engine_elapsed_ms: engine_elapsed,
        all_acceptance_passed: engine_run.status.success()
            && !engine_run.timed_out
            && !engine_committed
            && acceptance.iter().all(|result| result.success),
        acceptance,
        worktree_status: status,
        diff_file: diff_file.display().to_string(),
        diff_review_required: true,
    })
}

fn run_shell_command(
    command: &str,
    cwd: &Path,
    stdout: &Path,
    stderr: &Path,
    timeout: Duration,
) -> Result<CommandResult, Box<dyn Error>> {
    let run = run_process(
        Path::new("bash"),
        &[OsString::from("-c"), OsString::from(command)],
        cwd,
        None,
        stdout,
        stderr,
        timeout,
    )?;
    Ok(CommandResult {
        command: command.to_owned(),
        exit_code: run.status.code(),
        success: run.status.success() && !run.timed_out,
        timed_out: run.timed_out,
        elapsed_ms: run.elapsed_ms,
        stdout_file: stdout.display().to_string(),
        stderr_file: stderr.display().to_string(),
    })
}

struct ProcessResult {
    status: ExitStatus,
    timed_out: bool,
    elapsed_ms: u128,
}

fn run_process(
    executable: &Path,
    args: &[OsString],
    cwd: &Path,
    stdin_bytes: Option<&[u8]>,
    stdout_path: &Path,
    stderr_path: &Path,
    timeout: Duration,
) -> Result<ProcessResult, Box<dyn Error>> {
    let stdout = File::create(stdout_path)?;
    let stderr = File::create(stderr_path)?;
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(if stdin_bytes.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    if let Some(bytes) = stdin_bytes {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(bytes)?;
        }
    }
    wait_child(&mut child, timeout)
}

fn wait_child(child: &mut Child, timeout: Duration) -> Result<ProcessResult, Box<dyn Error>> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(ProcessResult {
                status,
                timed_out: false,
                elapsed_ms: start.elapsed().as_millis(),
            });
        }
        if start.elapsed() >= timeout {
            #[cfg(unix)]
            kill_process_group(child.id())?;
            #[cfg(not(unix))]
            child.kill()?;
            let status = child.wait()?;
            return Ok(ProcessResult {
                status,
                timed_out: true,
                elapsed_ms: start.elapsed().as_millis(),
            });
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn kill_process_group(child_pid: u32) -> io::Result<()> {
    use std::os::raw::c_int;
    extern "C" {
        fn kill(pid: c_int, signal: c_int) -> c_int;
    }
    let pid = i32::try_from(child_pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "child PID exceeds i32"))?;
    // SAFETY: run_process starts this child as the leader of a new process group;
    // a negative PID targets only that group's descendants, and SIGKILL is valid.
    let result = unsafe { kill(-pid, 9) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn git_output<const N: usize>(repo: &Path, args: [&str; N]) -> Result<String, Box<dyn Error>> {
    let output = Command::new("git").args(args).current_dir(repo).output()?;
    if !output.status.success() {
        return Err(format!(
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn git_status_ok<const N: usize>(repo: &Path, args: [&str; N]) -> Result<(), Box<dyn Error>> {
    let output = Command::new("git").args(args).current_dir(repo).output()?;
    if !output.status.success() {
        return Err(format!(
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

struct WorktreeGuard {
    repo: PathBuf,
    path: PathBuf,
    active: bool,
}

impl WorktreeGuard {
    fn remove(&mut self) -> Result<(), Box<dyn Error>> {
        if self.active {
            git_status_ok(
                &self.repo,
                [
                    "worktree",
                    "remove",
                    "--force",
                    self.path.to_str().ok_or("worktree path is not UTF-8")?,
                ],
            )?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = Command::new("git")
                .args(["worktree", "remove", "--force"])
                .arg(&self.path)
                .current_dir(&self.repo)
                .output();
        }
    }
}

struct RunRootGuard(PathBuf);

impl Drop for RunRootGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::{backtick_commands, parse_tasks, redact_arguments, PINNED_TASK_SHA256};
    use sha2::{Digest, Sha256};

    #[test]
    fn parses_all_ten_pinned_task_sections_and_acceptance_commands(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../specs/021-strata-inference-research/coding-task-fixtures.md"
        );
        let markdown = std::fs::read_to_string(path)?;
        assert_eq!(
            format!("{:x}", Sha256::digest(markdown.as_bytes())),
            PINNED_TASK_SHA256
        );
        let tasks = parse_tasks(&markdown)?;
        assert_eq!(tasks.len(), 10);
        assert_eq!(tasks[0].id, 1);
        assert_eq!(
            tasks[0].acceptance,
            ["nix develop --command cargo test -p agentix-harness"]
        );
        assert!(tasks[0].prompt.contains("Acceptance commands:"));
        assert_eq!(tasks[9].id, 10);
        Ok(())
    }

    #[test]
    fn extracts_multiple_commands_from_wrapped_acceptance_text() {
        assert_eq!(
            backtick_commands("`cargo test` and\n`cargo check`"),
            ["cargo test", "cargo check"]
        );
    }

    #[test]
    fn redacts_sensitive_engine_argument_values() {
        let args = [
            "--model".into(),
            "qwen-test".into(),
            "--api-key=secret-value".into(),
            "--access-token".into(),
            "secret-token".into(),
        ];
        assert_eq!(
            redact_arguments(&args),
            [
                "--model",
                "qwen-test",
                "--api-key=<redacted>",
                "--access-token",
                "<redacted>"
            ]
        );
    }
}
