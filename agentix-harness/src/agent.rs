use anyhow::Result;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

use crate::{
    client::GatewayClient,
    event::AgentEvent,
    policy::EscalationPolicy,
    stagnation::StagnationDetector,
    tool::{Tool, ToolCall},
};

const STAGNATION_MESSAGE: &str =
    "You have retrieved similar information multiple times without making progress. \
     You must now either: (1) call ask_cloud with a specific question you cannot \
     answer from available sources, or (2) provide your final answer based on \
     current evidence. Do not continue searching.";

const BUDGET_MESSAGE: &str =
    "You have reached your tool call budget. Provide your best final answer \
     based on what you have gathered so far. Do not request any more tool calls.";

/// Result of a completed agent run.
pub struct AgentOutput {
    /// The final text answer produced by the model.
    pub answer: String,
    /// Total tool invocations executed.
    pub tool_calls_made: usize,
    /// Number of times `ask_cloud` was called.
    pub escalations: usize,
    /// Number of times a stagnation intervention was injected.
    pub interventions: usize,
}

/// The agent loop harness.
///
/// ```rust,no_run
/// # use agentix_harness::{AgentLoop, AskCloud};
/// # #[tokio::main] async fn main() -> anyhow::Result<()> {
/// let output = AgentLoop::new("http://localhost:11434", "qwen3:32b")
///     .with_tool(Box::new(AskCloud::new("http://localhost:11434", "moonshotai/kimi-k2")?))
///     .run("Explain how X works.")
///     .await?;
/// println!("{}", output.answer);
/// # Ok(()) }
/// ```
pub struct AgentLoop {
    gateway_url: String,
    local_model: String,
    tools: Vec<Box<dyn Tool>>,
    policy: EscalationPolicy,
    system_prompt: Option<String>,
}

impl AgentLoop {
    pub fn new(gateway_url: impl Into<String>, local_model: impl Into<String>) -> Self {
        Self {
            gateway_url: gateway_url.into(),
            local_model: local_model.into(),
            tools: vec![],
            policy: EscalationPolicy::default(),
            system_prompt: None,
        }
    }

    pub fn with_tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn with_policy(mut self, policy: EscalationPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Run the agent loop, blocking until a final answer is produced or the
    /// budget is exhausted.
    pub async fn run(&self, prompt: &str) -> Result<AgentOutput> {
        self.run_impl(prompt, None).await
    }

    /// Run the agent loop, emitting [`AgentEvent`]s to `tx` as work proceeds.
    /// The sender is silently dropped if the receiver has been closed.
    pub async fn run_with_events(
        &self,
        prompt: &str,
        tx: UnboundedSender<AgentEvent>,
    ) -> Result<AgentOutput> {
        self.run_impl(prompt, Some(&tx)).await
    }

    async fn run_impl(
        &self,
        prompt: &str,
        event_tx: Option<&UnboundedSender<AgentEvent>>,
    ) -> Result<AgentOutput> {
        let client = GatewayClient::new(&self.gateway_url)?;
        let tool_refs: Vec<&dyn Tool> = self.tools.iter().map(|t| t.as_ref()).collect();

        let mut messages: Vec<serde_json::Value> = {
            let mut m = Vec::new();
            if let Some(ref sys) = self.system_prompt {
                m.push(serde_json::json!({"role": "system", "content": sys}));
            }
            m.push(serde_json::json!({"role": "user", "content": prompt}));
            m
        };

        let mut tool_calls_made: usize = 0;
        let mut escalations: usize = 0;
        let mut interventions: usize = 0;

        let mut stagnation = StagnationDetector::new(
            self.policy.stagnation_window,
            self.policy.stagnation_min_matches,
        );

        loop {
            if tool_calls_made >= self.policy.max_tool_calls {
                info!(budget = self.policy.max_tool_calls, "tool budget exhausted");
                emit(event_tx, AgentEvent::BudgetExhausted);
                messages.push(serde_json::json!({"role": "user", "content": BUDGET_MESSAGE}));

                let resp = client.chat(&self.local_model, &messages, &[]).await?;
                return Ok(AgentOutput {
                    answer: resp.content.unwrap_or_else(|| "(no final answer)".into()),
                    tool_calls_made,
                    escalations,
                    interventions,
                });
            }

            debug!(tool_calls_made, "calling local model");
            let resp = client
                .chat(&self.local_model, &messages, &tool_refs)
                .await?;

            if resp.tool_calls.is_empty() {
                let answer = resp.content.unwrap_or_else(|| "(no content)".into());
                info!(
                    tool_calls_made,
                    escalations, interventions, "agent loop complete"
                );
                return Ok(AgentOutput {
                    answer,
                    tool_calls_made,
                    escalations,
                    interventions,
                });
            }

            messages.push(build_assistant_message(&resp.content, &resp.tool_calls));

            for tc in &resp.tool_calls {
                if tool_calls_made >= self.policy.max_tool_calls {
                    break;
                }

                tool_calls_made += 1;

                // Emit a distinguished event for cloud escalations.
                if tc.name == "ask_cloud" {
                    escalations += 1;
                    let question_preview = tc.arguments["specific_question"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(80)
                        .collect::<String>();
                    emit(event_tx, AgentEvent::CloudEscalation { question_preview });
                } else {
                    let args_preview = serde_json::to_string(&tc.arguments)
                        .unwrap_or_default()
                        .chars()
                        .take(80)
                        .collect::<String>();
                    emit(
                        event_tx,
                        AgentEvent::ToolCallStarted {
                            name: tc.name.clone(),
                            args_preview,
                        },
                    );
                }

                debug!(tool = %tc.name, "executing tool");
                let result = match find_tool(&self.tools, &tc.name) {
                    Some(tool) => match tool.call(tc.arguments.clone()).await {
                        Ok(s) => s,
                        Err(e) => {
                            warn!(tool = %tc.name, error = %e, "tool execution failed");
                            format!("Tool error: {e}")
                        }
                    },
                    None => {
                        warn!(tool = %tc.name, "unknown tool requested");
                        format!("Unknown tool: {}", tc.name)
                    }
                };

                let result_preview = result.chars().take(120).collect::<String>();
                emit(
                    event_tx,
                    AgentEvent::ToolCallCompleted {
                        name: tc.name.clone(),
                        result_preview,
                    },
                );

                stagnation.push(&result);
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": result,
                }));
            }

            if stagnation.is_stagnant() {
                interventions += 1;
                warn!(interventions, "stagnation detected, injecting intervention");
                emit(event_tx, AgentEvent::StagnationDetected);
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": STAGNATION_MESSAGE,
                }));
            }
        }
    }
}

fn emit(tx: Option<&UnboundedSender<AgentEvent>>, event: AgentEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(event);
    }
}

fn build_assistant_message(content: &Option<String>, tool_calls: &[ToolCall]) -> serde_json::Value {
    let calls: Vec<serde_json::Value> = tool_calls
        .iter()
        .map(|tc| serde_json::json!({
            "id": tc.id,
            "type": "function",
            "function": {
                "name": tc.name,
                "arguments": serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".into()),
            }
        }))
        .collect();

    serde_json::json!({
        "role": "assistant",
        "content": content,
        "tool_calls": calls,
    })
}

fn find_tool<'a>(tools: &'a [Box<dyn Tool>], name: &str) -> Option<&'a dyn Tool> {
    tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
}
