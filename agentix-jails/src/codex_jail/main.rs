use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

#[derive(Parser)]
#[command(
    name = "codex-jail",
    about = "Run codex in a bubblewrap sandbox with nix daemon access"
)]
struct Args {
    /// Bind path read-only at its real path inside the jail. Repeatable.
    #[arg(long = "ro", value_name = "PATH")]
    ro_paths: Vec<PathBuf>,

    /// Bind path read-write at its real path inside the jail. Repeatable.
    #[arg(long = "rw", value_name = "PATH")]
    rw_paths: Vec<PathBuf>,

    /// Mount the parent of .bare to expose all sibling worktrees. Use when the
    /// session will spawn agents that work across multiple worktrees at once.
    #[arg(long = "all-worktrees")]
    all_worktrees: bool,

    /// Pass --dangerously-bypass-approvals-and-sandbox to codex (skip all confirmation prompts).
    #[arg(long = "dangerous")]
    dangerous: bool,

    /// Arguments passed directly to codex (e.g. "Fix the tests")
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    codex_args: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let home = PathBuf::from(env::var("HOME").context("HOME not set")?);
    let cwd = env::current_dir().context("cannot determine current directory")?;
    let bin_dir = PathBuf::from(
        env::var("CODEX_JAIL_BIN_DIR")
            .context("CODEX_JAIL_BIN_DIR not set — run via the nix package wrapper")?,
    );
    let bwrap_path = env::var("CODEX_JAIL_BWRAP").unwrap_or_else(|_| "bwrap".into());
    let nix_socket = env::var("AGENTIC_NIX_DAEMON")
        .unwrap_or_else(|_| "/nix/var/nix/daemon-socket/socket".to_string());

    let direnv_env = if cwd.join(".envrc").exists() {
        capture_direnv_env(&cwd).unwrap_or_default()
    } else {
        HashMap::new()
    };

    let mut b: Vec<OsString> = Vec::new();

    push(&mut b, &["--unshare-all", "--share-net"]);

    push(&mut b, &["--proc", "/proc"]);
    push(&mut b, &["--dev", "/dev"]);
    push(&mut b, &["--tmpfs", "/tmp"]);

    push(&mut b, &["--ro-bind", "/nix", "/nix"]);

    if Path::new(&nix_socket).exists() {
        push(&mut b, &["--bind", &nix_socket, &nix_socket]);
    } else {
        eprintln!("warning: nix daemon socket not found at {nix_socket}");
        eprintln!("  set AGENTIC_NIX_DAEMON to override");
    }

    ro_bind_if_exists(&mut b, "/etc/static", "/etc/static");
    ro_bind_if_exists(&mut b, "/etc/nix", "/etc/nix");
    ro_bind_if_exists(&mut b, "/nix/var/nix/profiles", "/nix/var/nix/profiles");
    ro_bind_if_exists(&mut b, "/run/current-system", "/run/current-system");

    for p in ["/etc/ssl", "/etc/ca-certificates", "/etc/pki/tls"] {
        ro_bind_if_exists(&mut b, p, p);
    }

    for p in [
        "/etc/resolv.conf",
        "/etc/hosts",
        "/etc/nsswitch.conf",
        "/etc/localtime",
        "/etc/passwd",
        "/etc/group",
    ] {
        ro_bind_if_exists(&mut b, p, p);
    }

    b.push("--tmpfs".into());
    b.push(home.as_os_str().into());

    bind(&mut b, "--ro-bind", &bin_dir, &home.join("bin"));

    push(
        &mut b,
        &[
            "--symlink",
            &format!("{}/bin/bash", home.display()),
            "/bin/sh",
        ],
    );

    push(&mut b, &["--dir", "/usr/bin"]);
    push(
        &mut b,
        &[
            "--symlink",
            &format!("{}/bin/env", home.display()),
            "/usr/bin/env",
        ],
    );

    ro_bind_if_exists(
        &mut b,
        &home.join(".gitconfig").to_string_lossy(),
        &home.join(".gitconfig").to_string_lossy(),
    );

