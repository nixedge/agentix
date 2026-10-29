use crate::{Error, Record, RecordBody, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Create {
        id: Uuid,
        body: RecordBody,
    },
    Replace {
        id: Uuid,
        expected_revision: u64,
        body: RecordBody,
    },
    Delete {
        id: Uuid,
        expected_revision: u64,
    },
}

impl Command {
    pub fn id(&self) -> Uuid {
        match self {
            Self::Create { id, .. } | Self::Replace { id, .. } | Self::Delete { id, .. } => *id,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.id().is_nil() {
            return Err(Error::Invalid("record ID must not be nil".into()));
        }
        match self {
            Self::Create { body, .. } => body.validate(),
            Self::Replace {
                expected_revision,
                body,
                ..
            } => {
                Self::validate_revision(*expected_revision)?;
                body.validate()
            }
            Self::Delete {
                expected_revision, ..
            } => Self::validate_revision(*expected_revision),
        }
    }

    fn validate_revision(revision: u64) -> Result<()> {
        if revision == 0 {
            return Err(Error::Invalid("expected revision must be positive".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    pub request_id: Uuid,
    pub command: Command,
}

impl CommandRequest {
    pub fn validate(&self) -> Result<()> {
        if self.request_id.is_nil() {
            return Err(Error::Invalid("request ID must not be nil".into()));
        }
        self.command.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandReceipt {
    pub request_id: Uuid,
    pub command: Command,
    pub record: Record,
}

impl CommandReceipt {
    pub fn validate(&self) -> Result<()> {
        CommandRequest {
            request_id: self.request_id,
            command: self.command.clone(),
        }
        .validate()?;
        self.record.validate()?;
        let matches = self.record.id == self.command.id()
            && match &self.command {
                Command::Create { body, .. } => {
                    self.record.revision == 1 && !self.record.deleted && self.record.body == *body
                }
                Command::Replace {
                    expected_revision,
                    body,
                    ..
                } => {
                    expected_revision.checked_add(1) == Some(self.record.revision)
                        && !self.record.deleted
                        && self.record.body == *body
                }
                Command::Delete {
                    expected_revision, ..
                } => {
                    expected_revision.checked_add(1) == Some(self.record.revision)
                        && self.record.deleted
                }
            };
        if !matches {
            return Err(Error::Storage("receipt disagrees with its command".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionJob {
    pub record_id: Uuid,
    pub revision: u64,
}
