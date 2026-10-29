use agentix_store::Store;
use anyhow::{ensure, Context, Result};
use std::process::{Child, Command, Stdio};
struct ServerProcess(Child);
impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[tokio::test]
async fn server_binary_loads_runtime_credential_file_and_serves_remote_writes() -> Result<()> {
    exercise(true).await
}
#[tokio::test]
async fn server_binary_starts_without_a_token_for_lan_clients() -> Result<()> {
    exercise(false).await
}
async fn exercise(authenticated: bool) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let token = "process-fixture-token-with-more-than-32-characters";
    let file = dir.path().join("token");
    std::fs::write(&file, format!("{token}\n"))?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentix-store-server"));
    command
        .env_remove("AGENTIX_STORE_URL")
        .env_remove("AGENTIX_STORE_TOKEN")
        .env_remove("PG_DSN")
        .env("AGENTIX_LANCE_PATH", dir.path().join("index"))
        .env_remove("AGENTIX_STORE_TOKEN_FILE")
        .env("EMBED_MODEL", "fixture")
        .env("EMBED_DIMENSIONS", "4")
        .env("AGENTIX_STORE_HOST", "127.0.0.1")
        .env("AGENTIX_STORE_PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    if authenticated {
        command.env("AGENTIX_STORE_TOKEN_FILE", &file);
    }
    let mut child = ServerProcess(command.spawn()?);
    let url = format!("http://127.0.0.1:{port}");
    let mut connected = None;
    for _ in 0..100 {
        ensure!(
            child.0.try_wait()?.is_none(),
            "storage process exited early"
        );
        if let Ok(store) =
            Store::connect(&url, authenticated.then(|| token.into()), "fixture", 4).await
        {
            connected = Some(store);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    let remote = connected.context("storage process did not become ready")?;
    remote
        .set_metadata("fixture", "key", serde_json::json!({"value":1}))
        .await?;
    assert_eq!(
        remote
            .metadata("fixture", "key")
            .await?
            .context("metadata missing")?["value"],
        1
    );
    assert!(
        Store::connect(&url, authenticated.then(|| token.into()), "wrong-model", 4)
            .await
            .is_err()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn invalid_configured_tokens_do_not_enable_unauthenticated_access() -> Result<()> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let dir = tempfile::tempdir()?;
    for token in [
        OsString::from(""),
        OsString::from("short"),
        OsString::from_vec(vec![0xff]),
    ] {
        let index = dir.path().join("must-not-open");
        let output = Command::new(env!("CARGO_BIN_EXE_agentix-store-server"))
            .env_remove("AGENTIX_STORE_URL")
            .env_remove("AGENTIX_STORE_TOKEN_FILE")
            .env("AGENTIX_STORE_TOKEN", token)
            .env("AGENTIX_LANCE_PATH", &index)
            .output()?;
        ensure!(
            !output.status.success(),
            "invalid configured token was accepted"
        );
        assert!(
            !index.exists(),
            "invalid token must fail before opening the index"
        );
    }
    Ok(())
}