    let git_cfg_dir = home.join(".config").join("git");
    if git_cfg_dir.exists() {
        push(&mut b, &["--dir", &home.join(".config").to_string_lossy()]);
        bind(
            &mut b,
            "--ro-bind",
            &git_cfg_dir,
            &home.join(".config").join("git"),
        );
    }

    let known_hosts = home.join(".ssh").join("known_hosts");
    if known_hosts.exists() {
        push(&mut b, &["--dir", &home.join(".ssh").to_string_lossy()]);
        bind(
            &mut b,
            "--ro-bind",
            &known_hosts,
            &home.join(".ssh").join("known_hosts"),
        );
    }

    let dot_codex = home.join(".codex");
    if !dot_codex.exists() {
        fs::create_dir_all(&dot_codex).context("creating ~/.codex")?;
    }
    bind(&mut b, "--bind", &dot_codex, &home.join(".codex"));

    let dot_agents = home.join(".agents");
    if !dot_agents.exists() {
        fs::create_dir_all(&dot_agents).context("creating ~/.agents")?;
    }
    bind(&mut b, "--bind", &dot_agents, &home.join(".agents"));

    let git_ctx = detect_git_worktree(&cwd);

    let all_wt_root: Option<PathBuf> = if args.all_worktrees {
        git_ctx
            .as_ref()
            .and_then(|(_, common_dir)| common_dir.parent().map(PathBuf::from))
    } else {
        None
    };
    let bind_root: &Path = all_wt_root
        .as_deref()
        .or_else(|| git_ctx.as_ref().map(|(wt, _)| wt.as_path()))
        .unwrap_or(cwd.as_path());
    if let Some(parent) = bind_root.parent() {
        push(&mut b, &["--dir", &parent.to_string_lossy()]);
    }
    bind(&mut b, "--bind", bind_root, bind_root);

    if let Some((wt_root, common_dir)) = &git_ctx {
        if !args.all_worktrees && !common_dir.starts_with(wt_root) {
            if let Some(parent) = common_dir.parent() {
                push(&mut b, &["--dir", &parent.to_string_lossy()]);
            }
            bind(&mut b, "--bind", common_dir, common_dir);
        }
        let hooks = common_dir.join("hooks");
        push(&mut b, &["--tmpfs", &hooks.to_string_lossy()]);
        let config = common_dir.join("config");
        if config.exists() {
            bind(&mut b, "--ro-bind", &config, &config);
        }
    }

    for path in &args.ro_paths {
        if let Some(parent) = path.parent() {
            push(&mut b, &["--dir", &parent.to_string_lossy()]);
        }
        bind(&mut b, "--ro-bind", path, path);
    }

    for path in &args.rw_paths {
        if let Some(parent) = path.parent() {
            push(&mut b, &["--dir", &parent.to_string_lossy()]);
        }
        bind(&mut b, "--bind", path, path);
    }

    b.push("--clearenv".into());

    setenv(&mut b, "HOME", &home.to_string_lossy());

    let jail_path = match direnv_env.get("PATH") {
        Some(dp) => format!("{}/bin:{dp}", home.display()),
        None => format!("{}/bin", home.display()),
    };
    setenv(&mut b, "PATH", &jail_path);

    setenv(
        &mut b,
        "USER",
        &env::var("USER").unwrap_or_else(|_| "user".into()),
    );
    passthrough(&mut b, "LOGNAME");

    setenv(&mut b, "NIX_REMOTE", "daemon");
    setenv(&mut b, "NIX_DAEMON_SOCKET_PATH", &nix_socket);
    passthrough(&mut b, "NIX_PATH");
    let host_nix_config = env::var("NIX_CONFIG").unwrap_or_default();
    let nix_config = if host_nix_config.is_empty() {
        "extra-experimental-features = nix-command flakes".into()
    } else {
        format!("{host_nix_config}\nextra-experimental-features = nix-command flakes")
    };
    setenv(&mut b, "NIX_CONFIG", &nix_config);

