use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use std::{env, fs, thread};

#[derive(Parser)]
#[command(
    name = "claude-jail",
    about = "Run claude in a bubblewrap sandbox with nix daemon access"
)]
struct Args {
    /// Bind path read-only at its real path inside the jail. Repeatable.
    #[arg(long = "ro", value_name = "PATH")]
    ro_paths: Vec<PathBuf>,

    /// Bind path read-write at its real path inside the jail. Repeatable.
    #[arg(long = "rw", value_name = "PATH")]
    rw_paths: Vec<PathBuf>,

    /// Pass --dangerously-skip-permissions to claude.
    #[arg(long = "dangerous")]
    dangerous: bool,

    /// Mount the parent of .bare to expose all sibling worktrees. Use when the
    /// session will spawn agents that work across multiple worktrees at once.
    #[arg(long = "all-worktrees")]
    all_worktrees: bool,

    /// Grant write (push/merge/release) access via the gh proxy.
    #[arg(long = "write")]
    write: bool,

    /// Additional GitHub repos (owner/repo) the proxy is allowed to access.
    /// The cwd's origin remote is always included automatically. Repeatable.
    #[arg(long = "repo", value_name = "OWNER/REPO")]
    allowed_repos: Vec<String>,

    /// Skip starting the gh proxy (no gh available inside the jail).
    #[arg(long = "no-github-auth")]
    no_github_auth: bool,

    /// Forward the host SSH agent socket into the jail.
    /// WARNING: this lets Claude sign arbitrary SSH operations with your keys.
    /// Never combine with --dangerous.
    #[arg(long = "allow-ssh")]
    allow_ssh: bool,

    /// Print each command before running it and dump the full bwrap arg list.
    #[arg(long = "debug")]
    debug: bool,
}

// ── gh proxy server ───────────────────────────────────────────────────────────

struct GhProxy {
    socket_path: PathBuf,
    child: std::process::Child,
}

impl GhProxy {
    /// Spawn `gh-jail-server` and wait for the socket to become available.
    fn start(
        socket_path: PathBuf,
        write_mode: bool,
        allowed_repos: &[String],
        debug: bool,
    ) -> Result<Self> {
        let server_bin =
            env::var("CLAUDE_JAIL_GH_SERVER").unwrap_or_else(|_| "gh-jail-server".into());

        let mut cmd = Command::new(&server_bin);
        cmd.arg("--socket").arg(&socket_path);
        if write_mode {
            cmd.arg("--write");
        }
        for repo in allowed_repos {
            cmd.arg("--repo").arg(repo);
        }
        dbg_cmd(&cmd, debug);

        let child = cmd
            .spawn()
            .with_context(|| format!("spawning {server_bin}"))?;

        // Poll until the socket file appears (up to 5 s).
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if socket_path.exists() {
                return Ok(Self { socket_path, child });
            }
            thread::sleep(Duration::from_millis(50));
        }

        anyhow::bail!(
            "gh-jail-server did not create socket at {} within 5 s",
            socket_path.display()
        );
    }
}

impl Drop for GhProxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.socket_path);
    }
}

// ── Synthetic git config ──────────────────────────────────────────────────────

fn write_synthetic_gitconfig(dir: &Path) -> Result<PathBuf> {
    let name = git_global_config("user.name").unwrap_or_else(|| "Claude".into());
    let email = git_global_config("user.email").unwrap_or_else(|| "claude@localhost".into());

    let contents = format!(
        "[user]\n\tname = {name}\n\temail = {email}\n\
         [commit]\n\tgpgsign = false\n\
         [tag]\n\tgpgsign = false\n\
         [credential \"https://github.com\"]\n\thelper = gh auth git-credential\n\
         [url \"https://github.com/\"]\n\tinsteadOf = git@github.com:\n"
    );

    let path = dir.join("gitconfig");
    fs::write(&path, contents).context("writing synthetic gitconfig")?;
    Ok(path)
}

