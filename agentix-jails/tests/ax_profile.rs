#[path = "../src/ax_jail/profile.rs"]
mod profile;
use profile::{Profile, ProfileOptions};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn tools(path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path)?;
    for (name, source) in [
        ("bash", std::env::var("AX_TEST_SHELL")?),
        ("git", std::env::var("AX_TEST_GIT")?),
        ("env", format!("{}/env", std::env::var("AX_TEST_PATH")?)),
    ] {
        std::os::unix::fs::symlink(source, path.join(name))?;
    }
    std::os::unix::fs::symlink(std::env::var("AX_TEST_NIX")?, path.join("nix"))?;
    Ok(())
}

#[test]
fn real_jail_can_reach_the_explicit_nix_daemon_socket() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let root = home.join("project");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&root)?;
    tools(&bin)?;
    let socket = std::env::var_os("AGENTIC_NIX_DAEMON")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/nix/var/nix/daemon-socket/socket".into());
    anyhow::ensure!(
        socket.exists(),
        "Nix daemon socket required for the real jail test"
    );
    let profile = Profile::build(ProfileOptions {
        home,
        cwd: root,
        bin_dir: bin,
        nix_socket: socket,
        ro_paths: vec![],
        rw_paths: vec![],
        all_worktrees: false,
        environment: HashMap::new(),
    })?;
    let output = Command::new(std::env::var("AX_TEST_BWRAP")?)
        .args(&profile.args)
        .args(["--", "nix", "store", "ping", "--store", "daemon"])
        .output()?;
    assert!(
        output.status.success(),
        "Nix socket probe: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn real_jail_hides_sentinels_environment_and_executable_git_config() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let root = home.join("project");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&root)?;
    tools(&bin)?;
    fs::create_dir_all(home.join(".ssh"))?;
    fs::write(home.join(".ssh/id_ed25519"), "fixture-secret")?;
    fs::write(home.join("outside"), "sentinel")?;
    let git = std::env::var("AX_TEST_GIT")?;
    assert!(Command::new(&git)
        .args(["init", "-q"])
        .arg(&root)
        .status()?
        .success());
    let marker = root.join("hook-ran");
    fs::write(root.join(".git/config"),format!("[core]\nrepositoryformatversion = 0\nbare = false\nfsmonitor = !touch {}\n[include]\npath = {}\n",marker.display(),home.join(".gitconfig").display()))?;
    fs::write(
        home.join(".gitconfig"),
        "[alias]\ndanger = !touch hook-ran\n",
    )?;
    fs::write(root.join(".envrc"), "touch envrc-ran\nexport SECRET=leak\n")?;
    let options = ProfileOptions {
        home: home.clone(),
        cwd: root.clone(),
        bin_dir: bin,
        nix_socket: PathBuf::from("/nonexistent/nix.sock"),
        ro_paths: vec![],
        rw_paths: vec![],
        all_worktrees: false,
        environment: HashMap::from([
            ("SSH_AUTH_SOCK".into(), "secret-socket".into()),
            ("HTTP_PROXY".into(), "http://secret-token@proxy".into()),
            ("SECRET".into(), "secret".into()),
        ]),
    };
    let profile = Profile::build(options)?;
    assert_eq!(profile.root, root.canonicalize()?);
    let script = r#"set -eu
test ! -e "$HOME/.ssh/id_ed25519"
test ! -e "$HOME/outside"
test ! -e "$HOME/.gitconfig"
test "${SSH_AUTH_SOCK-unset}" = unset
test "${HTTP_PROXY-unset}" = unset
test "${SECRET-unset}" = unset
test -r /run/agentix/ax-profile.json
git status --porcelain
test ! -e hook-ran
test ! -e envrc-ran
test "$(git config --get core.hooksPath)" = /dev/null
printf 'isolated\n'
"#;
    let output = Command::new(std::env::var("AX_TEST_BWRAP")?)
        .args(&profile.args)
        .args(["--", "/bin/sh", "-c", script])
        .output()?;
    assert!(
        output.status.success(),
        "bwrap probe: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("isolated"));
    assert!(!marker.exists());
    Ok(())
}

#[test]
fn worktree_grants_are_explicit_and_aliases_cannot_mount_credentials() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let family = home.join("repository");
    let bare = family.join(".bare");
    let main = family.join("master");
    let sibling = family.join("worker");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&family)?;
    tools(&bin)?;
    let git = std::env::var("AX_TEST_GIT")?;
    let run = |args: &[&str]| -> anyhow::Result<()> {
        let output = Command::new(&git)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "git fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    let bare_str = bare
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF8 path"))?;
    let main_str = main
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF8 path"))?;
    let sibling_str = sibling
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF8 path"))?;
    run(&["init", "--bare", "-q", bare_str])?;
    run(&[
        "--git-dir",
        bare_str,
        "worktree",
        "add",
        "-b",
        "fixture-main",
        main_str,
    ])?;
    run(&[
        "-C",
        main_str,
        "commit",
        "--allow-empty",
        "--no-gpg-sign",
        "-m",
        "fixture",
    ])?;
    run(&[
        "--git-dir",
        bare_str,
        "worktree",
        "add",
        "-b",
        "fixture-worker",
        sibling_str,
        "fixture-main",
    ])?;
    for all_worktrees in [false, true] {
        let profile = Profile::build(ProfileOptions {
            home: home.clone(),
            cwd: main.clone(),
            bin_dir: bin.clone(),
            nix_socket: "/nonexistent/nix.sock".into(),
            ro_paths: vec![],
            rw_paths: vec![],
            all_worktrees,
            environment: HashMap::new(),
        })?;
        let expected = if all_worktrees { "-d" } else { "! -e" };
        let script=format!("set -eu; test {expected} '{sibling_str}'; git status --porcelain; test ! -w '{bare_str}/config'");
        let output = Command::new(std::env::var("AX_TEST_BWRAP")?)
            .args(&profile.args)
            .args(["--", "/bin/sh", "-c", &script])
            .output()?;
        assert!(
            output.status.success(),
            "worktree profile: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::create_dir_all(home.join(".ssh"))?;
    let alias = temp.path().join("innocent-alias");
    std::os::unix::fs::symlink(home.join(".ssh"), &alias)?;
    assert!(Profile::build(ProfileOptions {
        home,
        cwd: main,
        bin_dir: bin,
        nix_socket: "/nonexistent/nix.sock".into(),
        ro_paths: vec![alias],
        rw_paths: vec![],
        all_worktrees: false,
        environment: HashMap::new()
    })
    .is_err());
    Ok(())
}
