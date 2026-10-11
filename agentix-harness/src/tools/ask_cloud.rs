use crate::{client::GatewayClient, tool::Tool};
use anyhow::Result;
use async_trait::async_trait;

/// Built-in tool that escalates a specific question to a cloud model via the gateway.
///
/// The daemon validates the structured rationale and its configured cloud policy.
///
/// Three required fields force the model to articulate the knowledge gap before
/// escalating, preventing lazy escalation ("I don't know" → cloud).
pub struct AskCloud {
    client: GatewayClient,
    cloud_model: String,
    context: agentix_api::agent::InvocationContext,
}

impl AskCloud {
    pub fn new(gateway_url: &str, cloud_model: impl Into<String>) -> Result<Self> {
        Ok(Self {
            client: GatewayClient::new(gateway_url)?,
            cloud_model: cloud_model.into(),
            context: agentix_api::agent::InvocationContext {
                session_id: uuid::Uuid::new_v4(),
                turn_id: uuid::Uuid::new_v4(),
                invocation_id: agentix_api::wire::Text::new("ask_cloud")?,
            },
        })
    }

    pub fn with_context(mut self, context: agentix_api::agent::InvocationContext) -> Self {
        self.context = context;
        self
    }
}

#[async_trait]
impl Tool for AskCloud {
    fn name(&self) -> &str {
        "ask_cloud"
    }

    fn description(&self) -> &str {
        "Escalate a specific technical question to a capable cloud model when local reasoning \
         or available sources are insufficient. You must supply: what you already know, a precise \
         question, and why you cannot answer it from local tools."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "what_i_already_know": {
                    "type": "string",
                    "description": "Summarise the evidence and context gathered so far."
                },
                "specific_question": {
                    "type": "string",
                    "description": "The exact technical question that needs answering."
                },
                "why_i_cant_answer_locally": {
                    "type": "string",
                    "description": "Why local tools or the available sources cannot answer this."
                }
            },
            "required": ["what_i_already_know", "specific_question", "why_i_cant_answer_locally"]
        })
    }

    async fn call(&self, args: serde_json::Value) -> Result<String> {
        self.call_in_context(args, self.context.clone()).await
    }
    async fn call_in_context(
        &self,
        args: serde_json::Value,
        invocation: agentix_api::agent::InvocationContext,
    ) -> Result<String> {
        let context = args["what_i_already_know"].as_str().unwrap_or("");
        let question = args["specific_question"].as_str().unwrap_or("");
        let why = args["why_i_cant_answer_locally"].as_str().unwrap_or("");

        use agentix_api::{
            agent::{CloudProvider, CloudTarget, EscalationRequest},
            wire::{Text, V1},
        };
        let request = EscalationRequest {
            schema_version: V1,
            knowledge: Text::new(context)?,
            question: Text::new(question)?,
            local_insufficiency: Text::new(why)?,
            target: CloudTarget {
                provider: CloudProvider::OpenRouter,
                model: Text::new(self.cloud_model.clone())?,
            },
            context: invocation,
        };
        self.client.escalate(&request).await
    }
}
