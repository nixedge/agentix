use crate::InferError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: String,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCompletionRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
}

impl ToolCompletionRequest {
    pub fn validate(&self) -> Result<(), InferError> {
        let invalid = |reason| InferError::InvalidToolExchange(reason);
        if self.messages.is_empty()
            || self.messages.len() > 4096
            || self.tools.len() > 128
            || self.max_tokens == 0
            || self
                .temperature
                .is_some_and(|t| !t.is_finite() || !(0.0..=2.0).contains(&t))
        {
            return Err(invalid("invalid request bounds"));
        }
        let mut names = HashSet::new();
        for tool in &self.tools {
            if !identifier(&tool.name)
                || !names.insert(tool.name.as_str())
                || !tool.parameters.is_object()
            {
                return Err(invalid("invalid or duplicate tool definition"));
            }
        }
        let mut seen = HashSet::new();
        let mut pending = HashSet::new();
        for message in &self.messages {
            match message {
                Message::Tool { tool_call_id, .. } => {
                    if !pending.remove(tool_call_id.as_str()) {
                        return Err(invalid("orphan or duplicate tool result"));
                    }
                }
                _ if !pending.is_empty() => return Err(invalid("missing tool results")),
                Message::Assistant { tool_calls, .. } => {
                    if tool_calls.len() > 1000 {
                        return Err(invalid("too many tool calls"));
                    }
                    for call in tool_calls {
                        // Historical tools need not remain enabled for this turn.
                        if !identifier(&call.id)
                            || !identifier(&call.name)
                            || !seen.insert(call.id.as_str())
                        {
                            return Err(invalid("invalid or reused tool call ID or name"));
                        }
                        pending.insert(call.id.as_str());
                    }
                }
                _ => {}
            }
        }
        if !pending.is_empty() {
            return Err(invalid("missing tool results"));
        }
        let bytes =
            serde_json::to_vec(self).map_err(|_| invalid("request serialization failed"))?;
        if bytes.len() > 1024 * 1024 {
            return Err(invalid("tool exchange exceeds one MiB"));
        }
        Ok(())
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolFinishReason {
    Stop,
    ToolCalls,
    Length,
    Cancelled,
}

#[derive(Debug, Clone)]
pub enum ToolCompletionEvent {
    Text(String),
    // Only complete, parsed calls cross the backend boundary.
    Calls(Vec<ToolCall>),
    Finished(ToolFinishReason),
}

#[derive(Debug, Clone)]
pub struct Cancellation(tokio::sync::watch::Sender<bool>);

impl Default for Cancellation {
    fn default() -> Self {
        Self(tokio::sync::watch::channel(false).0)
    }
}

impl Cancellation {
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }
    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
    pub async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        let _ = receiver.wait_for(|value| *value).await;
    }
}
