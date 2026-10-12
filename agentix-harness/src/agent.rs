use crate::approval::{ApprovalBroker, ApprovalRequired, Cancellation, Cancelled};
use anyhow::Result;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct TurnControl {
    pub cancel: Cancellation,
    pub approvals: Option<ApprovalBroker>,
    pub session_id: uuid::Uuid,
    pub cwd: PathBuf,
}

use crate::{
    client::{ChatBackend, GatewayClient},
    event::AgentEvent,
    policy::EscalationPolicy,
    session::{TurnMachine, TurnState, TurnStep},
    stagnation::StagnationDetector,
    tool::{Tool, ToolCall},
};

const STAGNATION_MESSAGE: &str =
    "You have retrieved similar information multiple times without making progress. \
     Provide a final answer from the available evidence and explain what remains \
     unresolved. Do not request further tool calls.";

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
    approval_policy: crate::approval::ApprovalPolicy,
    control: Option<TurnControl>,
    history: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl AgentLoop {
    pub fn new(gateway_url: impl Into<String>, local_model: impl Into<String>) -> Self {
        Self {
            gateway_url: gateway_url.into(),
            local_model: local_model.into(),
            tools: vec![],
            policy: EscalationPolicy::default(),
            system_prompt: None,
            approval_policy: crate::approval::ApprovalPolicy::Require,
            control: None,
            history: Arc::new(Mutex::new(vec![])),
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

    pub fn with_approval_policy(mut self, policy: crate::approval::ApprovalPolicy) -> Self {
        self.approval_policy = policy;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_control(mut self, control: TurnControl) -> Self {
        self.control = Some(control);
        self
    }
    pub fn with_history(mut self, history: Arc<Mutex<Vec<serde_json::Value>>>) -> Self {
        self.history = history;
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

    pub async fn run_with_backend(
        &self,
        prompt: &str,
        backend: &dyn ChatBackend,
    ) -> Result<AgentOutput> {
        self.run_backend(prompt, backend, None).await
    }

    async fn run_impl(
        &self,
        prompt: &str,
        event_tx: Option<&UnboundedSender<AgentEvent>>,
    ) -> Result<AgentOutput> {
        let client = GatewayClient::new(&self.gateway_url)?;
        self.run_backend(prompt, &client, event_tx).await
    }

    async fn run_backend(
        &self,
        prompt: &str,
        backend: &dyn ChatBackend,
        event_tx: Option<&UnboundedSender<AgentEvent>>,
    ) -> Result<AgentOutput> {
        let mut machine = TurnMachine::new(self.policy.max_tool_calls);
        machine.step(TurnStep::Begin)?;
        let mut messages = self
            .history
            .lock()
            .map_err(|_| anyhow::anyhow!("history lock poisoned"))?
            .clone();
        if let Some(system_prompt) = &self.system_prompt {
            let message = serde_json::json!({"role":"system","content":system_prompt});
            if messages
                .first()
                .is_some_and(|message| message["role"] == "system")
            {
                messages[0] = message;
            } else {
                messages.insert(0, message);
            }
        }
        messages.push(serde_json::json!({"role":"user","content":prompt}));
        let result = self
            .run_turn(&mut messages, backend, event_tx, &mut machine)
            .await;
        if let Err(error) = &result {
            if machine.state().is_active() {
                if error.downcast_ref::<Cancelled>().is_some() {
                    machine.step(TurnStep::Cancel)?;
                    machine.step(TurnStep::Stopped)?;
                } else {
                    machine.step(TurnStep::Fail)?;
                }
            }
        }
        if let Ok(output) = &result {
            messages.push(serde_json::json!({"role":"assistant","content":output.answer}));
        }
        *self
            .history
            .lock()
            .map_err(|_| anyhow::anyhow!("history lock poisoned"))? = messages;
        result
    }

    async fn run_turn(
        &self,
        messages: &mut Vec<serde_json::Value>,
        backend: &dyn ChatBackend,
        event_tx: Option<&UnboundedSender<AgentEvent>>,
        machine: &mut TurnMachine,
    ) -> Result<AgentOutput> {
        let tool_refs: Vec<&dyn Tool> = self.tools.iter().map(|t| t.as_ref()).collect();
        let turn_id = uuid::Uuid::new_v4();
        let session_id = self
            .control
            .as_ref()
            .map(|c| c.session_id)
            .unwrap_or_else(uuid::Uuid::new_v4);
        emit(
            event_tx,
            AgentEvent::TurnStarted {
                session_id,
                turn_id,
                model: self.local_model.clone(),
            },
        );
        let mut escalations = 0;
        let mut interventions = 0;
        let mut stagnation = StagnationDetector::new(
            self.policy.stagnation_window,
            self.policy.stagnation_min_matches,
        );
        machine.step(TurnStep::Prepared { compact: false })?;

        let mut rounds = 0usize;
        loop {
            if rounds >= self.policy.max_tool_calls.saturating_add(2) {
                anyhow::bail!("generation round budget exhausted");
            }
            rounds += 1;
            if machine.state() == TurnState::Finalizing {
                if machine.remaining() == 0 {
                    emit(event_tx, AgentEvent::BudgetExhausted);
                    messages.push(serde_json::json!({"role": "user", "content": BUDGET_MESSAGE}));
                }
                let response = self
                    .model_response(backend, messages, &[], event_tx)
                    .await?;
                if !response.tool_calls.is_empty() || response.finish_reason != "stop" {
                    anyhow::bail!("tool-free finalization did not produce a complete answer");
                }
                machine.step(TurnStep::Finish)?;
                return Ok(AgentOutput {
                    answer: response.content.unwrap_or_default(),
                    tool_calls_made: machine.used(),
                    escalations,
                    interventions,
                });
            }

            debug!(tool_calls_made = machine.used(), "calling local model");
            let response = self
                .model_response(backend, messages, &tool_refs, event_tx)
                .await?;
            if response.tool_calls.is_empty() {
                if response.finish_reason != "stop" {
                    anyhow::bail!("model response ended without a complete answer");
                }
                machine.step(TurnStep::Finish)?;
                info!(
                    tool_calls_made = machine.used(),
                    escalations, interventions, "agent loop complete"
                );
                return Ok(AgentOutput {
                    answer: response.content.unwrap_or_default(),
                    tool_calls_made: machine.used(),
                    escalations,
                    interventions,
                });
            }
            if response.finish_reason != "tool_calls"
                || response
                    .tool_calls
                    .iter()
                    .any(|call| !call.arguments.is_object())
            {
                anyhow::bail!("incomplete or malformed tool-call batch");
            }
            machine.step(TurnStep::ToolBatch(
                response
                    .tool_calls
                    .iter()
                    .map(|call| call.id.clone())
                    .collect(),
            ))?;
            machine.step(TurnStep::CallsValidated)?;
            messages.push(build_assistant_message(
                &response.content,
                &response.tool_calls,
            ));

            for call in &response.tool_calls {
                if self
                    .control
                    .as_ref()
                    .is_some_and(|control| control.cancel.is_cancelled())
                {
                    machine.step(TurnStep::Cancel)?;
                    resolve_pending(machine, messages, "Cancelled before execution")?;
                    machine.step(TurnStep::Stopped)?;
                    return Err(Cancelled.into());
                }
                if self.approval_policy == crate::approval::ApprovalPolicy::Require
                    && machine.remaining() > 0
                    && find_tool(&self.tools, &call.name).is_some_and(|tool| tool.needs_approval())
                {
                    machine.step(TurnStep::NeedApproval)?;
                    let decision = match (&self.control, event_tx) {
                        (Some(control), Some(events)) => match &control.approvals {
                            Some(broker) => {
                                let request = crate::approval::scoped_request(
                                    agentix_api::agent::InvocationContext {
                                        session_id: control.session_id,
                                        turn_id,
                                        invocation_id: agentix_api::wire::Text::new(
                                            call.id.clone(),
                                        )?,
                                    },
                                    &call.name,
                                    &control.cwd,
                                    &call.arguments,
                                )?;
                                broker.request(request, events, &control.cancel).await
                            }
                            None => Err(ApprovalRequired.into()),
                        },
                        _ => Err(ApprovalRequired.into()),
                    };
                    match decision {
                        Ok(agentix_api::agent::ApprovalDecision::Allow) => {
                            machine.step(TurnStep::ApprovalGranted)?;
                        }
                        Ok(agentix_api::agent::ApprovalDecision::Deny) => {
                            machine.step(TurnStep::ToolSkipped(call.id.clone()))?;
                            messages.push(serde_json::json!({"role":"tool","tool_call_id":call.id,"content":"Denied by user; no action executed."}));
                            emit(
                                event_tx,
                                AgentEvent::ToolCallCompleted {
                                    invocation_id: call.id.clone(),
                                    success: false,
                                    truncated_bytes: 0,
                                    name: call.name.clone(),
                                    result_preview: "Denied; no action executed".into(),
                                },
                            );
                            continue;
                        }
                        Err(error) => {
                            resolve_pending(
                                machine,
                                messages,
                                "Approval unavailable or cancelled; no action executed",
                            )?;
                            return Err(error);
                        }
                    }
                }
                let result = if machine.remaining() == 0 {
                    machine.step(TurnStep::ToolSkipped(call.id.clone()))?;
                    "Tool skipped: call budget exhausted; no action executed.".to_string()
                } else {
                    machine.step(TurnStep::StartTool(call.id.clone()))?;
                    if call.name == "ask_cloud" {
                        escalations += 1;
                        let question_preview = call.arguments["specific_question"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(80)
                            .collect();
                        emit(event_tx, AgentEvent::CloudEscalation { question_preview });
                    }
                    {
                        let args_preview = serde_json::to_string(&call.arguments)?
                            .chars()
                            .take(80)
                            .collect();
                        emit(
                            event_tx,
                            AgentEvent::ToolCallStarted {
                                invocation_id: call.id.clone(),
                                name: call.name.clone(),
                                args_preview,
                            },
                        );
                    }
                    let result = match find_tool(&self.tools, &call.name) {
                        Some(tool) => match tool
                            .call_in_context(
                                call.arguments.clone(),
                                agentix_api::agent::InvocationContext {
                                    session_id,
                                    turn_id,
                                    invocation_id: agentix_api::wire::Text::new(call.id.clone())?,
                                },
                            )
                            .await
                        {
                            Ok(result) => result,
                            Err(error) => {
                                warn!(tool = %call.name, error = %error, "tool execution failed");
                                format!("Tool error: {error}")
                            }
                        },
                        None => format!("Unknown tool: {}", call.name),
                    };
                    machine.step(TurnStep::ToolResolved(call.id.clone()))?;
                    stagnation.push(&result);
                    result
                };
                emit(
                    event_tx,
                    AgentEvent::ToolCallCompleted {
                        invocation_id: call.id.clone(),
                        success: !result.starts_with("Tool error:")
                            && !result.starts_with("Unknown tool:")
                            && !result.starts_with("Tool skipped:"),
                        truncated_bytes: result.len().saturating_sub(
                            result
                                .chars()
                                .take(16384)
                                .map(char::len_utf8)
                                .sum::<usize>(),
                        ) as u64,
                        name: call.name.clone(),
                        result_preview: result.chars().take(16384).collect(),
                    },
                );
                messages.push(serde_json::json!({
                    "role": "tool", "tool_call_id": call.id, "content": result,
                }));
            }
            if self
                .control
                .as_ref()
                .is_some_and(|control| control.cancel.is_cancelled())
            {
                machine.step(TurnStep::Cancel)?;
                machine.step(TurnStep::Stopped)?;
                return Err(Cancelled.into());
            }
            machine.step(TurnStep::Evaluate {
                stagnant: stagnation.is_stagnant(),
            })?;
            if machine.state() == TurnState::AwaitingApproval {
                interventions += 1;
                emit(event_tx, AgentEvent::StagnationDetected);
                messages.push(serde_json::json!({"role": "user", "content": STAGNATION_MESSAGE}));
                // The legacy runner has no intervention UI; finish once without tools.
                // The session controller may instead resume after explicit user input.
                machine.step(TurnStep::Finalize)?;
            }
        }
    }

    async fn model_response(
        &self,
        backend: &dyn ChatBackend,
        messages: &[serde_json::Value],
        tools: &[&dyn Tool],
        events: Option<&UnboundedSender<AgentEvent>>,
    ) -> Result<crate::client::ChatResponse> {
        let response = backend.complete_with_events(&self.local_model, messages, tools, events);
        match &self.control {
            Some(control) => {
                tokio::select! { _=control.cancel.cancelled()=>Err(Cancelled.into()), result=response=>result }
            }
            None => response.await,
        }
    }
}

fn resolve_pending(
    machine: &mut TurnMachine,
    messages: &mut Vec<serde_json::Value>,
    reason: &str,
) -> Result<()> {
    while let Some(id) = machine.next_call().map(str::to_string) {
        machine.step(TurnStep::ToolSkipped(id.clone()))?;
        messages.push(serde_json::json!({"role":"tool","tool_call_id":id,"content":reason}));
    }
    Ok(())
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
