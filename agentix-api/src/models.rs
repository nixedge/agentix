use std::num::NonZeroU64;

use serde::{de::Error, Deserialize, Deserializer, Serialize};

use crate::wire::{ApiError, ContractError, IdempotencyKey, ModelId, Sha256, Text, Uuid, V1};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Revision(String);

impl Revision {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        if ![40, 64].contains(&value.len())
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ContractError("expected a full lowercase commit hash"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Revision {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Residency {
    Unloaded,
    Loading,
    Ready,
    Busy,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCapability {
    Completion,
    Embedding,
    Vision,
    Transcription,
    ToolCalls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum CodingSupport {
    Supported { profile: Text<128> },
    Unsupported { reason: Text<4096> },
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HubSource {
    pub repo_id: Text<256>,
    pub revision_commit: Revision,
    pub filename: Text<1024>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRecord {
    pub id: ModelId,
    pub display_name: Text<256>,
    pub aliases: Vec<ModelId>,
    pub sha256: Sha256,
    pub byte_size: NonZeroU64,
    pub source: Option<HubSource>,
    pub format: Text<64>,
    pub architecture: Text<128>,
    pub context_limit: u32,
    pub capabilities: Vec<ModelCapability>,
    pub coding_support: CodingSupport,
    pub installed: bool,
    pub resident: Residency,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCatalog {
    pub schema_version: V1,
    pub items: Vec<ModelRecord>,
    pub next_cursor: Option<Text<1024>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HubModel {
    pub repo_id: Text<256>,
    pub display_name: Text<256>,
    pub gated: bool,
    pub license: Option<Text<256>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HubFile {
    pub source: HubSource,
    pub byte_size: Option<NonZeroU64>,
    pub sha256: Option<Sha256>,
    pub supported: bool,
    pub unsupported_reason: Option<Text<4096>>,
    pub license: Option<Text<256>>,
    pub gated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page<T> {
    pub schema_version: V1,
    pub items: Vec<T>,
    pub next_cursor: Option<Text<1024>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelOperationRequest {
    Warm {
        schema_version: V1,
        idempotency_key: IdempotencyKey,
        model_id: ModelId,
    },
    InstallAndWarm {
        schema_version: V1,
        idempotency_key: IdempotencyKey,
        repo_id: Text<256>,
        revision_commit: Revision,
        filename: Text<1024>,
        expected_sha256: Sha256,
        expected_bytes: NonZeroU64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationPhase {
    Queued,
    Resolving,
    Downloading,
    Verifying,
    Installing,
    Warming,
    Ready,
    Cancelled,
    Failed,
    Interrupted,
}

impl OperationPhase {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Cancelled | Self::Failed | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelOperation {
    pub schema_version: V1,
    pub id: Uuid,
    pub phase: OperationPhase,
    pub sequence: u64,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<NonZeroU64>,
    pub model_id: Option<ModelId>,
    pub error: Option<ApiError>,
    pub cancel_requested: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationAccepted {
    pub schema_version: V1,
    pub operation_id: Uuid,
    pub status_url: Text<1024>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenizeRequest {
    pub schema_version: V1,
    pub model_id: ModelId,
    pub text: Text<1048576>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenCount {
    pub count: u64,
    pub context_limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessRequest {
    pub schema_version: V1,
    pub model_id: ModelId,
    pub load: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelReadiness {
    pub schema_version: V1,
    pub model_id: ModelId,
    pub residency: Residency,
    pub coding_supported: bool,
    pub context_limit: u32,
}
