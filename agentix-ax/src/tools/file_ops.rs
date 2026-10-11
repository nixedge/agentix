use super::access::{digest, FileAccess};
use agentix_harness::Tool;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};

pub struct ReadFile(pub Arc<FileAccess>);
pub struct WriteFile(pub Arc<FileAccess>);
pub struct ListDir(pub Arc<FileAccess>);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArgs {
    path: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    path: PathBuf,
    content: String,
    expected_sha256: Option<String>,
}
#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read a UTF-8 file (up to 1 MiB) relative to the project. Returns content and its SHA-256 for conflict-checked edits."
    }
    fn needs_approval(&self) -> bool {
        false
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false})
    }
    async fn call(&self, args: Value) -> Result<String> {
        let args: PathArgs = serde_json::from_value(args)?;
        let fs = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let text = fs.read(&args.path)?;
            Ok(json!({"path":args.path,"sha256":digest(&text),"content":text}).to_string())
        })
        .await?
    }
}
#[async_trait]
impl Tool for ListDir {
    fn name(&self) -> &str {
        "list_dir"
    }
    fn description(&self) -> &str {
        "List up to 1000 names in a project directory. Use path '.' for the current directory."
    }
    fn needs_approval(&self) -> bool {
        false
    }
    fn parameters(&self) -> Value {
        ReadFile(self.0.clone()).parameters()
    }
    async fn call(&self, args: Value) -> Result<String> {
        let args: PathArgs = serde_json::from_value(args)?;
        let fs = self.0.clone();
        tokio::task::spawn_blocking(move || fs.list(&args.path)).await?
    }
}
#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Atomically write a UTF-8 file after approval. Supply SHA-256 from read_file for an existing file, or null for a new file. Parent directory must exist."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"expected_sha256":{"type":["string","null"]}},"required":["path","content","expected_sha256"],"additionalProperties":false})
    }
    async fn call(&self, args: Value) -> Result<String> {
        if args.get("expected_sha256").is_none() {
            anyhow::bail!("expected_sha256 is required");
        }
        let args: WriteArgs = serde_json::from_value(args)?;
        let fs = self.0.clone();
        tokio::task::spawn_blocking(move || {
            fs.write(&args.path, &args.content, args.expected_sha256.as_deref())?;
            Ok(json!({"written":args.path,"sha256":digest(&args.content)}).to_string())
        })
        .await?
    }
}
