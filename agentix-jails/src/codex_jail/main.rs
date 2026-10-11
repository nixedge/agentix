use anyhow::{Context, Result};
use clap::Parser;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use std::{env, fs, thread};

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

    /// Grant write (push/merge/release) access via the gh proxy.
    #[arg(long = "write")]
    write: bool,

    /// Additional GitHub repos (owner/repo) the proxy is allowed to access.
    /// The cwd's origin remote is always included automatically. Repeatable.
    #[arg(long = "repo", value_name = "OWNER/REPO")]
    allowed_repos: Vec<String>,

    /// Allow the gh proxy to access any GitHub repo (no allowlist).
    /// By default only repos found in git remotes (plus any --repo flags) are allowed.
    #[arg(long = "all-repos")]
    all_repos: bool,

    /// Skip starting the gh proxy (no gh available inside the jail).
    #[arg(long = "no-github-auth")]
    no_github_auth: bool,

    /// Forward the host SSH agent socket into the jail.
    /// WARNING: this lets Codex sign arbitrary SSH operations with your keys.
    /// Never combine with --dangerous.
    #[arg(long = "allow-ssh")]
    allow_ssh: bool,

    /// Print each command before running it and print bwrap arguments with environment values redacted.
    #[arg(long = "debug")]
    debug: bool,

    /// Arguments passed directly to codex (e.g. "Fix the tests")
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    codex_args: Vec<String>,
}

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
            env::var("CODEX_JAIL_GH_SERVER").unwrap_or_else(|_| "gh-jail-server".into());

        let mut cmd = Command::new(&server_bin);
        cmd.arg("--socket").arg(&socket_path);
        if write_mode {
            cmd.arg("--write");
        }
        for repo in allowed_repos {
            cmd.arg("--repo").arg(repo);
        }
        if debug {
            eprintln!("[debug] starting gh proxy: {cmd:?}");
        }

        let child = cmd
            .spawn()
            .with_context(|| format!("spawning {server_bin}"))?;

        let mut proxy = Self { socket_path, child };

        // Poll until the socket file appears (up to 5 s).
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(status) = proxy.child.try_wait()? {
                anyhow::bail!("gh-jail-server exited during startup: {status}");
            }
            if proxy.socket_path.exists() {
                return Ok(proxy);
            }
            thread::sleep(Duration::from_millis(50));
        }

        anyhow::bail!(
            "gh-jail-server did not create socket at {} within 5 s",
            proxy.socket_path.display()
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
    let name = git_global_config("user.name").unwrap_or_else(|| "Codex".into());
    let email = git_global_config("user.email").unwrap_or_else(|| "codex@localhost".into());

    let path = dir.join("gitconfig");
    for (key, value) in [
        ("user.name", name.as_str()),
        ("user.email", email.as_str()),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
        ("url.https://github.com/.insteadOf", "git@github.com:"),
    ] {
        let status = Command::new("git")
            .arg("config")
            .arg("--file")
            .arg(&path)
            .args([key, value])
            .status()
            .context("writing synthetic gitconfig")?;
        anyhow::ensure!(status.success(), "could not set {key} in jail gitconfig");
    }
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

    let gitconfig_dir = tempfile::Builder::new()
        .prefix("codex-jail-git-")
        .tempdir()
        .context("creating gitconfig tempdir")?;
    let gitconfig_host_path = write_synthetic_gitconfig(gitconfig_dir.path())?;

    // Build the allowed-repos list: all GitHub remotes + any explicit --repo flags.
    // When --all-repos is set, pass an empty list so the server allows everything.
    let allowed_repos: Vec<String> = if args.all_repos {
        Vec::new()
    } else {
        let mut seen: HashSet<String> = HashSet::new();
        let mut repos: Vec<String> = Vec::new();
        for repo in github_repos_from_all_remotes(&cwd) {
            if seen.insert(repo.clone()) {
                repos.push(repo);
            }
        }
        for repo in &args.allowed_repos {
            if seen.insert(repo.clone()) {
                repos.push(repo.clone());
            }
        }
        repos
    };

    // Start the gh proxy server (unless opted out).
    // The TempDir is kept in the tuple so it lives until gh_proxy drops (after bwrap exits).
    let gh_proxy: Option<(GhProxy, tempfile::TempDir)> = if args.no_github_auth {
        None
    } else if allowed_repos.is_empty() && !args.all_repos {
        eprintln!("codex-jail: no GitHub remotes found; use --repo OWNER/REPO or --all-repos to enable the proxy");
        None
    } else {
        let socket_dir = tempfile::Builder::new()
            .prefix("codex-jail-gh-")
            .tempdir()
            .context("creating gh socket tempdir")?;
        let socket_path = socket_dir.path().join("gh.sock");
        match GhProxy::start(socket_path, args.write, &allowed_repos, args.debug) {
            Ok(proxy) => {
                eprintln!(
                    "codex-jail: gh proxy listening at {}",
                    proxy.socket_path.display()
                );
                Some((proxy, socket_dir))
            }
            Err(e) => {
                eprintln!("codex-jail: warning: could not start gh proxy: {e}");
                eprintln!("codex-jail: gh will not be available inside the jail");
                None
            }
        }
    };

    let mut b: Vec<OsString> = Vec::new();

    push(&mut b, &["--unshare-all", "--share-net"]);

    push(&mut b, &["--proc", "/proc"]);
    push(&mut b, &["--dev", "/dev"]);
    push(&mut b, &["--tmpfs", "/tmp"]);
    bind(
        &mut b,
        "--ro-bind",
        &gitconfig_host_path,
        Path::new("/tmp/gitconfig"),
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

    if args.allow_ssh {
        eprintln!("codex-jail: WARNING: --allow-ssh forwards your SSH agent into the jail.");
        eprintln!("codex-jail: Codex can sign arbitrary SSH operations with your keys.");
        if args.dangerous {
            eprintln!("codex-jail: WARNING: combining --allow-ssh with --dangerous is strongly discouraged.");
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
        let Some(sock_dir) = proxy.socket_path.parent() else {
            anyhow::bail!("gh proxy socket path has no parent directory");
        };
        bind(&mut b, "--bind", sock_dir, sock_dir);
    }

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
        if matches!(
            k.as_str(),
            "PATH" | "SSH_AUTH_SOCK" | "GH_PROXY_SOCKET" | "GIT_CONFIG_GLOBAL"
        ) {
            continue;
        }
        setenv(&mut b, k, v);
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

    if args.allow_ssh {
        passthrough(&mut b, "SSH_AUTH_SOCK");
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

    if args.debug {
        eprintln!("[debug] bwrap: {bwrap_path}");
        let mut i = 0;
        while i < b.len() {
            if b[i] == "--setenv" && i + 2 < b.len() {
                eprintln!("[debug] --setenv {:?} <redacted>", b[i + 1]);
                i += 3;
            } else if b[i] == "--" {
                eprintln!("[debug] codex arguments omitted");
                break;
            } else {
                eprintln!("[debug] {:?}", b[i]);
                i += 1;
            }
        }
    }
    let status = Command::new(&bwrap_path)
        .args(&b)
        .status()
        .with_context(|| format!("running {bwrap_path}"))?;
    drop(gh_proxy);
    drop(gitconfig_dir);
    std::process::exit(status.code().unwrap_or(1));
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

fn parse_github_slug(url: &str) -> Option<String> {
    let slug = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else if let Some(rest) = url.strip_prefix("https://github.com/") {
        rest
    } else {
        return None;
    };
    Some(slug.trim_end_matches(".git").to_string())
}

/// Enumerate all git remotes in `cwd` and return the GitHub `owner/repo` slug for each
/// one that points at GitHub. Non-GitHub remotes are silently skipped. Duplicates are
/// removed. Returns an empty Vec if the directory is not a git repo or git fails.
fn github_repos_from_all_remotes(cwd: &Path) -> Vec<String> {
    let names_out = Command::new("git")
        .args(["-C", &cwd.to_string_lossy(), "remote"])
        .output();
    let names_out = match names_out {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let names = std::str::from_utf8(&names_out.stdout)
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();

    let mut seen = HashSet::new();
    let mut repos = Vec::new();
    for name in names {
        let url_out = Command::new("git")
            .args(["-C", &cwd.to_string_lossy(), "remote", "get-url", name])
            .output();
        let url = match url_out {
            Ok(o) if o.status.success() => o.stdout,
            _ => continue,
        };
        let url_str = std::str::from_utf8(&url).unwrap_or("").trim().to_string();
        if let Some(slug) = parse_github_slug(&url_str) {
            if seen.insert(slug.clone()) {
                repos.push(slug);
            }
        }
    }
    repos
}

#[cfg(test)]
mod tests {
    use super::parse_github_slug;

    // T005: positive cases — SSH and HTTPS GitHub URLs
    #[test]
    fn parse_ssh_with_git_suffix() {
        assert_eq!(
            parse_github_slug("git@github.com:user/repo.git"),
            Some("user/repo".into())
        );
    }

    #[test]
    fn parse_ssh_without_git_suffix() {
        assert_eq!(
            parse_github_slug("git@github.com:user/repo"),
            Some("user/repo".into())
        );
    }

    #[test]
    fn parse_https_with_git_suffix() {
        assert_eq!(
            parse_github_slug("https://github.com/user/repo.git"),
            Some("user/repo".into())
        );
    }

    #[test]
    fn parse_https_without_git_suffix() {
        assert_eq!(
            parse_github_slug("https://github.com/user/repo"),
            Some("user/repo".into())
        );
    }

    // T006: negative cases — non-GitHub URLs return None without panicking
    #[test]
    fn parse_gitlab_ssh_returns_none() {
        assert_eq!(parse_github_slug("git@gitlab.com:user/repo.git"), None);
    }

    #[test]
    fn parse_non_github_https_returns_none() {
        assert_eq!(parse_github_slug("https://example.com/repo.git"), None);
    }

    #[test]
    fn parse_malformed_returns_none() {
        assert_eq!(parse_github_slug("not-a-url"), None);
    }

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse_github_slug(""), None);
    }

    // T007: deduplication — same slug from SSH and HTTPS appears only once
    #[test]
    fn dedup_same_slug_different_url_forms() {
        let urls = [
            "git@github.com:org/repo.git",
            "https://github.com/org/repo.git",
            "git@github.com:org/repo",
        ];
        let mut seen = std::collections::HashSet::new();
        let mut repos: Vec<String> = Vec::new();
        for url in &urls {
            if let Some(slug) = parse_github_slug(url) {
                if seen.insert(slug.clone()) {
                    repos.push(slug);
                }
            }
        }
        assert_eq!(repos, vec!["org/repo".to_string()]);
    }
}
