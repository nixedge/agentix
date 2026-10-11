use anyhow::Result;
use async_trait::async_trait;

/// A single tool call returned by the model.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// Model-assigned call ID, echoed back in the tool-result message.
    pub id: String,
    pub name: String,
    /// Parsed arguments object.
    pub arguments: serde_json::Value,
}

/// Implement this for every tool you want to expose to the agent.
/// Object-safe: works in `Vec<Box<dyn Tool>>`.
#[async_trait]
pub trait Tool: Send + Sync {
    fn needs_approval(&self) -> bool {
        true
    }
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema object (`"type": "object"` with `"properties"`) for the function parameters.
    fn parameters(&self) -> serde_json::Value;
    async fn call(&self, args: serde_json::Value) -> Result<String>;
    async fn call_in_context(
        &self,
        args: serde_json::Value,
        _context: agentix_api::agent::InvocationContext,
    ) -> Result<String> {
        self.call(args).await
    }
}