    if let Ok(cert) = env::var("NIX_SSL_CERT_FILE") {
        setenv(&mut b, "NIX_SSL_CERT_FILE", &cert);
        setenv(&mut b, "SSL_CERT_FILE", &cert);
    } else {
        for cert_path in [
            "/etc/ssl/certs/ca-bundle.crt",
            "/etc/ssl/certs/ca-certificates.crt",
        ] {
            if Path::new(cert_path).exists() {
                setenv(&mut b, "NIX_SSL_CERT_FILE", cert_path);
                setenv(&mut b, "SSL_CERT_FILE", cert_path);
                break;
            }
        }
    }

    for var in ["TERM", "COLORTERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION"] {
        passthrough(&mut b, var);
    }

    for var in [
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "LC_MESSAGES",
        "LC_COLLATE",
        "LC_TIME",
    ] {
        passthrough(&mut b, var);
    }

    for var in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "no_proxy",
    ] {
        passthrough(&mut b, var);
    }

    for var in ["OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_ORG_ID"] {
        passthrough(&mut b, var);
    }

    for (k, v) in env::vars() {
        if k.starts_with("CODEX_") {
            setenv(&mut b, &k, &v);
        }
    }

    for var in [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
    ] {
        passthrough(&mut b, var);
    }

    for (k, v) in &direnv_env {
        if k == "PATH" {
            continue;
        }
        setenv(&mut b, k, v);
    }

    setenv(&mut b, "TMPDIR", "/tmp");

    b.push("--chdir".into());
    b.push(cwd.as_os_str().into());

    b.push("--".into());
    b.push("codex".into());
    // Always bypass hook trust — the jail already masks hooks/ with tmpfs.
    b.push("--dangerously-bypass-hook-trust".into());
    if args.dangerous {
        b.push("--dangerously-bypass-approvals-and-sandbox".into());
    }
    for a in &args.codex_args {
        b.push(a.into());
    }

    let err = Command::new(&bwrap_path).args(&b).exec();
    Err(anyhow::anyhow!("exec {bwrap_path}: {err}"))
}

fn push(b: &mut Vec<OsString>, args: &[&str]) {
    for a in args {
        b.push((*a).into());
    }
}

fn bind(b: &mut Vec<OsString>, flag: &str, src: &Path, dst: &Path) {
    b.push(flag.into());
    b.push(src.as_os_str().into());
    b.push(dst.as_os_str().into());
}

fn ro_bind_if_exists(b: &mut Vec<OsString>, src: &str, dst: &str) {
    if Path::new(src).exists() {
        b.push("--ro-bind".into());
        b.push(src.into());
        b.push(dst.into());
    }
}

fn setenv(b: &mut Vec<OsString>, key: &str, val: &str) {
    b.push("--setenv".into());
    b.push(key.into());
    b.push(val.into());
}

fn passthrough(b: &mut Vec<OsString>, key: &str) {
    if let Ok(val) = env::var(key) {
        setenv(b, key, &val);
    }
}

fn detect_git_worktree(cwd: &Path) -> Option<(PathBuf, PathBuf)> {
    let wt_out = Command::new("git")
        .args(["-C", &cwd.to_string_lossy(), "rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !wt_out.status.success() {
        return None;
    }
    let wt_root = PathBuf::from(std::str::from_utf8(&wt_out.stdout).ok()?.trim());

    let common_out = Command::new("git")
        .args([
            "-C",
            &cwd.to_string_lossy(),
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ])
        .output()
        .ok()?;
    if !common_out.status.success() {
        return None;
    }
    let common_dir = PathBuf::from(std::str::from_utf8(&common_out.stdout).ok()?.trim());

    Some((wt_root, common_dir))
}

fn capture_direnv_env(dir: &Path) -> Result<HashMap<String, String>> {
    let out = Command::new("direnv")
        .args(["export", "json"])
        .current_dir(dir)
        .output()?;

    if !out.status.success() || out.stdout.is_empty() {
        return Ok(HashMap::new());
    }

    let raw: HashMap<String, Option<String>> = serde_json::from_slice(&out.stdout)?;
    Ok(raw
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| (k, v)))
        .collect())
}
