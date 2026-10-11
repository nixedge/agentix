use std::fmt;

use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
pub use uuid::Uuid;

pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

pub fn decode_request<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ContractError> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(ContractError("request exceeds 1 MiB"));
    }
    // Do not echo untrusted values (possibly secrets) in public diagnostics.
    serde_json::from_slice(bytes).map_err(|_| ContractError("invalid request schema or value"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError(pub &'static str);

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ContractError {}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct V1;

impl Serialize for V1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u16(1)
    }
}

impl<'de> Deserialize<'de> for V1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match u16::deserialize(deserializer)? {
            1 => Ok(Self),
            _ => Err(D::Error::custom("unsupported schema version")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Text<const MAX: usize>(String);

impl<const MAX: usize> Text<MAX> {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > MAX || value.contains('\0') {
            return Err(ContractError("text is empty, too long, or contains NUL"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de, const MAX: usize> Deserialize<'de> for Text<MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

pub type ModelId = Text<256>;
pub type IdempotencyKey = Text<128>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Sha256(String);

impl Sha256 {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ContractError(
                "expected 64 lowercase hexadecimal SHA-256 digits",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Sha256 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiError {
    pub code: Text<128>,
    pub message: Text<4096>,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorEnvelope {
    pub error: ApiError,
}
