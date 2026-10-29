use crate::{apply, decode, keys, storage, SledRepository};
use agentix_assistant_core::{
    CommandReceipt, CommandRequest, Error, ProjectionJob, Record, Repository, Result,
};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
impl Repository for SledRepository {
    async fn apply(&self, request: CommandRequest) -> Result<CommandReceipt> {
        self.blocking(move |db| apply::apply(db, request)).await
    }

    async fn get(&self, id: Uuid) -> Result<Option<Record>> {
        self.blocking(move |db| apply::load_record(db, id)).await
    }

    async fn records(&self) -> Result<Vec<Record>> {
        self.blocking(|db| {
            db.scan_prefix(keys::RECORDS)
                .map(|entry| {
                    let (key, bytes) = entry.map_err(storage)?;
                    let record: Record = decode(&bytes)?;
                    record.validate()?;
                    if keys::record(record.id).as_slice() != key.as_ref() {
                        return Err(Error::Storage("record identity disagrees with key".into()));
                    }
                    Ok(record)
                })
                .collect()
        })
        .await
    }

    async fn pending_projections(&self, limit: usize) -> Result<Vec<ProjectionJob>> {
        if !(1..=1_000).contains(&limit) {
            return Err(Error::Invalid(
                "projection limit must be in 1..=1000".into(),
            ));
        }
        self.blocking(move |db| {
            db.scan_prefix(keys::PROJECTIONS)
                .take(limit)
                .map(|entry| {
                    let (key, bytes) = entry.map_err(storage)?;
                    let job: ProjectionJob = decode(&bytes)?;
                    if job.record_id.is_nil()
                        || job.revision == 0
                        || keys::projection(job.record_id).as_slice() != key.as_ref()
                    {
                        return Err(Error::Storage("invalid projection job".into()));
                    }
                    Ok(job)
                })
                .collect()
        })
        .await
    }

    async fn ack_projection(&self, job: ProjectionJob) -> Result<bool> {
        if job.record_id.is_nil() || job.revision == 0 {
            return Err(Error::Invalid("invalid projection acknowledgement".into()));
        }
        self.blocking(move |db| {
            let key = keys::projection(job.record_id);
            let current: Option<ProjectionJob> = db
                .get(&key)
                .map_err(storage)?
                .map(|bytes| decode(&bytes))
                .transpose()?;
            let matches = current.as_ref() == Some(&job);
            if matches {
                db.remove(key).map_err(storage)?;
            }
            db.flush().map_err(storage)?;
            Ok(matches)
        })
        .await
    }
}
