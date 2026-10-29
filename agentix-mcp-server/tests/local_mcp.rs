use agentix_store::{Record, Store};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::atomic::Ordering,
};
#[path = "../../agentix-store/tests/support/http.rs"]
mod http;
#[path = "../../agentix-store/tests/support/mod.rs"]
mod support;
use support::Embeddings;
struct FixtureDirectory(std::path::PathBuf);
impl FixtureDirectory {
    fn new() -> Result<Self> {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("agentix-mcp-test-{}-{unique}", std::process::id()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn command(index: &Path, server: &Embeddings, remote: Option<(&str, bool)>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mcp-server"));
    command
        .env("AGENTIX_LANCE_PATH", index)
        .env("EMBED_MODEL", "fixture")
        .env("EMBED_DIMENSIONS", "4")
        .env("OLLAMA_HOST", &server.url)
        .env("PROJECTS", "allowed")
        .env("PG_DSN", "postgresql://127.0.0.1:1/unused")
        .env_remove("RERANK_MODEL");
    if let Some((url, authenticated)) = remote {
        command
            .env_remove("AGENTIX_LANCE_PATH")
            .env("AGENTIX_STORE_URL", url);
        if authenticated {
            command.env("AGENTIX_STORE_TOKEN", http::TOKEN);
        } else {
            command.env_remove("AGENTIX_STORE_TOKEN");
        }
    } else {
        command
            .env_remove("AGENTIX_STORE_URL")
            .env_remove("AGENTIX_STORE_TOKEN");
    }
    command.env_remove("AGENTIX_STORE_TOKEN_FILE");
    command
}
struct Mcp {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}
impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.input.take());
        let _ = self.child.wait();
    }
}
impl Mcp {
    fn job(&mut self, name: &str, arguments: Value) -> Result<Value> {
        serde_json::from_str(&self.call(100, name, arguments)?).map_err(Into::into)
    }
    fn wait_job(&mut self, id: &str) -> Result<Value> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let row = self.job("ingestion_status", json!({"job_id":id}))?;
            if !matches!(row["state"].as_str(), Some("queued" | "running")) {
                return Ok(row);
            }
            ensure!(std::time::Instant::now() < deadline, "job timed out: {row}");
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }
    fn start(mut command: Command) -> Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let input = child.stdin.take().context("MCP input")?;
        let output = BufReader::new(child.stdout.take().context("MCP output")?);
        let mut mcp = Self {
            child,
            input: Some(input),
            output,
        };
        mcp.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}))?;
        writeln!(
            mcp.input.as_mut().context("closed MCP input")?,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )?;
        Ok(mcp)
    }
    fn send(&mut self, message: Value) -> Result<Value> {
        let input = self.input.as_mut().context("closed MCP input")?;
        writeln!(input, "{message}")?;
        input.flush()?;
        let mut line = String::new();
        ensure!(
            self.output.read_line(&mut line)? > 0,
            "MCP exited before responding"
        );
        let response: Value = serde_json::from_str(&line)?;
        ensure!(response.get("error").is_none(), "MCP error: {response}");
        Ok(response)
    }
    fn call(&mut self, id: u32, name: &str, arguments: Value) -> Result<String> {
        let response=self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}))?;
        ensure!(
            response["result"]["isError"] != true,
            "tool error: {response}"
        );
        Ok(response["result"]["content"]
            .as_array()
            .context("missing tool content")?
            .iter()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"))
    }
}
#[tokio::test]
async fn cli_and_stdio_tools_use_local_storage_for_every_source() -> Result<()> {
    exercise(None).await
}
#[tokio::test]
async fn cli_and_stdio_tools_use_remote_storage_for_every_source() -> Result<()> {
    exercise(Some(true)).await
}
#[tokio::test]
async fn cli_and_stdio_tools_use_lan_storage_without_credentials() -> Result<()> {
    exercise(Some(false)).await
}
async fn exercise(remote: Option<bool>) -> Result<()> {
    let tmp = FixtureDirectory::new()?;
    let index = tmp.path().join("index");
    let store = Store::open(&index, "fixture", 4).await?;
    let authenticated = remote.unwrap_or(false);
    let storage = if remote.is_some() {
        Some(http::StorageServer::start(
            store.clone(),
            authenticated.then(|| http::TOKEN.into()),
        )?)
    } else {
        None
    };
    let remote = storage.as_ref().map(|s| (s.url.as_str(), authenticated));
    let visible = tmp.path().join("visible").to_string_lossy().into_owned();
    for (repo, project) in [(&visible, "allowed"), (&"hidden".to_owned(), "denied")] {
        store.put("code_chunks",vec![Record{row:json!({"repo_path":repo,"file_path":"main.rs","chunk_index":0,"content":"codetoken implementation","start_line":1,"end_line":1,"language":"rust","symbol_kind":"function","project":[project]}),embedding:Some(vec![1.0,0.2,0.3,0.4])}]).await?;
    }
    store.put("documents",vec![Record{row:json!({"repo_path":visible,"source_path":"README.md","chunk_index":0,"doc_kind":"readme","title":"Manual","content":"doctoken instructions","project":["allowed"]}),embedding:Some(vec![1.0,0.2,0.3,0.4])}]).await?;
    store.put("github_issues",vec![Record{row:json!({"repo":"owner/repo","number":1,"state":"closed","title":"Resolved issue","content":"issuetoken resolved"}),embedding:Some(vec![1.0,0.2,0.3,0.4])}]).await?;
    store.put("github_prs",vec![Record{row:json!({"repo":"owner/repo","number":2,"state":"merged","title":"Merged change","content":"prtoken merged"}),embedding:Some(vec![1.0,0.2,0.3,0.4])}]).await?;
    store.put("code_chunks",vec![Record{row:json!({"repo_path":"crates.io::fixture-0.0.0","file_path":"lib.rs","chunk_index":0,"content":"packagefixturetoken","language":"rust","project":["another-team"]}),embedding:Some(vec![1.0,0.2,0.3,0.4])}]).await?;
    agentix_indexer::ingest::completion::mark_package(&store, "crates.io::fixture-0.0.0", true)
        .await?;
    let server = Embeddings::start()?;
    if remote.is_some() {
        let ambiguous = command(&index, &server, remote)
            .env("AGENTIX_LANCE_PATH", tmp.path().join("ambiguous"))
            .arg("repos")
            .output()?;
        assert!(!ambiguous.status.success());
        assert!(!tmp.path().join("ambiguous").exists());
        let bad_model = command(&index, &server, remote)
            .env("EMBED_MODEL", "wrong-model")
            .arg("repos")
            .output()?;
        assert!(!bad_model.status.success());
        let fallback_home = tmp.path().join("no-local-fallback");
        let bad_url = command(&index, &server, remote)
            .env("AGENTIX_STORE_URL", "not-a-url")
            .env("XDG_DATA_HOME", &fallback_home)
            .arg("repos")
            .output()?;
        assert!(!bad_url.status.success());
        assert!(!fallback_home.exists());
    }
    let repos = command(&index, &server, remote).arg("repos").output()?;
    ensure!(repos.status.success(), "CLI repos failed");
    let output = String::from_utf8(repos.stdout)?;
    assert!(output.contains(&visible));
    assert!(!output.contains("hidden"));
    let keyword = command(&index, &server, remote)
        .args(["bm25", "codetoken"])
        .output()?;
    ensure!(keyword.status.success(), "CLI BM25 failed");
    assert!(String::from_utf8(keyword.stdout)?.contains("codetoken"));
    let mut mcp = Mcp::start(command(&index, &server, remote))?;
    let mut second = if remote.is_some() {
        Some(Mcp::start(command(&index, &server, remote))?)
    } else {
        None
    };
    if let Some(second) = second.as_mut() {
        std::thread::scope(|scope| -> Result<()> {
            let other = scope
                .spawn(|| second.call(20, "bm25_search", json!({"query":"codetoken","limit":10})));
            assert!(mcp
                .call(20, "bm25_search", json!({"query":"codetoken","limit":10}))?
                .contains("codetoken"));
            let result = other
                .join()
                .map_err(|_| anyhow::anyhow!("second MCP thread panicked"))??;
            assert!(result.contains("codetoken"));
            Ok(())
        })?;
    }
    let repos = mcp.call(2, "list_repos", json!({}))?;
    assert!(repos.contains(&visible));
    assert!(!repos.contains("hidden"));
    assert!(mcp
        .call(3, "bm25_search", json!({"query":"codetoken","limit":10}))?
        .contains("codetoken"));
    assert!(mcp.call(4,"search_code",json!({"query":"codetoken","limit":10,"language":"rust","symbol_kind":"function","repo_path":visible}))?.contains("codetoken"));
    assert!(mcp
        .call(
            5,
            "search_docs",
            json!({"query":"doctoken","limit":10,"doc_kind":"readme"})
        )?
        .contains("doctoken"));
    assert!(mcp.call(6,"search_github",json!({"query":"issuetoken","limit":10,"entity_type":"issues","repo":"owner/repo","state":"closed"}))?.contains("Resolved issue"));
    assert!(mcp
        .call(
            7,
            "search_github",
            json!({"query":"prtoken","limit":10,"entity_type":"prs","state":"merged"})
        )?
        .contains("Merged change"));
    let file = mcp.call(
        8,
        "get_file",
        json!({"repo_path":visible,"file_path":"main.rs"}),
    )?;
    assert!(file.starts_with("File written to: "), "{file}");
    let package = mcp.call(
        9,
        "fetch_package",
        json!({"package":"fixture","version":"0.0.0","ecosystem":"rust"}),
    )?;
    assert!(package.contains("Added fixture-0.0.0"), "{package}");
    if let Some(second) = second.as_mut() {
        assert!(second
            .call(
                21,
                "bm25_search",
                json!({"query":"packagefixturetoken","limit":10})
            )?
            .contains("packagefixturetoken"));
    }
    assert_eq!(
        store
            .count(
                "code_chunks",
                "repo_path = 'crates.io::fixture-0.0.0' AND array_has(projects, 'allowed')"
            )
            .await?,
        1
    );
    assert!(server.requests.load(Ordering::Relaxed) > 0);
    let listing = mcp.send(json!({"jsonrpc":"2.0","id":99,"method":"tools/list","params":{}}))?;
    let names: Vec<_> = listing["result"]["tools"]
        .as_array()
        .context("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for name in [
        "start_ingestion",
        "ingestion_status",
        "retry_ingestion",
        "cancel_ingestion",
        "index_status",
        "list_sources",
        "optimize_index",
        "prune_index",
    ] {
        assert!(names.contains(&name), "missing {name}");
    }
    let source = tmp.path().join("operational");
    std::fs::create_dir(&source)?;
    std::fs::write(
        source.join("lib.rs"),
        "pub fn operational_fixture() -> u32 { 42 }\n",
    )?;
    std::fs::write(
        source.join("README.md"),
        "# Operational fixture\n\nDocumentation for operational_fixture.\n",
    )?;
    let job = mcp.job(
        "start_ingestion",
        json!({"request":{"source":"code","repo_path":source}}),
    )?;
    // The original process remains alive, while a second client polls shared state.
    let observer = second.as_mut().unwrap_or(&mut mcp);
    let completed = observer.wait_job(job["id"].as_str().context("job id")?)?;
    assert_eq!(completed["state"], "completed", "{completed}");
    let filter = agentix_store::repo_filter(&source.to_string_lossy());
    assert!(store.count("code_chunks", &filter).await? > 0);
    assert!(store.count("documents", &filter).await? > 0);
    let registry = observer.job("list_sources", json!({"local":true}))?;
    assert!(registry.to_string().contains("operational"));
    let status = observer.job("index_status", json!({}))?;
    assert!(status["counts"]["code_chunks"].as_u64().unwrap_or(0) > 0);
    assert!(!status["maintenance"]
        .as_array()
        .context("maintenance")?
        .is_empty());
    assert!(status["maintenance"]
        .as_array()
        .context("maintenance")?
        .iter()
        .all(|row| row["collection"].is_string()));
    let optimized = observer.job(
        "optimize_index",
        json!({"collections":["code_chunks","documents"]}),
    )?;
    assert_eq!(
        observer.wait_job(optimized["id"].as_str().context("job id")?)?["state"],
        "completed"
    );
    let preview = observer.job(
        "prune_index",
        json!({"selection":{"target":"repo","repo_path":source}}),
    )?;
    let preview = observer.wait_job(preview["id"].as_str().context("job id")?)?;
    assert_eq!(preview["result"]["ingestion"]["dry_run"], true);
    assert!(store.count("code_chunks", &filter).await? > 0);
    let prune = observer.job(
        "prune_index",
        json!({"selection":{"target":"repo","repo_path":source},"dry_run":false}),
    )?;
    assert_eq!(
        observer.wait_job(prune["id"].as_str().context("job id")?)?["state"],
        "completed"
    );
    assert_eq!(store.count("code_chunks", &filter).await?, 0);
    assert_eq!(store.count("documents", &filter).await?, 0);
    store
        .set_metadata(
            "mcp_jobs",
            "stale-fixture",
            json!({"id":"stale-fixture","state":"running","heartbeat_at":1,"created_at":1,
        "request":{"source":"optimize","collections":["documents"]}}),
        )
        .await?;
    assert_eq!(
        observer.job("ingestion_status", json!({"job_id":"stale-fixture"}))?["state"],
        "interrupted"
    );
    let retried = observer.job("retry_ingestion", json!({"job_id":"stale-fixture"}))?;
    assert_ne!(retried["id"], "stale-fixture");
    assert_eq!(
        observer.wait_job(retried["id"].as_str().context("job id")?)?["state"],
        "completed"
    );
    for args in [
        vec!["init"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "fixture",
        ],
    ] {
        ensure!(
            Command::new("git")
                .current_dir(&source)
                .args(args)
                .output()?
                .status
                .success(),
            "fixture Git setup failed"
        );
    }
    let git = observer.job(
        "start_ingestion",
        json!({"request":{"source":"git","url":source}}),
    )?;
    let git = observer.wait_job(git["id"].as_str().context("job id")?)?;
    assert_eq!(git["state"], "completed", "{git}");
    assert!(store
        .metadata_rows("git_ingest")
        .await?
        .iter()
        .any(|r| r["complete"] == true));
    let package = observer.job("start_ingestion", json!({"request":{"source":"package","package":"fixture","version":"0.0.0","ecosystem":"rust"}}))?;
    assert_eq!(
        observer.wait_job(package["id"].as_str().context("job id")?)?["state"],
        "completed"
    );
    let input = tmp.path().join("export.jsonl");
    let destination = tmp.path().join("offline-import");
    std::fs::write(&input, json!({"table":"documents","row":{"repo_path":"import-fixture","source_path":"README.md","chunk_index":0,"content":"import fixture"},"embedding":null}).to_string())?;
    let imported = observer.job("start_ingestion", json!({"request":{"source":"import","input":input,"destination":destination,"model":"fixture","dimensions":4}}))?;
    let imported = observer.wait_job(imported["id"].as_str().context("job id")?)?;
    assert_eq!(imported["state"], "completed", "{imported}");
    assert_eq!(
        Store::open(&destination, "fixture", 4)
            .await?
            .count("documents", "true")
            .await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn another_mcp_can_cancel_a_slow_background_ingestion() -> Result<()> {
    let tmp = FixtureDirectory::new()?;
    let index = tmp.path().join("index");
    let store = Store::open(&index, "fixture", 4).await?;
    let storage = http::StorageServer::start(store.clone(), None)?;
    let embeddings = Embeddings::start_with_delay(std::time::Duration::from_secs(6))?;
    let source = tmp.path().join("slow");
    std::fs::create_dir(&source)?;
    std::fs::write(source.join("lib.rs"), "pub fn cancelled_fixture() {}\n")?;
    let remote = Some((storage.url.as_str(), false));
    let mut worker = Mcp::start(command(&index, &embeddings, remote))?;
    let mut observer = Mcp::start(command(&index, &embeddings, remote))?;
    let job = worker.job(
        "start_ingestion",
        json!({"request":{"source":"code","repo_path":source}}),
    )?;
    let id = job["id"].as_str().context("job id")?;
    observer.job("cancel_ingestion", json!({"job_id":id}))?;
    assert_eq!(observer.wait_job(id)?["state"], "cancelled");
    assert_eq!(store.count("code_chunks", "true").await?, 0);
    Ok(())
}
