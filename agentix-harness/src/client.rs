use crate::tool::{Tool, ToolCall};
use anyhow::{Context, Result};
use async_trait::async_trait;

#[async_trait]
pub trait ChatBackend: Send + Sync {
    async fn complete_with_events(
        &self,
        model: &str,
        messages: &[serde_json::Value],
        tools: &[&dyn Tool],
        events: Option<&tokio::sync::mpsc::UnboundedSender<crate::event::AgentEvent>>,
    ) -> Result<ChatResponse> {
        let response = self.complete(model, messages, tools).await?;
        if let (Some(tx), Some(text)) = (events, response.content.as_ref()) {
            let _ = tx.send(crate::event::AgentEvent::TextDelta { text: text.clone() });
        }
        Ok(response)
    }
    async fn complete(
        &self,
        model: &str,
        messages: &[serde_json::Value],
        tools: &[&dyn Tool],
    ) -> Result<ChatResponse>;
}

/// Parsed response from a chat completion call.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    /// Text content of the assistant message, if any (absent when finish_reason is "tool_calls").
    pub content: Option<String>,
    /// Tool calls requested by the model; empty on plain-text responses.
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
}

#[async_trait]
impl ChatBackend for GatewayClient {
    async fn complete_with_events(
        &self,
        model: &str,
        messages: &[serde_json::Value],
        tools: &[&dyn Tool],
        events: Option<&tokio::sync::mpsc::UnboundedSender<crate::event::AgentEvent>>,
    ) -> Result<ChatResponse> {
        self.chat_events(model, messages, tools, events).await
    }
    async fn complete(
        &self,
        model: &str,
        messages: &[serde_json::Value],
        tools: &[&dyn Tool],
    ) -> Result<ChatResponse> {
        self.chat(model, messages, tools).await
    }
}

/// Thin async wrapper around the agentix-daemon OpenAI-compatible HTTP API.
pub struct GatewayClient {
    http: reqwest::Client,
    base_url: String,
}

impl GatewayClient {
    pub async fn models(&self) -> Result<Vec<agentix_api::models::ModelRecord>> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut request = self
                .http
                .get(format!("{}/v1/agentix/models", self.base_url))
                .timeout(std::time::Duration::from_secs(15))
                .query(&[("limit", "100")]);
            if let Some(cursor) = &cursor {
                request = request.query(&[("cursor", cursor)]);
            }
            let response = request.send().await.context("cannot reach model catalog")?;
            anyhow::ensure!(response.status().is_success(), "model catalog rejected: {}; this server needs the updated agentix daemon and worker", response.status());
            let page: agentix_api::models::ModelCatalog =
                agentix_api::wire::decode_request(&bounded_body(response, 1024 * 1024).await?)?;
            anyhow::ensure!(
                items.len() + page.items.len() <= 10000,
                "model catalog exceeds limit"
            );
            let next = page.next_cursor.map(|s| s.as_str().to_owned());
            if let Some(next) = &next {
                anyhow::ensure!(
                    cursor.as_ref().is_none_or(|old| old < next) && !page.items.is_empty(),
                    "invalid model catalog cursor"
                );
            }
            items.extend(page.items);
            cursor = next;
            if cursor.is_none() {
                break;
            }
        }
        let mut ids = std::collections::HashSet::new();
        anyhow::ensure!(
            items.iter().all(|m| ids.insert(m.id.as_str())),
            "duplicate model IDs"
        );
        Ok(items)
    }

    pub async fn tokenize(
        &self,
        model: &str,
        text: &str,
    ) -> Result<agentix_api::models::TokenCount> {
        let response = self
            .http
            .post(format!("{}/v1/agentix/models/tokenize", self.base_url))
            .json(&agentix_api::models::TokenizeRequest {
                schema_version: agentix_api::wire::V1,
                model_id: agentix_api::wire::Text::new(model)?,
                text: agentix_api::wire::Text::new(text)?,
            })
            .send()
            .await?;
        anyhow::ensure!(
            response.status().is_success(),
            "context check rejected: {}",
            response.status()
        );
        Ok(agentix_api::wire::decode_request(
            &bounded_body(response, 65536).await?,
        )?)
    }
    pub async fn readiness(&self, model: &str) -> Result<agentix_api::models::ModelReadiness> {
        let request = agentix_api::models::ReadinessRequest {
            schema_version: agentix_api::wire::V1,
            model_id: agentix_api::wire::Text::new(model)?,
            load: true,
        };
        let mut response = self
            .http
            .post(format!("{}/v1/agentix/models/readiness", self.base_url))
            .json(&request)
            .send()
            .await
            .context("cannot reach agentix-daemon")?;
        if !response.status().is_success() {
            anyhow::bail!(
                "readiness rejected: {}; use this branch's daemon and an installed local model ID",
                response.status()
            );
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 65536 {
                anyhow::bail!("readiness response exceeds limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(agentix_api::wire::decode_request(&bytes)?)
    }
    pub async fn escalate(
        &self,
        request: &agentix_api::agent::EscalationRequest,
    ) -> Result<String> {
        let mut response = self
            .http
            .post(format!("{}/v1/agentix/escalations", self.base_url))
            .json(request)
            .send()
            .await?;
        if !response.status().is_success() {
            anyhow::bail!("escalation rejected: {}", response.status());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
                anyhow::bail!("escalation response exceeds limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        value["answer"]
            .as_str()
            .map(str::to_string)
            .context("missing escalation answer")
    }

    pub fn new(base_url: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("agentix-harness/0.2")
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            http,
            base_url: base_url
                .trim_end_matches('/')
                .trim_end_matches("/v1")
                .to_string(),
        })
    }

    /// Send a chat completion. Pass `tools = &[]` for tool-free calls (e.g. final answer).
    pub async fn chat(
        &self,
        model: &str,
        messages: &[serde_json::Value],
        tools: &[&dyn Tool],
    ) -> Result<ChatResponse> {
        self.chat_events(model, messages, tools, None).await
    }

    pub async fn chat_events(
        &self,
        model: &str,
        messages: &[serde_json::Value],
        tools: &[&dyn Tool],
        events: Option<&tokio::sync::mpsc::UnboundedSender<crate::event::AgentEvent>>,
    ) -> Result<ChatResponse> {
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });

        if !tools.is_empty() {
            let defs: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name(),
                            "description": t.description(),
                            "parameters": t.parameters(),
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(defs);
        }

        let url = format!("{}/v1/chat/completions", self.base_url);
        let mut response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("HTTP request to gateway failed")?;
        if !response.status().is_success() {
            anyhow::bail!("gateway rejected completion: {}; run ax --check with the same model and gateway to verify local coding support", response.status());
        }
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"))
        {
            anyhow::bail!("gateway did not return an event stream");
        }
        let mut decoder = crate::streaming::SseCompletion::default();
        while let Some(chunk) = response.chunk().await? {
            decoder.push(&chunk, events)?;
        }
        decoder.finish()
    }
}

