use agentix_api::{
    agent::{AgentEvent as WireEvent, EventEnvelope},
    wire::{Text, V1},
};
use agentix_harness::{
    approval::{ApprovalRequired, Cancelled},
    AgentEvent,
};
use anyhow::{Context, Result};
use std::io::Write;
pub struct Output {
    json: bool,
    sequence: u64,
    context: Option<(uuid::Uuid, uuid::Uuid)>,
    invocation: Option<String>,
}
impl Output {
    pub fn new(json: bool) -> Self {
        Self {
            json,
            sequence: 0,
            context: None,
            invocation: None,
        }
    }
    fn wire(&mut self, event: WireEvent) -> Result<()> {
        let (session_id, turn_id) = self.context.context("missing turn event context")?;
        let envelope = EventEnvelope {
            schema_version: V1,
            session_id,
            turn_id,
            sequence: self.sequence,
            event,
        };
        serde_json::to_writer(std::io::stdout().lock(), &envelope)?;
        println!();
        self.sequence += 1;
        Ok(())
    }
    pub fn event(&mut self, event: AgentEvent) -> Result<()> {
        if !self.json {
            match event {
                AgentEvent::TextDelta { text } => {
                    print!("{}", crate::sanitize(&text));
                    std::io::stdout().flush()?;
                }
                AgentEvent::TurnStarted { .. } => {}
                event => eprintln!("{}", crate::sanitize(&format!("{event:?}"))),
            }
            return Ok(());
        }
        let wire = match event {
            AgentEvent::TurnStarted {
                session_id,
                turn_id,
                model,
            } => {
                self.context = Some((session_id, turn_id));
                WireEvent::TurnStarted {
                    model: Text::new(model)?,
                }
            }
            AgentEvent::TextDelta { text } => WireEvent::TextDelta { text },
            AgentEvent::ApprovalRequired { request } => WireEvent::ApprovalRequired(request),
            AgentEvent::ToolCallStarted {
                invocation_id,
                name,
                ..
            } => {
                self.invocation = Some(invocation_id.clone());
                WireEvent::ToolStarted {
                    invocation_id: Text::new(invocation_id)?,
                    name: Text::new(name)?,
                }
            }
            AgentEvent::ToolOutput { text, .. } => WireEvent::ToolOutput {
                invocation_id: Text::new(
                    self.invocation
                        .as_deref()
                        .context("output without invocation")?,
                )?,
                text,
                truncated_bytes: 0,
            },
            AgentEvent::ToolCallCompleted {
                invocation_id,
                result_preview,
                success,
                truncated_bytes,
                ..
            } => WireEvent::ToolResult {
                invocation_id: Text::new(invocation_id)?,
                output: result_preview,
                success,
                truncated_bytes,
            },
            AgentEvent::CloudEscalation { .. }
            | AgentEvent::StagnationDetected
            | AgentEvent::BudgetExhausted => return Ok(()),
        };
        self.wire(wire)
    }
    pub fn finish(&mut self, result: &Result<agentix_harness::AgentOutput>) -> Result<()> {
        if !self.json {
            if let Ok(output) = result {
                println!("\n{}", crate::sanitize(&output.answer));
            }
            return Ok(());
        }
        match result {
            Ok(output) => self.wire(WireEvent::TurnCompleted {
                answer: output.answer.clone(),
            }),
            Err(error) => {
                if error.is::<Cancelled>() {
                    self.wire(WireEvent::TurnCancelled {})
                } else {
                    self.wire(WireEvent::TurnFailed {
                        code: Text::new(if error.is::<ApprovalRequired>() {
                            "approval_required"
                        } else {
                            "failed"
                        })?,
                        message: Text::new(
                            error.to_string().chars().take(1000).collect::<String>(),
                        )?,
                    })
                }
            }
        }
    }
}
