mod command;
mod conversation;
mod groceries;
mod model;
mod ports;
mod projection;
mod reminders;
pub use reminders::*;

pub use command::{Command, CommandReceipt, CommandRequest, ProjectionJob};
pub use conversation::*;
pub use groceries::*;
pub use model::{Record, RecordBody, RecordKind, SCHEMA_VERSION};
pub use ports::{Projection, Repository, SearchHit, SearchRequest};
pub use projection::{
    project_pending, rebuild_projection, search_records, ProjectionProgress, RetrievedRecord,
    SearchResults,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("record not found: {0}")]
    NotFound(uuid::Uuid),
    #[error("revision conflict: expected {expected}, actual {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("record already exists: {0}")]
    AlreadyExists(uuid::Uuid),
    #[error("request ID was already used for another command")]
    RequestConflict,
    #[error("assistant is busy or this conversation has an unresolved request")]
    Busy,
    #[error("local inference unavailable or returned an invalid decision")]
    Inference,
    #[error("OurGroceries is unavailable or returned an invalid response")]
    Groceries,
    #[error("unsupported or missing storage schema: {0}")]
    Schema(String),
    #[error("personal projection unavailable: {0}")]
    Projection(String),
    #[error(
        "storage operation failed; retry the same request ID to resolve an uncertain outcome: {0}"
    )]
    Storage(String),
}

pub type Result<T> = std::result::Result<T, Error>;