async fn bounded_body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= limit,
            "server response exceeds limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(crate) fn parse_response(json: &serde_json::Value) -> Result<ChatResponse> {
    let choice = json
        .get("choices")
        .and_then(|c| c.get(0))
        .context("no choices in response")?;

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|r| r.as_str())
        .context("missing finish reason")?
        .to_string();

    match finish_reason.as_str() {
        "stop" | "tool_calls" => {}
        "length" => {
            anyhow::bail!("model response reached its output token limit (finish_reason=length); incomplete tool calls were not executed");
        }
        _ => {
            return Err(anyhow::anyhow!("finish_reason={finish_reason:?}"))
                .context("model response ended with an unsupported finish reason");
        }
    }

    let message = choice.get("message").context("no message in choice")?;

    let content = message
        .get("content")
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut tool_calls = vec![];
    if let Some(calls) = message.get("tool_calls").and_then(|tc| tc.as_array()) {
        for call in calls {
            let id = call
                .get("id")
                .and_then(|i| i.as_str())
                .context("missing tool ID")?
                .to_string();

            let function = call.get("function").context("tool_call missing function")?;

            let name = function
                .get("name")
                .and_then(|n| n.as_str())
                .context("tool_call function missing name")?
                .to_string();

            // arguments is a JSON-encoded string on the wire, not a JSON object.
            let arguments_str = function
                .get("arguments")
                .and_then(|a| a.as_str())
                .context("missing arguments")?;
            let arguments: serde_json::Value =
                serde_json::from_str(arguments_str).context("malformed tool arguments")?;

            if call["type"] != "function"
                || id.is_empty()
                || id.len() > 128
                || name.is_empty()
                || name.len() > 128
                || !arguments.is_object()
            {
                anyhow::bail!("invalid tool call");
            }
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }

    let unique: std::collections::HashSet<_> = tool_calls.iter().map(|call| &call.id).collect();
    if unique.len() != tool_calls.len() || tool_calls.len() > 128 {
        anyhow::bail!("duplicate or excessive tool calls");
    }
    if (finish_reason == "tool_calls") == tool_calls.is_empty() {
        return Err(anyhow::anyhow!(
            "finish_reason={finish_reason:?}, received {} tool calls",
            tool_calls.len()
        ))
        .context("model response has an inconsistent finish reason");
    }
    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason,
    })
}
