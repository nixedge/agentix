use anyhow::{bail, Context, Result};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

pub struct ProfileOptions {
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub bin_dir: PathBuf,
    pub nix_socket: PathBuf,
    pub ro_paths: Vec<PathBuf>,
    pub rw_paths: Vec<PathBuf>,
    pub all_worktrees: bool,
    pub environment: HashMap<String, String>,
}

pub struct Profile {
    pub args: Vec<OsString>,
    pub root: PathBuf,
    _masks: Vec<tempfile::NamedTempFile>,
}

impl Profile {
    pub fn build(options: ProfileOptions) -> Result<Self> {
        let home = options.home.canonicalize().context("canonical home")?;
        let cwd = options.cwd.canonicalize().context("canonical cwd")?;
        let bin_dir = options
            .bin_dir
            .canonicalize()
            .context("canonical tool directory")?;
        let git = bin_dir.join("git");
        let context = git_context(&git, &cwd)?;
        let root = if options.all_worktrees {
            let (_, common) = context
                .as_ref()
                .context("--all-worktrees requires a Git worktree")?;
            if common.file_name().is_none_or(|name| name != ".bare") {
                bail!("--all-worktrees requires the .bare-plus-worktrees layout");
            }
            common
                .parent()
                .context("bare repository has no parent")?
                .to_path_buf()
        } else {
            context.as_ref().map(|c| c.0.clone()).unwrap_or(cwd.clone())
        };
        validate_grant(&root, &home)?;
        let mut args = Vec::new();
        push(
            &mut args,
            &[
                "--die-with-parent",
                "--unshare-all",
                "--share-net",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--tmpfs",
                "/tmp",
                "--ro-bind",
                "/nix/store",
                "/nix/store",
            ],
        );
        if options.nix_socket.exists() {
            bind(
                &mut args,
                "--bind",
                &options.nix_socket,
                &options.nix_socket,
            );
        }
        for path in [
            "/etc/resolv.conf",
            "/etc/hosts",
            "/etc/nsswitch.conf",
            "/etc/passwd",
            "/etc/group",
            "/etc/ssl/certs",
        ] {
            if Path::new(path).exists() {
                bind(&mut args, "--ro-bind", Path::new(path), Path::new(path));
            }
        }
        push_path(&mut args, "--tmpfs", &home);
        bind(&mut args, "--ro-bind", &bin_dir, &home.join("bin"));
        push(&mut args, &["--dir", "/bin", "--symlink"]);
        args.push(home.join("bin/bash").into_os_string());
        args.push("/bin/sh".into());
        push(&mut args, &["--dir", "/usr/bin", "--symlink"]);
        args.push(home.join("bin/env").into_os_string());
        args.push("/usr/bin/env".into());
        bind(&mut args, "--bind", &root, &root);
        if let Some((_, common)) = &context {
            validate_grant(common, &home)?;
            if !common.starts_with(&root) {
                bind(&mut args, "--bind", common, common);
            }
        }
        for (paths, mode) in [
            (&options.ro_paths, "--ro-bind"),
            (&options.rw_paths, "--bind"),
        ] {
            for path in paths {
                let path = path.canonicalize().context("canonical extra mount")?;
                validate_grant(&path, &home)?;
                bind(&mut args, mode, &path, &path);
            }
        }
        let state = home.join(".ax");
        if fs::symlink_metadata(&state).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            bail!("~/.ax must not be a symlink");
        }
        fs::create_dir_all(&state)?;
        bind(&mut args, "--bind", &state, &state);
        let mut masks = Vec::new();
        let mut marker = tempfile::NamedTempFile::new()?;
        let mut read_roots = vec![root.clone()];
        let mut write_roots = vec![root.clone()];
        for path in &options.ro_paths {
            read_roots.push(path.canonicalize()?);
        }
        for path in &options.rw_paths {
            let path = path.canonicalize()?;
            read_roots.push(path.clone());
            write_roots.push(path);
        }
        serde_json::to_writer(
            &mut marker,
            &serde_json::json!({"schema_version":1,"root":root,"read_roots":read_roots,"write_roots":write_roots}),
        )?;
        marker.flush()?;
        bind(
            &mut args,
            "--ro-bind",
            marker.path(),
            Path::new("/run/agentix/ax-profile.json"),
        );
        masks.push(marker);
        if let Some((_, common)) = &context {
            // Mount masks last, so extra mounts cannot restore host hooks/config.
            push_path(&mut args, "--tmpfs", &common.join("hooks"));
            let mut mask = tempfile::NamedTempFile::new()?;
            for (key, default) in [
                ("core.repositoryformatversion", "0"),
                ("core.bare", "false"),
                ("extensions.objectformat", ""),
                ("extensions.refstorage", ""),
                ("extensions.worktreeconfig", ""),
            ] {
                let output = git_command(&git)
                    .args(["config", "--no-includes", "--file"])
                    .arg(common.join("config"))
                    .args(["--get", key])
                    .output()?;
                let value = if output.status.success() {
                    std::str::from_utf8(&output.stdout)?.trim()
                } else {
                    default
                };
                if value.is_empty() {
                    continue;
                }
                if !value.bytes().all(|b| b.is_ascii_alphanumeric()) {
                    bail!("unsupported structural Git config value");
                }
                let (section, key) = key.split_once('.').context("invalid structural key")?;
                writeln!(mask, "[{section}]\n{key} = {value}")?;
            }
            bind(&mut args, "--ro-bind", mask.path(), &common.join("config"));
            masks.push(mask);
            let mut per_worktree = vec![common.join("config.worktree")];
            let worktrees = common.join("worktrees");
            if worktrees.is_dir() {
                for entry in fs::read_dir(worktrees)?.take(1024) {
                    per_worktree.push(entry?.path().join("config.worktree"));
                }
            }
            for path in per_worktree {
                if path.exists() {
                    let mask = tempfile::NamedTempFile::new()?;
                    bind(&mut args, "--ro-bind", mask.path(), &path);
                    masks.push(mask);
                }
            }
        }
        push(&mut args, &["--clearenv"]);
        setenv(&mut args, "HOME", &home.to_string_lossy());
        setenv(&mut args, "PATH", &home.join("bin").to_string_lossy());
        setenv(&mut args, "TMPDIR", "/tmp");
        setenv(&mut args, "AX_JAILED", "1");
        setenv(&mut args, "AX_PROJECT_ROOT", &root.to_string_lossy());
        setenv(&mut args, "NIX_REMOTE", "daemon");
        setenv(
            &mut args,
            "NIX_DAEMON_SOCKET_PATH",
            &options.nix_socket.to_string_lossy(),
        );
        setenv(
            &mut args,
            "NIX_CONFIG",
            "extra-experimental-features = nix-command flakes",
        );
        for key in [
            "TERM",
            "COLORTERM",
            "LANG",
            "LC_ALL",
            "AGENTIX_MODEL",
            "AGENTIX_GATEWAY_URL",
            "AGENTIX_CLOUD_MODEL",
            "GIT_AUTHOR_NAME",
            "GIT_AUTHOR_EMAIL",
            "GIT_COMMITTER_NAME",
            "GIT_COMMITTER_EMAIL",
        ] {
            if let Some(value) = options.environment.get(key) {
                setenv(&mut args, key, value);
            }
        }
        if let Some(cert) = options.environment.get("NIX_SSL_CERT_FILE").filter(|p| {
            Path::new(p).starts_with("/nix/store") || Path::new(p).starts_with("/etc/ssl/certs")
        }) {
            setenv(&mut args, "NIX_SSL_CERT_FILE", cert);
            setenv(&mut args, "SSL_CERT_FILE", cert);
        }
        setenv(&mut args, "GIT_CONFIG_NOSYSTEM", "1");
        setenv(&mut args, "GIT_CONFIG_GLOBAL", "/dev/null");
        setenv(&mut args, "GIT_CONFIG_SYSTEM", "/dev/null");
        for (index, (key, value)) in [
            ("core.hooksPath", "/dev/null"),
            ("core.fsmonitor", "false"),
            ("credential.helper", ""),
            ("core.sshCommand", "false"),
            ("protocol.ext.allow", "never"),
        ]
        .iter()
        .enumerate()
        {
            setenv(&mut args, &format!("GIT_CONFIG_KEY_{index}"), key);
            setenv(&mut args, &format!("GIT_CONFIG_VALUE_{index}"), value);
        }
        setenv(&mut args, "GIT_CONFIG_COUNT", "5");
        push_path(&mut args, "--chdir", &cwd);
        Ok(Self {
            args,
            root,
            _masks: masks,
        })
    }
}