fn git_global_config(key: &str) -> Option<String> {
    Command::new("git")
        .args(["config", "--global", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = Args::parse();

    let home = PathBuf::from(env::var("HOME").context("HOME not set")?);
    let cwd = env::current_dir().context("cannot determine current directory")?;
    let bin_dir = PathBuf::from(
        env::var("CLAUDE_JAIL_BIN_DIR")
            .context("CLAUDE_JAIL_BIN_DIR not set — run via the nix package wrapper")?,
    );
    let bwrap_path = env::var("CLAUDE_JAIL_BWRAP").unwrap_or_else(|_| "bwrap".into());
    let nix_socket = env::var("AGENTIC_NIX_DAEMON")
        .unwrap_or_else(|_| "/nix/var/nix/daemon-socket/socket".to_string());

    let direnv_env = if cwd.join(".envrc").exists() {
        capture_direnv_env(&cwd).unwrap_or_default()
    } else {
        HashMap::new()
    };

    let gitconfig_dir = tempfile::Builder::new()
        .prefix("claude-jail-git-")
        .tempdir()
        .context("creating gitconfig tempdir")?;
    let gitconfig_host_path = write_synthetic_gitconfig(gitconfig_dir.path())?;

    // Build the allowed-repos list: cwd origin + any --repo flags.
    let mut allowed_repos = args.allowed_repos.clone();
    if let Some(origin) = github_repo_from_remote(&cwd) {
        if !allowed_repos.contains(&origin) {
            allowed_repos.insert(0, origin);
        }
    }

    // Start the gh proxy server (unless opted out).
    // The TempDir is kept in the tuple so it lives until gh_proxy drops (after bwrap exits).
    let gh_proxy: Option<(GhProxy, tempfile::TempDir)> = if args.no_github_auth {
        None
    } else {
        let socket_dir = tempfile::Builder::new()
            .prefix("claude-jail-gh-")
            .tempdir()
            .context("creating gh socket tempdir")?;
        let socket_path = socket_dir.path().join("gh.sock");
        match GhProxy::start(socket_path, args.write, &allowed_repos, args.debug) {
            Ok(proxy) => {
                eprintln!(
                    "claude-jail: gh proxy listening at {}",
                    proxy.socket_path.display()
                );
                Some((proxy, socket_dir))
            }
            Err(e) => {
                eprintln!("claude-jail: warning: could not start gh proxy: {e}");
                eprintln!("claude-jail: gh will not be available inside the jail");
                None
            }
        }
    };

    let mut b: Vec<OsString> = Vec::new();

    push(&mut b, &["--unshare-all", "--share-net"]);
    push(&mut b, &["--proc", "/proc"]);
    push(&mut b, &["--dev", "/dev"]);
    push(&mut b, &["--tmpfs", "/tmp"]);

    push(
        &mut b,
        &[
            "--ro-bind",
            &gitconfig_host_path.to_string_lossy(),
            "/tmp/gitconfig",
        ],
    );

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

    // Node.js (Claude Code) uses posix_spawn('/bin/sh') to run hook commands.
    // NixOS has no /bin; create /bin/sh → ~/bin/bash inside the sandbox.
    push(
        &mut b,
        &[
            "--symlink",
            &format!("{}/bin/bash", home.display()),
            "/bin/sh",
        ],
    );

    let dot_claude = home.join(".claude");
    if !dot_claude.exists() {
        fs::create_dir_all(&dot_claude).context("creating ~/.claude")?;
    }
    bind(&mut b, "--bind", &dot_claude, &home.join(".claude"));

    let dot_claude_json = home.join(".claude.json");
    if dot_claude_json.exists() {
        bind(
            &mut b,
            "--bind",
            &dot_claude_json,
            &home.join(".claude.json"),
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

    if args.allow_ssh {
        eprintln!("claude-jail: WARNING: --allow-ssh forwards your SSH agent into the jail.");
        eprintln!("claude-jail: Claude can sign arbitrary SSH operations with your keys.");
        if args.dangerous {
            eprintln!("claude-jail: WARNING: combining --allow-ssh with --dangerous is strongly discouraged.");
        }
        if let Ok(sock) = env::var("SSH_AUTH_SOCK") {
            let sock_path = PathBuf::from(&sock);
            if sock_path.exists() {
                bind(&mut b, "--bind", &sock_path, &sock_path);
            }
        }
    }

    // Bind the gh proxy socket directory so the socket is reachable inside.
    if let Some((ref proxy, _)) = gh_proxy {
        let sock_dir = proxy.socket_path.parent().expect("socket path has parent");
        bind(&mut b, "--bind", sock_dir, sock_dir);
    }

    let git_ctx = detect_git_worktree(&cwd);

    // With --all-worktrees, bind the parent of .bare to expose all sibling
    // worktrees; otherwise bind only the current worktree root.
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
        // With --all-worktrees the parent bind already covers common_dir; without
        // it, bind common_dir separately when it lives outside the worktree root.
        if !args.all_worktrees && !common_dir.starts_with(wt_root) {
            if let Some(parent) = common_dir.parent() {
                push(&mut b, &["--dir", &parent.to_string_lossy()]);
            }
            bind(&mut b, "--bind", common_dir, common_dir);
        }
        let hooks = common_dir.join("hooks");
        push(&mut b, &["--tmpfs", &hooks.to_string_lossy()]);
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

    for var in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
    ] {
        passthrough(&mut b, var);
    }
    for (k, v) in env::vars() {
        if k.starts_with("CLAUDE_") {
            setenv(&mut b, &k, &v);
        }
    }

    if args.allow_ssh {
        if let Ok(v) = env::var("SSH_AUTH_SOCK") {
            setenv(&mut b, "SSH_AUTH_SOCK", &v);
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

    setenv(&mut b, "GIT_CONFIG_GLOBAL", "/tmp/gitconfig");

    // Expose the proxy socket path — no token needed inside the jail.
    if let Some((ref proxy, _)) = gh_proxy {
        setenv(
            &mut b,
            "GH_PROXY_SOCKET",
            &proxy.socket_path.to_string_lossy(),
        );
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
    b.push("claude".into());
    if args.dangerous {
        b.push("--dangerously-skip-permissions".into());
    }

    if args.debug {
        eprintln!("[debug] bwrap: {bwrap_path}");
        for arg in &b {
            eprintln!("[debug]   {:?}", arg);
        }
    }

    let status = Command::new(&bwrap_path)
        .args(&b)
        .spawn()
        .with_context(|| format!("spawning {bwrap_path}"))?
        .wait()
        .context("waiting for bwrap")?;

    // GhProxy::drop kills the server and removes the socket.
    drop(gh_proxy);
    // gitconfig_dir TempDir drops here.

    std::process::exit(status.code().unwrap_or(1));
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn dbg_cmd(cmd: &Command, debug: bool) {
    if debug {
        eprintln!("[debug] {:?}", cmd);
    }
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

fn github_repo_from_remote(cwd: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["-C", &cwd.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = std::str::from_utf8(&out.stdout).ok()?.trim();
    let slug = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else if let Some(rest) = url.strip_prefix("https://github.com/") {
        rest
    } else {
        return None;
    };
    Some(slug.trim_end_matches(".git").to_string())
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
