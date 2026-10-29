use crate::{Error, Projection, Record, Repository, Result, SearchRequest};
use std::collections::HashSet;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProjectionProgress {
    pub applied: usize,
    pub acknowledged: usize,
}

pub async fn project_pending(
    repository: &dyn Repository,
    projection: &dyn Projection,
    limit: usize,
) -> Result<ProjectionProgress> {
    let mut progress = ProjectionProgress::default();
    for job in repository.pending_projections(limit).await? {
        let record = repository
            .get(job.record_id)
            .await?
            .ok_or_else(|| Error::Storage("projection job has no authoritative record".into()))?;
        record.validate()?;
        if record.id != job.record_id || record.revision < job.revision {
            return Err(Error::Storage(
                "projection job disagrees with original".into(),
            ));
        }
        projection.apply(record).await?;
        progress.applied += 1;
        if repository.ack_projection(job).await? {
            progress.acknowledged += 1;
        }
    }
    Ok(progress)
}

// The caller supplies a fresh destination. Pending jobs are intentionally retained
// so edits made after this snapshot can still catch up through the normal worker.
pub async fn rebuild_projection(
    repository: &dyn Repository,
    destination: &dyn Projection,
) -> Result<usize> {
    let records = repository.records().await?;
    let count = records.len();
    for record in records {
        destination.apply(record).await?;
    }
    Ok(count)
}

#[derive(Debug)]
pub struct RetrievedRecord {
    pub record: Record,
    pub score: f64,
}

#[derive(Debug)]
pub struct SearchResults {
    pub records: Vec<RetrievedRecord>,
    pub discarded_candidates: usize,
    pub projection_pending: bool,
}

pub async fn search_records(
    repository: &dyn Repository,
    projection: &dyn Projection,
    request: SearchRequest,
) -> Result<SearchResults> {
    request.validate()?;
    let kind = request.kind;
    let limit = request.limit;
    let candidates = projection.search(request).await?;
    let mut records = Vec::new();
    let mut discarded_candidates = 0;
    let mut seen = HashSet::new();
    for hit in candidates {
        let record = repository.get(hit.record_id).await?;
        match record {
            Some(record)
                if !record.deleted
                    && record.id == hit.record_id
                    && record.revision == hit.revision
                    && !kind.is_some_and(|kind| record.body.kind() != kind)
                    && hit.score.is_finite()
                    && seen.insert(record.id) =>
            {
                record.validate()?;
                if records.len() < limit {
                    records.push(RetrievedRecord {
                        record,
                        score: hit.score,
                    });
                }
            }
            _ => discarded_candidates += 1,
        }
    }
    let projection_pending = !repository.pending_projections(1).await?.is_empty();
    Ok(SearchResults {
        records,
        discarded_candidates,
        projection_pending,
    })
}
