use crate::{CommandReceipt, CommandRequest, Error, Record, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_INSTRUCTION_BYTES: usize = 4_096;
pub const MAX_REPLY_BYTES: usize = 8_192;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TellRequest {
    pub request_id: Uuid,
    pub conversation_id: Uuid,
    pub text: String,
    pub timezone: String,
}

impl TellRequest {
    pub fn validate(&self) -> Result<()> {
        if self.request_id.is_nil() || self.conversation_id.is_nil() {
            return Err(Error::Invalid(
                "request and conversation IDs must not be nil".into(),
            ));
        }
        bounded_text(&self.text, MAX_INSTRUCTION_BYTES)?;
        if self.timezone.len() > 64 || self.timezone.parse::<chrono_tz::Tz>().is_err() {
            return Err(Error::Invalid("timezone must be an IANA timezone".into()));
        }
        Ok(())
    }
}

pub fn bounded_text(text: &str, maximum: usize) -> Result<()> {
    if text.trim().is_empty() || text.len() > maximum {
        return Err(Error::Invalid(format!(
            "text must be nonblank and at most {maximum} UTF-8 bytes"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Intent {
    AddListItem {
        list_name: String,
        list_id: Option<Uuid>,
        text: String,
    },
    ShowList {
        list_name: String,
        list_id: Option<Uuid>,
    },
    SaveNote {
        text: String,
    },
    SaveTask {
        title: String,
        due_at: Option<DateTime<Utc>>,
    },
    EditListItem {
        id: Uuid,
        expected_revision: u64,
        text: String,
    },
    SetTaskCompleted {
        id: Uuid,
        expected_revision: u64,
        completed: bool,
    },
    DeleteRecord {
        id: Uuid,
        expected_revision: u64,
    },
    Clarify {
        question: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TellStatus {
    Saved,
    Answer,
    Clarification,
    Failed,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TellResponse {
    pub request_id: Uuid,
    pub conversation_id: Uuid,
    pub status: TellStatus,
    pub reply: String,
    pub outcomes: Vec<CommandReceipt>,
    pub records: Vec<Record>,
}

impl TellResponse {
    pub fn validate(&self, request: &TellRequest) -> Result<()> {
        if self.request_id != request.request_id || self.conversation_id != request.conversation_id
        {
            return Err(Error::Storage(
                "response identity disagrees with request".into(),
            ));
        }
        bounded_text(&self.reply, MAX_REPLY_BYTES)?;
        if self.outcomes.len() > 2 || self.records.len() > 50 {
            return Err(Error::Invalid("response exceeds result limit".into()));
        }
        for receipt in &self.outcomes {
            receipt.validate()?;
        }
        for record in &self.records {
            record.validate()?;
        }
        match self.status {
            TellStatus::Saved | TellStatus::Partial if self.outcomes.is_empty() => {
                return Err(Error::Invalid("save status needs actual outcomes".into()))
            }
            TellStatus::Answer | TellStatus::Clarification | TellStatus::Failed
                if !self.outcomes.is_empty() =>
            {
                return Err(Error::Invalid(
                    "non-save status cannot carry mutations".into(),
                ))
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedPlan {
    pub commands: Vec<CommandRequest>,
    pub status: TellStatus,
    pub reply: String,
    pub records: Vec<Record>,
}

impl PreparedPlan {
    pub fn validate(&self) -> Result<()> {
        bounded_text(&self.reply, MAX_REPLY_BYTES)?;
        if self.commands.len() > 2 || self.records.len() > 50 {
            return Err(Error::Invalid("plan exceeds operation/result limit".into()));
        }
        match self.status {
            TellStatus::Saved if self.commands.is_empty() => {
                return Err(Error::Invalid("saved plan needs commands".into()))
            }
            TellStatus::Answer | TellStatus::Clarification | TellStatus::Failed
                if !self.commands.is_empty() =>
            {
                return Err(Error::Invalid("non-save plan cannot mutate".into()))
            }
            TellStatus::Partial => {
                return Err(Error::Invalid("partial is an execution outcome".into()))
            }
            _ => {}
        }
        for (index, command) in self.commands.iter().enumerate() {
            command.validate()?;
            if self.commands[..index]
                .iter()
                .any(|previous| previous.request_id == command.request_id)
            {
                return Err(Error::Invalid("duplicate command request identity".into()));
            }
        }
        for record in &self.records {
            record.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum TurnState {
    Received,
    Prepared { plan: PreparedPlan },
    Completed { response: TellResponse },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub schema_version: u32,
    pub request: TellRequest,
    pub accepted_at: DateTime<Utc>,
    pub state: TurnState,
}

impl Turn {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(Error::Schema("unsupported conversation schema".into()));
        }
        self.request.validate()?;
        match &self.state {
            TurnState::Received => Ok(()),
            TurnState::Prepared { plan } => plan.validate(),
            TurnState::Completed { response } => response.validate(&self.request),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub conversation_id: Uuid,
    pub sequence: u64,
    pub request_id: Uuid,
    pub role: MessageRole,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

impl Message {
    pub fn validate(&self) -> Result<()> {
        if self.conversation_id.is_nil() || self.request_id.is_nil() || self.sequence == 0 {
            return Err(Error::Storage(
                "invalid conversation message identity".into(),
            ));
        }
        bounded_text(&self.text, MAX_REPLY_BYTES)
    }
}

#[derive(Debug, Clone)]
pub struct InferenceContext {
    pub request: TellRequest,
    pub accepted_at: DateTime<Utc>,
    pub messages: Vec<Message>,
    pub records: Vec<Record>,
    pub context_incomplete: bool,
}

#[async_trait]
pub trait Inference: Send + Sync {
    async fn propose(&self, context: InferenceContext) -> Result<Intent>;
}

#[async_trait]
pub trait Conversations: Send + Sync {
    async fn begin(&self, request: TellRequest) -> Result<Turn>;
    async fn prepare(&self, request_id: Uuid, plan: PreparedPlan) -> Result<Turn>;
    async fn complete(&self, request_id: Uuid, response: TellResponse) -> Result<TellResponse>;
    async fn recent_messages(&self, conversation_id: Uuid, limit: usize) -> Result<Vec<Message>>;
}
