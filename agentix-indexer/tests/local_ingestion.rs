use agentix_store::{project_filter, Store};
use anyhow::{ensure, Context, Result};
use std::{path::Path, process::Command, sync::atomic::Ordering};
#[path = "../../agentix-store/tests/support/http.rs"]
mod http;
#[path = "../../agentix-store/tests/support/mod.rs"]
mod support;
use support::Embeddings;
fn run(index: &Path, server: &Embeddings, remote: Option<&str>, args: &[&str]) -> Result<()> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ingest"));
    command
        .args(args)
        .env("AGENTIX_LANCE_PATH", index)
        .env("EMBED_MODEL", "fixture")
        .env("EMBED_DIMENSIONS", "4")
        .env("OLLAMA_HOST", &server.url)
        .env_remove("PG_DSN")
        .env_remove("AGENTIX_STORE_TOKEN_FILE");
    if let Some(url) = remote {
        command
            .env_remove("AGENTIX_LANCE_PATH")
            .env("AGENTIX_STORE_URL", url)
            .env("AGENTIX_STORE_TOKEN", http::TOKEN);
    } else {
        command
            .env_remove("AGENTIX_STORE_URL")
            .env_remove("AGENTIX_STORE_TOKEN");
    }
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "ingest failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
#[tokio::test]
async fn code_docs_retag_shorter_deleted_files_and_prune_survive_process_reopen() -> Result<()> {
    exercise(false).await
}
#[tokio::test]
async fn remote_code_docs_ingestion_retag_and_prune_use_shared_store() -> Result<()> {
    exercise(true).await
}
async fn exercise(remote: bool) -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo)?;
    std::fs::write(
        repo.join("lib.rs"),
        "pub fn firstoriginal() -> u8 { 1 }\npub fn secondobsolete() -> u8 { 2 }\n",
    )?;
    std::fs::write(repo.join("README.md"),"# Fixture\n\nThis manualmarker explains how the two fixture functions are used by callers in this small project.\n")?;
    let index = tmp.path().join("index");
    let local = Store::open(&index, "fixture", 4).await?;
    let storage = if remote {
        Some(http::StorageServer::start(local, Some(http::TOKEN.into()))?)
    } else {
        None
    };
    let remote = storage.as_ref().map(|s| s.url.as_str());
    let server = Embeddings::start()?;
    let path = repo.to_str().context("UTF8 fixture path")?;
    run(
        &index,
        &server,
        remote,
        &["code", path, "--project", "alpha"],
    )?;
    let reader = Store::open(&index, "fixture", 4).await?;
    assert_eq!(reader.count("code_chunks", "true").await?, 2);
    assert_eq!(reader.count("documents", "true").await?, 1);
    let calls = server.requests.load(Ordering::Relaxed);
    run(
        &index,
        &server,
        remote,
        &["code", path, "--project", "beta"],
    )?;
    assert_eq!(
        server.requests.load(Ordering::Relaxed),
        calls,
        "unchanged files should not be embedded again"
    );
    for tag in ["alpha", "beta"] {
        assert_eq!(
            reader
                .count("code_chunks", &project_filter(&[tag.into()]))
                .await?,
            2
        );
        assert_eq!(
            reader
                .count("documents", &project_filter(&[tag.into()]))
                .await?,
            1
        );
    }
    std::fs::write(
        repo.join("lib.rs"),
        "pub fn replacementonly() -> u8 { 3 }\n",
    )?;
    run(
        &index,
        &server,
        remote,
        &["code", path, "--project", "beta"],
    )?;
    assert_eq!(reader.count("code_chunks", "true").await?, 1);
    assert!(reader
        .rows("code_chunks", "true")
        .await?
        .iter()
        .all(|row| row["content"].as_str().is_some_and(
            |text| text.contains("replacementonly") && !text.contains("secondobsolete")
        )));
    assert!(reader
        .keyword("code_chunks", "secondobsolete", "true", 10)
        .await?
        .is_empty());
    std::fs::remove_file(repo.join("lib.rs"))?;
    run(&index, &server, remote, &["code", path])?;
    assert_eq!(reader.count("code_chunks", "true").await?, 0);
    run(
        &index,
        &server,
        remote,
        &["prune", "--dry-run", "repo", path],
    )?;
    assert_eq!(reader.count("documents", "true").await?, 1);
    run(&index, &server, remote, &["prune", "repo", path])?;
    assert_eq!(reader.count("documents", "true").await?, 0);
    Ok(())
}