fn validate_grant(path: &Path, home: &Path) -> Result<()> {
    for protected in [
        home.to_path_buf(),
        PathBuf::from("/nix"),
        PathBuf::from("/etc"),
        PathBuf::from("/run"),
        PathBuf::from("/proc"),
        PathBuf::from("/dev"),
    ] {
        if protected.starts_with(path) || (protected != home && path.starts_with(&protected)) {
            bail!("mount overlaps a protected host path: {}", path.display());
        }
    }
    for name in [
        ".ssh", ".aws", ".gnupg", ".config", ".codex", ".claude", ".ax",
    ] {
        if path.starts_with(home.join(name)) {
            bail!("credential/state directories cannot be extra grants");
        }
    }
    Ok(())
}
fn git_command(git: &Path) -> Command {
    let mut command = Command::new(git);
    command
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    command
}
fn git_context(git: &Path, cwd: &Path) -> Result<Option<(PathBuf, PathBuf)>> {
    let query = |flag: &str| -> Result<Option<PathBuf>> {
        let output = git_command(git)
            .arg("-C")
            .arg(cwd)
            .args(["rev-parse", "--path-format=absolute", flag])
            .output()?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(Some(
            PathBuf::from(std::str::from_utf8(&output.stdout)?.trim()).canonicalize()?,
        ))
    };
    match (query("--show-toplevel")?, query("--git-common-dir")?) {
        (Some(root), Some(common)) => Ok(Some((root, common))),
        _ => Ok(None),
    }
}
fn push(args: &mut Vec<OsString>, values: &[&str]) {
    args.extend(values.iter().map(OsString::from));
}
fn push_path(args: &mut Vec<OsString>, flag: &str, path: &Path) {
    args.push(flag.into());
    args.push(path.into());
}
fn bind(args: &mut Vec<OsString>, mode: &str, source: &Path, target: &Path) {
    push_path(args, mode, source);
    args.push(target.into());
}
fn setenv(args: &mut Vec<OsString>, key: &str, value: &str) {
    push(args, &["--setenv", key, value]);
}
