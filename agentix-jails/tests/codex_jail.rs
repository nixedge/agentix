use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn script(path: &Path, body: &str) -> Result<()> {
    fs::write(path, format!("#!/bin/sh\nset -eu\n{body}\n"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

// Exercise the actual launcher, proxy server and client. Only bwrap and the
// upstream gh executable are fixtures, so this needs neither namespaces nor credentials.
#[test]
fn proxy_round_trip_and_launcher_contract() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let home = root.join("home");
    let bin = root.join("bin");
    fs::create_dir(&home)?;
    fs::create_dir(&bin)?;
    let capture = root.join("socket");
    script(&bin.join("gh"), "printf 'upstream:%s\\n' \"$*\"")?;
    script(
        &bin.join("bwrap"),
        r#"
socket=
config=
mounted=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --setenv)
            case "$2" in
                GH_PROXY_SOCKET) socket=$3 ;;
                GIT_CONFIG_GLOBAL) [ "$3" = /tmp/gitconfig ] ;;
                SSH_AUTH_SOCK) echo "unexpected SSH forwarding" >&2; exit 90 ;;
            esac
            shift 3 ;;
        --ro-bind)
            if [ "$3" = /tmp/gitconfig ]; then config=$2; fi
            shift 3 ;;
        --bind)
            case "$2" in */codex-jail-gh-*) mounted=$2 ;; esac
            shift 3 ;;
        --)
            shift
            [ "$1" = codex ]
            [ "$2" = --dangerously-bypass-hook-trust ]
            [ "$3" = --dangerously-bypass-approvals-and-sandbox ]
            [ "$4" = fixture-prompt ]
            break ;;
        *) shift ;;
    esac
done
[ -S "$socket" ]
[ "$mounted/gh.sock" = "$socket" ]
[ "$(git config --file "$config" commit.gpgsign)" = false ]
[ "$(git config --file "$config" user.name)" = Fixture ]
printf '%s' "$socket" > "$CAPTURE"
export GH_PROXY_SOCKET="$socket"
"$CLIENT" pr view 3426 --repo example/project
if "$CLIENT" pr view 3426 --repo other/project; then exit 91; fi
if "$CLIENT" api repos/example/project/releases --method POST; then exit 92; fi
exit 23
"#,
    )?;
    fs::write(
        home.join(".gitconfig"),
        "[user]\nname = Fixture\nemail = fixture@example.test\n",
    )?;
    let path = format!("{}:{}", bin.display(), std::env::var("PATH")?);
    let output = Command::new(env!("CARGO_BIN_EXE_codex-jail"))
        .current_dir(root)
        .env("HOME", &home)
        .env("PATH", path)
        .env("CODEX_JAIL_BIN_DIR", &bin)
        .env("CODEX_JAIL_BWRAP", bin.join("bwrap"))
        .env("CODEX_JAIL_GH_SERVER", env!("CARGO_BIN_EXE_gh-jail-server"))
        .env("CLIENT", env!("CARGO_BIN_EXE_gh-jail-client"))
        .env("CAPTURE", &capture)
        .env("CODEX_TEST_SECRET", "do-not-print-this")
        .env_remove("GIT_CONFIG_GLOBAL")
        .args([
            "--repo",
            "example/project",
            "--dangerous",
            "--debug",
            "--",
            "fixture-prompt",
        ])
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(23), "{stderr}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("upstream:pr view 3426"));
    assert!(stderr.contains("not in the allowed list"));
    assert!(stderr.contains("requires the --write flag"));
    assert!(!stderr.contains("do-not-print-this"));
    assert!(!stderr.contains("fixture-prompt"));
    let socket = fs::read_to_string(capture).context("fixture did not capture socket")?;
    assert!(!Path::new(&socket).exists(), "proxy socket leaked");
    assert!(!Path::new(&socket)
        .parent()
        .context("socket parent")?
        .exists());
    Ok(())
}

#[test]
fn no_auth_does_not_start_proxy() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    script(
        &root.join("bwrap"),
        r#"
for arg in "$@"; do
    [ "$arg" != GH_PROXY_SOCKET ] || exit 90
done
"#,
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_codex-jail"))
        .current_dir(root)
        .env("HOME", root)
        .env("CODEX_JAIL_BIN_DIR", root)
        .env("CODEX_JAIL_BWRAP", root.join("bwrap"))
        .env("CODEX_JAIL_GH_SERVER", "/does/not/exist")
        .args(["--no-github-auth", "--repo", "example/project"])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("could not start gh proxy"));
    Ok(())
}

#[test]
fn write_all_repos_and_ssh_are_explicit_options() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let ssh = root.join("ssh.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&ssh)?;
    script(&root.join("gh"), "printf 'upstream-ok\\n'")?;
    script(
        &root.join("bwrap"),
        r#"
socket=
ssh_env=
ssh_bind=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --setenv)
            case "$2" in
                GH_PROXY_SOCKET) socket=$3 ;;
                SSH_AUTH_SOCK) ssh_env=$3 ;;
            esac
            shift 3 ;;
        --bind)
            if [ "$2" = "$SSH_AUTH_SOCK" ]; then ssh_bind=$2; fi
            shift 3 ;;
        --) break ;;
        *) shift ;;
    esac
done
[ "$ssh_env" = "$SSH_AUTH_SOCK" ]
[ "$ssh_bind" = "$SSH_AUTH_SOCK" ]
export GH_PROXY_SOCKET="$socket"
"$CLIENT" api repos/arbitrary/project/releases --method POST
"#,
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_codex-jail"))
        .current_dir(root)
        .env("HOME", root)
        .env(
            "PATH",
            format!("{}:{}", root.display(), std::env::var("PATH")?),
        )
        .env("CODEX_JAIL_BIN_DIR", root)
        .env("CODEX_JAIL_BWRAP", root.join("bwrap"))
        .env("CODEX_JAIL_GH_SERVER", env!("CARGO_BIN_EXE_gh-jail-server"))
        .env("CLIENT", env!("CARGO_BIN_EXE_gh-jail-client"))
        .env("SSH_AUTH_SOCK", &ssh)
        .args(["--all-repos", "--write", "--allow-ssh"])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("upstream-ok"));
    Ok(())
}
