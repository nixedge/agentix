use agentix_harness::{approval::Cancellation, upstream, Tool};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    num::NonZeroUsize,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};
pub struct FileSearch {
    pub root: PathBuf,
    pub cancel: Cancellation,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    query: String,
}
#[async_trait]
impl Tool for FileSearch {
    fn name(&self) -> &str {
        "file_search"
    }
    fn description(&self) -> &str {
        "Fuzzy filename search in this project using Codex file-search. Returns at most 100 matches; respects upstream ignore and symlink behavior within the jail."
    }
    fn needs_approval(&self) -> bool {
        false
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false})
    }
    async fn call(&self, args: Value) -> Result<String> {
        let args: Args = serde_json::from_value(args)?;
        let flag = Arc::new(AtomicBool::new(false));
        let results = tokio::select! {r=upstream::search_files(&self.root,&args.query,NonZeroUsize::new(100).ok_or_else(||anyhow::anyhow!("invalid limit"))?,flag)=>r?,_=self.cancel.cancelled()=>return Err(agentix_harness::approval::Cancelled.into())};
        Ok(json!({"total":results.total_match_count,"matches":results.matches.into_iter().map(|m|json!({"path":m.path,"is_directory":m.is_directory})).collect::<Vec<_>>()}).to_string())
    }
}

pub struct SearchText(pub Arc<super::access::FileAccess>);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextArgs {
    path: PathBuf,
    pattern: String,
}
#[async_trait]
impl Tool for SearchText {
    fn name(&self) -> &str {
        "search_text"
    }
    fn description(&self) -> &str {
        "Find literal text in a UTF-8 project file, returning up to 100 matching lines with line numbers. Use file_search to discover paths first."
    }
    fn needs_approval(&self) -> bool {
        false
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"pattern":{"type":"string","minLength":1,"maxLength":4096}},"required":["path","pattern"],"additionalProperties":false})
    }
    async fn call(&self, args: Value) -> Result<String> {
        let args: TextArgs = serde_json::from_value(args)?;
        anyhow::ensure!(
            !args.pattern.is_empty() && args.pattern.len() <= 4096,
            "invalid text pattern"
        );
        let fs = self.0.clone();
        tokio::task::spawn_blocking(move||{let text=fs.read(&args.path)?;let matches:Vec<_>=text.lines().enumerate().filter(|(_,line)|line.contains(&args.pattern)).take(101).map(|(line,text)|json!({"line":line+1,"text":text.chars().take(1000).collect::<String>(),"truncated":text.chars().count()>1000})).collect();let truncated=matches.len()>100;Ok(json!({"path":args.path,"matches":matches.into_iter().take(100).collect::<Vec<_>>(),"truncated":truncated}).to_string())}).await?
    }
}
