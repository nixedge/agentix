use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::process::Command;

#[derive(Parser)]
#[command(name = "gh-jail-server", about = "gh proxy server for claude-jail")]
struct Args {
    /// Path for the Unix domain socket to listen on.
    #[arg(long)]
    socket: String,

    /// Allow mutating API calls and release write operations.
    #[arg(long)]
    write: bool,

    /// Allowed GitHub repos (owner/repo). Repeatable. If none given, all repos
    /// are allowed (no restriction). Commands that explicitly target a repo not
    /// in this list are rejected.
    #[arg(long = "repo", value_name = "OWNER/REPO")]
    allowed_repos: Vec<String>,
}

#[derive(Deserialize)]
struct Request {
    args: Vec<String>,
}

#[derive(Serialize)]
struct Response {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Remove any stale socket from a previous run.
    let _ = std::fs::remove_file(&args.socket);

    let listener = UnixListener::bind(&args.socket)
        .with_context(|| format!("binding socket {}", args.socket))?;

    if args.allowed_repos.is_empty() {
        eprintln!(
            "gh-jail-server: listening on {} (all repos allowed)",
            args.socket
        );
    } else {
        eprintln!(
            "gh-jail-server: listening on {} (allowed repos: {})",
            args.socket,
            args.allowed_repos.join(", ")
        );
    }

    let write_mode = args.write;
    let allowed_repos = args.allowed_repos;
    loop {
        let (stream, _) = listener.accept().await.context("accept")?;
        let allowed_repos = allowed_repos.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, write_mode, &allowed_repos).await {
                eprintln!("gh-jail-server: handler error: {e}");
            }
        });
    }
}

async fn handle(
    stream: tokio::net::UnixStream,
    write_mode: bool,
    allowed_repos: &[String],
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .context("reading request")?;

    let req: Request = serde_json::from_str(line.trim()).context("parsing request")?;

    let resp = match check_allowed(&req.args, write_mode, allowed_repos) {
        Err(reason) => Response {
            stdout: String::new(),
            stderr: format!("gh: {reason}\n"),
            exit_code: 1,
        },
        Ok(()) => run_gh(&req.args).await,
    };

    let mut out = serde_json::to_string(&resp).context("serializing response")?;
    out.push('\n');
    write_half
        .write_all(out.as_bytes())
        .await
        .context("writing response")?;

    Ok(())
}

async fn run_gh(args: &[String]) -> Response {
    match Command::new("gh")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
    {
        Ok(out) => Response {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            exit_code: out.status.code().unwrap_or(1),
        },
        Err(e) => Response {
            stdout: String::new(),
            stderr: format!("gh-jail-server: failed to run gh: {e}\n"),
            exit_code: 1,
        },
    }
}

/// Returns Ok(()) if the command is permitted, Err(reason) if it is blocked.
fn check_allowed(
    args: &[String],
    write_mode: bool,
    allowed_repos: &[String],
) -> Result<(), String> {
    let sub = match args.first() {
        Some(s) => s.as_str(),
        None => return Err("no subcommand given".into()),
    };

    // Hard blocks — could exfiltrate credentials or modify account/org settings.
    match sub {
        "auth" | "ssh-key" | "gpg-key" | "config" | "extension" | "alias" => {
            return Err(format!("'{sub}' is not permitted in the jail"));
        }
        _ => {}
    }

    // gh repo: only safe read operations.
    if sub == "repo" {
        let action = args.get(1).map(String::as_str).unwrap_or("");
        match action {
            "view" | "list" | "clone" | "sync" => {}
            _ => {
                return Err(format!(
                    "'repo {action}' is not permitted (allowed: view, list, clone, sync)"
                ))
            }
        }
    }

    // gh release: mutating operations require write mode.
    if sub == "release" && !write_mode {
        let action = args.get(1).map(String::as_str).unwrap_or("");
        match action {
            "create" | "upload" | "delete" | "edit" => {
                return Err(format!("'release {action}' requires the --write flag"));
            }
            _ => {}
        }
    }

    // gh api: GET is always fine; anything else requires write mode.
    // Also enforce repo restriction on the URL path.
    if sub == "api" {
        let method = find_flag(args, &["-X", "--method"]).unwrap_or("GET");
        if !method.eq_ignore_ascii_case("GET") && !write_mode {
            return Err(format!("'api --method {method}' requires the --write flag"));
        }
        // Check that any /repos/owner/repo/ path is in the allowed list.
        if let Some(repo) = api_path_repo(args) {
            check_repo_allowed(&repo, allowed_repos)?;
        }
        return Ok(());
    }

    // For all other subcommands, check any explicit -R/--repo flag.
    if let Some(repo) = find_flag(args, &["-R", "--repo"]) {
        check_repo_allowed(repo, allowed_repos)?;
    }

    Ok(())
}

/// Reject if `repo` is not in the allowed list (no-op when list is empty).
fn check_repo_allowed(repo: &str, allowed: &[String]) -> Result<(), String> {
    if allowed.is_empty() {
        return Ok(());
    }
    if allowed.iter().any(|r| r.eq_ignore_ascii_case(repo)) {
        Ok(())
    } else {
        Err(format!(
            "repo '{repo}' is not in the allowed list ({})",
            allowed.join(", ")
        ))
    }
}

/// Extract `owner/repo` from a `gh api` path argument like `/repos/owner/repo/pulls`.
fn api_path_repo(args: &[String]) -> Option<String> {
    // The path is the first positional arg after flags.
    for arg in args.iter().skip(1) {
        if arg.starts_with('-') {
            continue;
        }
        // Strip leading slash for uniform handling.
        let path = arg.trim_start_matches('/');
        if let Some(rest) = path.strip_prefix("repos/") {
            let mut parts = rest.splitn(3, '/');
            let owner = parts.next()?;
            let repo = parts.next()?;
            return Some(format!("{owner}/{repo}"));
        }
        // Only examine the first positional arg.
        break;
    }
    None
}

/// Find the value following a flag like `-R`/`--repo` or `--repo=VALUE`.
fn find_flag<'a>(args: &'a [String], flags: &[&str]) -> Option<&'a str> {
    for (i, arg) in args.iter().enumerate() {
        if flags.contains(&arg.as_str()) {
            return args.get(i + 1).map(String::as_str);
        }
        for flag in flags {
            if let Some(val) = arg.strip_prefix(&format!("{flag}=")) {
                return Some(val);
            }
        }
    }
    None
}
