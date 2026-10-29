use agentix_api::{
    ChatCompletionRequest, ChatMessage, JsonSchemaSpec, ResponseFormat, ResponseFormatType,
};
use agentix_assistant::{decision_schema, SYSTEM_PROMPT};
use agentix_assistant_core::{Error, Inference, InferenceContext, Intent, MessageRole, Result};
use agentix_router::{RouteTarget, Router};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

pub struct LocalGateway {
    http: reqwest::Client,
    url: String,
    model: String,
}

impl LocalGateway {
    pub fn new(base: &str, model: &str) -> Result<Self> {
        if model.trim().is_empty() || Router::new().route(model) != RouteTarget::Local {
            return Err(Error::Invalid("assistant requires a local model".into()));
        }
        let parsed =
            reqwest::Url::parse(base).map_err(|_| Error::Invalid("invalid gateway URL".into()))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(Error::Invalid(
                "gateway URL must have no credentials, query or fragment".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(125))
            .build()
            .map_err(|_| Error::Inference)?;
        Ok(Self {
            http,
            url: format!("{}/v1/chat/completions", base.trim_end_matches('/')),
            model: model.into(),
        })
    }

    fn completion(&self, context: InferenceContext) -> ChatCompletionRequest {
        let reference = json!({"accepted_at":context.accepted_at,"owner_timezone":context.request.timezone,
            "current_records":context.records,"context_incomplete":context.context_incomplete});
        let mut messages = vec![ChatMessage {
            role: "system".into(),
            content: Value::String(format!("{SYSTEM_PROMPT}\nReference data: {reference}")),
            name: None,
        }];
        messages.extend(context.messages.into_iter().map(|message| {
            ChatMessage {
                role: match message.role {
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                }
                .into(),
                content: Value::String(message.text),
                name: None,
            }
        }));
        ChatCompletionRequest {
            model: self.model.clone(),
            messages,
            stream: Some(false),
            max_tokens: Some(512),
            temperature: Some(0.0),
            response_format: Some(ResponseFormat {
                format_type: ResponseFormatType::JsonSchema,
                json_schema: Some(JsonSchemaSpec {
                    name: Some("personal_assistant_action".into()),
                    schema: decision_schema(),
                    strict: Some(true),
                }),
            }),
            extra: Default::default(),
        }
    }
}

#[async_trait]
impl Inference for LocalGateway {
    async fn propose(&self, context: InferenceContext) -> Result<Intent> {
        let body = self.completion(context);
        let mut response = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|_| Error::Inference)?;
        if !response.status().is_success() {
            return Err(Error::Inference);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Inference)? {
            if bytes.len() + chunk.len() > 65_536 {
                return Err(Error::Inference);
            }
            bytes.extend_from_slice(&chunk);
        }
        let response: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Inference)?;
        let choice = &response["choices"][0];
        if choice["finish_reason"] != "stop" {
            return Err(Error::Inference);
        }
        let content = choice["message"]["content"]
            .as_str()
            .ok_or(Error::Inference)?;
        if content.len() > 8_192 {
            return Err(Error::Inference);
        }
        serde_json::from_str(content).map_err(|_| Error::Inference)
    }
}
