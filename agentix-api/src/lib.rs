use serde::{Deserialize, Serialize};

// ── response_format types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormatType {
    JsonObject,
    JsonSchema,
    Text,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonSchemaSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub format_type: ResponseFormatType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<JsonSchemaSpec>,
}

// ── Responses API request types ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseInputContent {
    Text(String),
    Parts(Vec<ResponseInputPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseInputPart {
    #[serde(rename = "type")]
    pub part_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseInputItem {
    pub role: String,
    pub content: ResponseInputContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseTextFormat {
    #[serde(rename = "type")]
    pub format_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseTextConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<ResponseTextFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesRequest {
    pub model: String,
    pub input: Vec<ResponseInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<ResponseTextConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<serde_json::Value>,
}

// ── Responses API response types ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseOutputContent {
    OutputText { text: String },
    Refusal { refusal: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseOutputMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub id: String,
    pub role: String,
    pub status: String,
    pub content: Vec<ResponseOutputContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesResponse {
    pub id: String,
    pub object: String,
    pub model: String,
    pub output: Vec<ResponseOutputMessage>,
    pub output_text: String,
}

// ── Chat completion types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRequest {
    pub model: String,
    pub input: serde_json::Value,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResponse {
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_message_with_tool_calls_round_trips() {
        let json = serde_json::json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{"id": "call_abc", "type": "function", "function": {"name": "todo_list", "arguments": "{}"}}]
        });
        let msg: ChatMessage = serde_json::from_value(json.clone()).expect("deserialize ok");
        assert_eq!(msg.role, "assistant");
        assert!(msg.tool_calls.is_some());
        let re = serde_json::to_value(&msg).expect("serialize ok");
        assert_eq!(re["tool_calls"], json["tool_calls"]);
    }

    #[test]
    fn chat_message_with_tool_call_id_round_trips() {
        let json = serde_json::json!({
            "role": "tool",
            "content": "[{\"id\":\"1\"}]",
            "tool_call_id": "call_abc"
        });
        let msg: ChatMessage = serde_json::from_value(json).expect("deserialize ok");
        assert_eq!(msg.role, "tool");
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_abc"));
        let re = serde_json::to_value(&msg).expect("serialize ok");
        assert_eq!(re["tool_call_id"], "call_abc");
    }

    #[test]
    fn chat_completion_request_with_tools_round_trips() {
        let json = serde_json::json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "hello"}],
            "tools": [{"type": "function", "function": {"name": "f", "description": "desc", "parameters": {}}}]
        });
        let req: ChatCompletionRequest = serde_json::from_value(json).expect("deserialize ok");
        assert!(req.tools.is_some());
        let tools = req.tools.as_ref().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], "f");
    }
}

/// The only valid path to a cloud backend (Principle II).
///
/// All three fields are mandatory. A cloud call that bypasses this type and
/// goes directly to a cloud backend MUST NOT be accepted by the router.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationRequest {
    /// What the local model already knows about this problem.
    pub context: String,
    /// A precise, specific question that cannot be answered from local sources.
    pub question: String,
    /// Why local sources are insufficient for this question.
    pub local_insufficiency_reason: String,
}
