use std::num::{NonZeroU16, NonZeroU64};

use serde::{Deserialize, Serialize};

use crate::agent::ApprovalPolicy;
use crate::models::Revision;
use crate::wire::{IdempotencyKey, ModelId, Sha256, Text, Uuid, V1};

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoleCapability(Sha256);

impl RoleCapability {
    pub fn new(hex_token: String) -> Result<Self, crate::wire::ContractError> {
        Ok(Self(Sha256::new(hex_token)?))
    }

    pub fn expose_for_header(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Debug for RoleCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RoleCapability([REDACTED])")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRegistered {
    pub schema_version: V1,
    pub launch_scope_id: Uuid,
    pub parent_capability: RoleCapability,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Heartbeat {
    pub schema_version: V1,
    pub assignment: AssignmentRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detach {
    pub schema_version: V1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSnapshot {
    pub task_id: Uuid,
    pub state: TaskState,
    pub worker_id: Option<Uuid>,
    pub model: Option<ModelId>,
    pub assignment: Option<AssignmentRef>,
    pub input_manifest_hash: Option<Sha256>,
    pub approved_artifact_hash: Option<Sha256>,
    pub reason: Option<Text<8192>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSnapshot {
    pub schema_version: V1,
    pub run_id: Uuid,
    pub launch_scope_id: Uuid,
    pub detached: bool,
    pub tasks: Vec<TaskSnapshot>,
    pub next_cursor: Option<Text<1024>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Preparing,
    InProgress,
    Revoking,
    Reviewing,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Parent,
    Worker,
    Supervisor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerSettings {
    pub model: ModelId,
    pub quality: Option<Text<128>>,
    pub approval_policy: ApprovalPolicy,
    pub no_shell: bool,
    pub no_cloud: bool,
    pub max_calls: NonZeroU16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommittedBase {
    CleanHead {
        commit: Revision,
    },
    ExplicitCommit {
        commit: Revision,
        excluded_dirty_paths: Vec<Text<4096>>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterLaunch {
    pub schema_version: V1,
    pub repository: Text<4096>,
    pub base: CommittedBase,
    pub parent_session_id: Uuid,
    pub worker_limit: NonZeroU16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRun {
    pub schema_version: V1,
    pub idempotency_key: IdempotencyKey,
    pub launch_scope_id: Uuid,
    pub parent_session_id: Uuid,
    pub settings: WorkerSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDefinition {
    pub id: Uuid,
    pub description: Text<32768>,
    pub acceptance_criteria: Text<32768>,
    pub priority: i32,
    pub dependencies: Vec<Uuid>,
    pub target_files: Vec<Text<4096>>,
    pub preferred_model: Option<ModelId>,
    pub explicit_settings_override: Option<WorkerSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitTasks {
    pub schema_version: V1,
    pub idempotency_key: IdempotencyKey,
    pub tasks: Vec<TaskDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentRef {
    pub assignment_id: Uuid,
    pub generation: NonZeroU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub schema_version: V1,
    pub worker_id: Uuid,
    pub settings: WorkerSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub assignment: AssignmentRef,
    pub task: TaskDefinition,
    pub input_manifest_hash: Sha256,
    pub input_version: NonZeroU64,
    pub expires_at_unix_ms: u64,
    pub state: TaskState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    Success,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitResult {
    pub schema_version: V1,
    pub result_id: Uuid,
    pub assignment: AssignmentRef,
    pub outcome: TaskOutcome,
    pub summary: Text<32768>,
    pub evidence: Vec<Text<4096>>,
    pub new_files: Vec<Text<4096>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intervention {
    pub schema_version: V1,
    pub assignment: AssignmentRef,
    pub reason: Text<8192>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskMessage {
    pub schema_version: V1,
    pub message_id: Uuid,
    pub text: Text<32768>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewResult {
    pub schema_version: V1,
    pub artifact_hash: Sha256,
    pub approve: bool,
    pub notes: Option<Text<8192>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prepared {
    pub schema_version: V1,
    pub assignment: AssignmentRef,
    pub input_manifest_hash: Sha256,
    pub input_tree_hash: Sha256,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stopped {
    pub schema_version: V1,
    pub assignment: AssignmentRef,
    pub process_identity: Text<1024>,
    pub observed_at_unix_ms: u64,
    pub terminal_intent: TaskOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPath {
    pub path: Text<4096>,
    pub before: Option<Sha256>,
    pub after: Option<Sha256>,
    pub before_mode: Option<u32>,
    pub after_mode: Option<u32>,
    pub byte_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealArtifact {
    pub schema_version: V1,
    pub assignment: AssignmentRef,
    pub input_manifest_hash: Sha256,
    pub artifact_hash: Sha256,
    pub paths: Vec<ArtifactPath>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputManifest {
    pub schema_version: V1,
    pub base: Revision,
    pub ordered_artifacts: Vec<Sha256>,
    pub tree_hash: Sha256,
    pub version: NonZeroU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unblock {
    pub schema_version: V1,
    pub resolution: Text<8192>,
    pub corrected_input: Option<InputManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelTask {
    pub schema_version: V1,
    pub reason: Text<8192>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPriority {
    pub schema_version: V1,
    pub priority: i32,
}
