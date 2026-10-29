use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Note,
    Task,
    List,
    ListItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordBody {
    Note {
        text: String,
    },
    Task {
        title: String,
        completed: bool,
        due_at: Option<DateTime<Utc>>,
    },
    List {
        name: String,
    },
    ListItem {
        list_id: Uuid,
        text: String,
        completed: bool,
    },
}

impl RecordBody {
    pub fn kind(&self) -> RecordKind {
        match self {
            Self::Note { .. } => RecordKind::Note,
            Self::Task { .. } => RecordKind::Task,
            Self::List { .. } => RecordKind::List,
            Self::ListItem { .. } => RecordKind::ListItem,
        }
    }

    pub fn validate(&self) -> Result<()> {
        let (text, limit) = match self {
            Self::Note { text } => (text, 65_536),
            Self::Task { title, .. } => (title, 1_024),
            Self::List { name } => (name, 256),
            Self::ListItem { list_id, text, .. } => {
                if list_id.is_nil() {
                    return Err(Error::Invalid("list ID must not be nil".into()));
                }
                (text, 1_024)
            }
        };
        if text.trim().is_empty() || text.len() > limit {
            return Err(Error::Invalid(format!(
                "text must be nonblank and at most {limit} UTF-8 bytes"
            )));
        }
        Ok(())
    }

    pub fn list_id(&self) -> Option<Uuid> {
        match self {
            Self::ListItem { list_id, .. } => Some(*list_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub schema_version: u32,
    pub id: Uuid,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted: bool,
    pub body: RecordBody,
}

impl Record {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(Error::Schema(self.schema_version.to_string()));
        }
        if self.id.is_nil() || self.revision == 0 || self.updated_at < self.created_at {
            return Err(Error::Invalid("invalid record envelope".into()));
        }
        self.body.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_bounds_are_utf8_bytes_and_do_not_destroy_original_wording() {
        let body = RecordBody::Note {
            text: "  Remember the Garage Key!  ".into(),
        };
        assert!(body.validate().is_ok());
        assert!(
            matches!(body, RecordBody::Note { text } if text == "  Remember the Garage Key!  ")
        );
        assert!(RecordBody::Note {
            text: " \n\t".into()
        }
        .validate()
        .is_err());
        assert!(RecordBody::List {
            name: "é".repeat(129)
        }
        .validate()
        .is_err());
    }

    #[test]
    fn nil_parent_is_not_a_valid_reference() {
        assert!(RecordBody::ListItem {
            list_id: Uuid::nil(),
            text: "pineapple".into(),
            completed: false
        }
        .validate()
        .is_err());
    }
}
