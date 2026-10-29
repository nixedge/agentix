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
    Ok(())
}
