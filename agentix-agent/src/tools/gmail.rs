use agentix_harness::Tool;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::google::gmail::GmailClient;

pub struct GmailScanInboxTool(pub Arc<GmailClient>);

#[async_trait]
impl Tool for GmailScanInboxTool {
    // Home-assistant tools retain their existing service-level authorization.
    fn needs_approval(&self) -> bool {
        false
    }

    fn name(&self) -> &str {
        "gmail_scan_inbox"
    }

    fn description(&self) -> &str {
        "Scan the Gmail inbox for unread messages and return their subjects and snippets."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of messages to return (default 10)"
                }
            }
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let max = args["max_results"].as_u64().unwrap_or(10) as u32;
        let messages = self.0.list_unread(max).await?;
        Ok(serde_json::to_string(&messages)?)
    }
}
