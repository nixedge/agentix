use super::access::FileAccess;
use agentix_harness::{
    upstream::{self, Hunk},
    Tool,
};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::Arc,
};
pub struct ApplyPatch(pub Arc<FileAccess>);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    patch: String,
    expected_sha256: BTreeMap<PathBuf, Option<String>>,
}
#[async_trait]
impl Tool for ApplyPatch {
    fn name(&self) -> &str {
        "apply_patch"
    }
    fn description(&self) -> &str {
        "Apply a Codex *** Begin Patch text patch after approval. expected_sha256 maps every patch path to its read_file hash, or null for new files. All preimages are checked first. Renames require separate explicit writes/deletes. Partial I/O failures report changed paths."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"patch":{"type":"string"},"expected_sha256":{"type":"object","additionalProperties":{"type":["string","null"]}}},"required":["patch","expected_sha256"],"additionalProperties":false})
    }
    async fn call(&self, args: Value) -> Result<String> {
        let args: Args = serde_json::from_value(args)?;
        let fs = self.0.clone();
        tokio::task::spawn_blocking(move || apply(&fs, args)).await?
    }
}
fn apply(fs: &FileAccess, args: Args) -> Result<String> {
    let patch = upstream::parse_patch(&args.patch)?;
    let mut edits = Vec::new();
    let mut seen = HashSet::new();
    for hunk in patch.hunks {
        let path = match &hunk {
            Hunk::AddFile { path, .. }
            | Hunk::DeleteFile { path }
            | Hunk::UpdateFile { path, .. } => path.clone(),
        };
        if !seen.insert(path.clone()) {
            bail!("duplicate patch path");
        }
        let expected = args
            .expected_sha256
            .get(&path)
            .context("missing preimage for patch path")?
            .clone();
        fs.check(&path, expected.as_deref())?;
        let content = match hunk {
            Hunk::AddFile { contents, .. } => {
                if expected.is_some() {
                    bail!("add requires a new path");
                }
                Some(contents)
            }
            Hunk::DeleteFile { .. } => {
                if expected.is_none() {
                    bail!("delete requires an existing preimage");
                }
                None
            }
            Hunk::UpdateFile {
                move_path, chunks, ..
            } => {
                if move_path.is_some() {
                    bail!("rename patches are not supported at this checkpoint");
                }
                let original = fs.read(&path)?;
                Some(upstream::update_text(
                    &path.to_string_lossy(),
                    &original,
                    &original,
                    &chunks,
                )?)
            }
        };
        edits.push((path, expected, content));
    }
    let mut changed = Vec::new();
    for (path, expected, content) in edits {
        let result = match content {
            Some(content) => fs.write(&path, &content, expected.as_deref()),
            None => fs.delete(
                &path,
                expected.as_deref().context("missing delete preimage")?,
            ),
        };
        if let Err(error) = result {
            bail!(
                "patch stopped: {error}; already changed: {}",
                serde_json::to_string(&changed)?
            );
        }
        changed.push(path);
    }
    Ok(json!({"changed":changed}).to_string())
}
