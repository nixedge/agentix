use crate::{CommandReceipt, CommandRequest, Error, ProjectionJob, Record, RecordKind, Result};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait Repository: Send + Sync {
    async fn apply(&self, request: CommandRequest) -> Result<CommandReceipt>;
    async fn get(&self, id: Uuid) -> Result<Option<Record>>;
    async fn records(&self) -> Result<Vec<Record>>;
    async fn pending_projections(&self, limit: usize) -> Result<Vec<ProjectionJob>>;
    async fn ack_projection(&self, job: ProjectionJob) -> Result<bool>;
}

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub text: String,
    pub kind: Option<RecordKind>,
    pub limit: usize,
}

impl SearchRequest {
    pub fn validate(&self) -> Result<()> {
        if self.text.trim().is_empty()
            || self.text.len() > 4_096
            || !(1..=100).contains(&self.limit)
        {
            return Err(Error::Invalid(
                "search needs nonblank text of at most 4096 bytes and limit 1..=100".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub record_id: Uuid,
    pub revision: u64,
    pub score: f64,
}

#[async_trait]
pub trait Projection: Send + Sync {
    async fn apply(&self, record: Record) -> Result<()>;
    async fn search(&self, request: SearchRequest) -> Result<Vec<SearchHit>>;
}
