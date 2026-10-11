use serde::{de::Error, Deserialize, Deserializer, Serialize};

use crate::wire::{ModelId, Sha256, Text, Uuid, V1};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudProvider {
    OpenAi,
    Anthropic,
    OpenRouter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudTarget {
    pub provider: CloudProvider,
    pub model: ModelId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationContext {
    pub session_id: Uuid,
    pub turn_id: Uuid,
    pub invocation_id: Text<128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EscalationRequest {
    pub schema_version: V1,
    pub knowledge: Text<32768>,
    pub question: Text<8192>,
    pub local_insufficiency: Text<8192>,
    pub target: CloudTarget,
    pub context: InvocationContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    OnRequest,
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequest {
    pub request_id: Uuid,
    pub context: InvocationContext,
    pub tool: Text<128>,
    pub cwd: Text<4096>,
    pub arguments_hash: Sha256,
    pub effects: Text<8192>,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalResponse {
    pub request_id: Uuid,
    pub context: InvocationContext,
    pub arguments_hash: Sha256,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AgentEvent {
    SessionStarted {
        model: ModelId,
    },
    TurnStarted {
        model: ModelId,
    },
    TextDelta {
        text: String,
    },
    ToolStarted {
        invocation_id: Text<128>,
        name: Text<128>,
    },
    ToolOutput {
        invocation_id: Text<128>,
        text: String,
        truncated_bytes: u64,
    },
    ToolResult {
        invocation_id: Text<128>,
        output: String,
        success: bool,
        truncated_bytes: u64,
    },
    ApprovalRequired(ApprovalRequest),
    ModelOperation {
        operation_id: Uuid,
        phase: crate::models::OperationPhase,
    },
    ModelChanged {
        model: ModelId,
    },
    ContextCompacted {
        first_sequence: u64,
        last_sequence: u64,
    },
    WorkerStatus {
        task_id: Uuid,
        state: crate::orchestration::TaskState,
    },
    TurnCompleted {
        answer: String,
    },
    TurnFailed {
        code: Text<128>,
        message: Text<4096>,
    },
    TurnCancelled {},
}

#[derive(Debug, Clone, Serialize)]
pub struct EventEnvelope {
    pub schema_version: V1,
    pub session_id: Uuid,
    pub turn_id: Uuid,
    pub sequence: u64,
    #[serde(flatten)]
    pub event: AgentEvent,
}

impl<'de> Deserialize<'de> for EventEnvelope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Decode the envelope explicitly: flatten cannot enforce unknown-field rejection.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            schema_version: V1,
            session_id: Uuid,
            turn_id: Uuid,
            sequence: u64,
            #[serde(rename = "type")]
            kind: String,
            payload: serde_json::Value,
        }
        let raw = Raw::deserialize(deserializer)?;
        let event =
            serde_json::from_value(serde_json::json!({"type": raw.kind, "payload": raw.payload}))
                .map_err(D::Error::custom)?;
        Ok(Self {
            schema_version: raw.schema_version,
            session_id: raw.session_id,
            turn_id: raw.turn_id,
            sequence: raw.sequence,
            event,
        })
    }
}
