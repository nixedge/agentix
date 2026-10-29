use agentix_assistant_core::{
    project_pending, rebuild_projection, search_records, Command, CommandRequest, Error,
    Projection, Record, RecordBody, RecordKind, Repository, SearchRequest,
};
use agentix_assistant_lance::LanceProjection;
use agentix_assistant_sled::SledRepository;
use async_trait::async_trait;
use std::sync::Arc;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn note(text: &str) -> RecordBody {
    RecordBody::Note { text: text.into() }
}
fn command(command: Command) -> CommandRequest {
    CommandRequest {
        request_id: Uuid::new_v4(),
        command,
    }
}
async fn create(
    store: &SledRepository,
    body: RecordBody,
) -> agentix_assistant_core::Result<Record> {
    Ok(store
        .apply(command(Command::Create {
            id: Uuid::new_v4(),
            body,
        }))
        .await?
        .record)
}
async fn edit(
    store: &SledRepository,
    record: &Record,
    body: RecordBody,
) -> agentix_assistant_core::Result<Record> {
    Ok(store
        .apply(command(Command::Replace {
            id: record.id,
            expected_revision: record.revision,
            body,
        }))
        .await?
        .record)
}
async fn delete(store: &SledRepository, record: &Record) -> agentix_assistant_core::Result<Record> {
    Ok(store
        .apply(command(Command::Delete {
            id: record.id,
            expected_revision: record.revision,
        }))
        .await?
        .record)
}
fn search(text: &str) -> SearchRequest {
    SearchRequest {
        text: text.into(),
        kind: None,
        limit: 10,
    }
}

#[tokio::test]
async fn lexical_search_filters_limits_and_tracks_pending_work() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path().join("originals")).await?;
    let index = LanceProjection::open(directory.path().join("index")).await?;
    assert!(index.search(search("pineapple")).await?.is_empty());
    create(&store, note("pineapple lives in the kitchen")).await?;
    create(
        &store,
        RecordBody::Task {
            title: "buy pineapple".into(),
            completed: false,
            due_at: None,
        },
    )
    .await?;
    let before = search_records(&store, &index, search("pineapple")).await?;
    assert!(before.records.is_empty());
    assert!(before.projection_pending);
    assert_eq!(project_pending(&store, &index, 100).await?.acknowledged, 2);
    let after = search_records(&store, &index, search("pineapple")).await?;
    assert_eq!(after.records.len(), 2);
    assert!(!after.projection_pending);
    let mut request = search("pineapple' OR 1=1 --");
    request.kind = Some(RecordKind::Task);
    assert_eq!(index.search(request).await?.len(), 1);
    let mut limited = search("pineapple");
    limited.limit = 1;
    assert_eq!(index.search(limited).await?.len(), 1);
    assert!(index.search(search("!!!")).await?.is_empty());
    let mut invalid = search("pineapple");
    invalid.limit = 0;
    assert!(matches!(
        index.search(invalid).await,
        Err(Error::Invalid(_))
    ));
    Ok(())
}

#[tokio::test]
async fn revision_barriers_and_rehydration_prevent_old_text_or_deleted_records() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path().join("originals")).await?;
    let index = LanceProjection::open(directory.path().join("index")).await?;
    let v1 = create(&store, note("garage key")).await?;
    project_pending(&store, &index, 10).await?;
    let v2 = edit(&store, &v1, note("kitchen key")).await?;
    let stale = search_records(&store, &index, search("garage")).await?;
    assert!(stale.records.is_empty());
    assert_eq!(stale.discarded_candidates, 1);
    assert!(stale.projection_pending);
    index.apply(v2.clone()).await?;
    index.apply(v1.clone()).await?;
    assert!(index.search(search("garage")).await?.is_empty());
    assert_eq!(index.search(search("kitchen")).await?[0].revision, 2);
    let mut conflicting = v2.clone();
    conflicting.body = note("contradictory revision");
    assert!(matches!(
        index.apply(conflicting).await,
        Err(Error::Projection(_))
    ));
    let tombstone = delete(&store, &v2).await?;
    assert!(search_records(&store, &index, search("kitchen"))
        .await?
        .records
        .is_empty());
    index.apply(tombstone.clone()).await?;
    index.apply(v2).await?;
    index.apply(v1).await?;
    index.apply(tombstone).await?;
    assert!(index.search(search("key")).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn restart_between_projection_and_ack_replays_and_rebuilds_into_fresh_index() -> TestResult {
    let directory = tempfile::tempdir()?;
    let original_path = directory.path().join("originals");
    let index_path = directory.path().join("index");
    let original;
    let deleted;
    {
        let store = SledRepository::open(&original_path).await?;
        let index = LanceProjection::open(&index_path).await?;
        original = create(&store, note("pineapple in pantry")).await?;
        deleted = create(&store, note("obsolete pineapple")).await?;
        delete(&store, &deleted).await?;
        index.apply(original.clone()).await?;
        assert_eq!(store.pending_projections(10).await?.len(), 2);
        store.close().await?;
    }
    let store = SledRepository::open(&original_path).await?;
    let index = LanceProjection::open(&index_path).await?;
    let progress = project_pending(&store, &index, 10).await?;
    assert_eq!(progress.acknowledged, 2);
    assert_eq!(index.search(search("pineapple")).await?.len(), 1);
    let fresh_path = directory.path().join("rebuilt");
    let fresh = LanceProjection::open_fresh(&fresh_path).await?;
    assert_eq!(rebuild_projection(&store, &fresh).await?, 2);
    let results = search_records(&store, &fresh, search("pineapple")).await?;
    assert_eq!(results.records.len(), 1);
    assert_eq!(results.records[0].record, original);
    assert!(store.pending_projections(10).await?.is_empty());
    fresh.apply(deleted).await?;
    assert!(fresh.search(search("obsolete")).await?.is_empty());
    drop(fresh);
    assert!(matches!(
        LanceProjection::open_fresh(&fresh_path).await,
        Err(Error::Schema(_))
    ));
    Ok(())
}

struct Unavailable;
#[async_trait]
impl Projection for Unavailable {
    async fn apply(&self, _: Record) -> agentix_assistant_core::Result<()> {
        Err(Error::Projection("synthetic outage".into()))
    }
    async fn search(
        &self,
        _: SearchRequest,
    ) -> agentix_assistant_core::Result<Vec<agentix_assistant_core::SearchHit>> {
        Err(Error::Projection("synthetic outage".into()))
    }
}

#[tokio::test]
async fn projection_outage_never_undoes_a_save_or_consumes_its_job() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path().join("originals")).await?;
    let record = create(&store, note("saved despite retrieval outage")).await?;
    assert!(project_pending(&store, &Unavailable, 10).await.is_err());
    assert_eq!(store.get(record.id).await?, Some(record));
    assert_eq!(store.pending_projections(10).await?.len(), 1);
    let index = LanceProjection::open(directory.path().join("index")).await?;
    assert_eq!(project_pending(&store, &index, 10).await?.acknowledged, 1);
    Ok(())
}

struct EditWhileProjecting {
    store: SledRepository,
    index: LanceProjection,
}
#[async_trait]
impl Projection for EditWhileProjecting {
    async fn apply(&self, record: Record) -> agentix_assistant_core::Result<()> {
        edit(&self.store, &record, note("new revision during projection")).await?;
        self.index.apply(record).await
    }
    async fn search(
        &self,
        request: SearchRequest,
    ) -> agentix_assistant_core::Result<Vec<agentix_assistant_core::SearchHit>> {
        self.index.search(request).await
    }
}

#[tokio::test]
async fn edit_during_projection_preserves_new_job_and_hides_old_candidate() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path().join("originals")).await?;
    let index = LanceProjection::open(directory.path().join("index")).await?;
    create(&store, note("old revision")).await?;
    let racing = EditWhileProjecting {
        store: store.clone(),
        index: index.clone(),
    };
    let progress = project_pending(&store, &racing, 1).await?;
    assert_eq!(progress.applied, 1);
    assert_eq!(progress.acknowledged, 0);
    assert_eq!(store.pending_projections(1).await?[0].revision, 2);
    assert!(search_records(&store, &index, search("old"))
        .await?
        .records
        .is_empty());
    assert_eq!(project_pending(&store, &index, 1).await?.acknowledged, 1);
    assert_eq!(
        search_records(&store, &index, search("new"))
            .await?
            .records
            .len(),
        1
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_owner_is_rejected_and_clones_serialize_out_of_order_writes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path().join("originals")).await?;
    let path = directory.path().join("index");
    let index = LanceProjection::open(&path).await?;
    assert!(matches!(
        LanceProjection::open(&path).await,
        Err(Error::Projection(_))
    ));
    let v1 = create(&store, note("first wording")).await?;
    let v2 = edit(&store, &v1, note("latest wording")).await?;
    let mut tasks = Vec::new();
    for record in [v2.clone(), v1.clone(), v2.clone(), v1] {
        let index = index.clone();
        tasks.push(tokio::spawn(async move { index.apply(record).await }));
    }
    for task in tasks {
        task.await??;
    }
    assert_eq!(index.search(search("latest")).await?[0].revision, 2);
    assert!(index.search(search("first")).await?.is_empty());
    drop(index);
    assert_eq!(
        LanceProjection::open(&path)
            .await?
            .search(search("latest"))
            .await?
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn incompatible_profile_dimension_and_schema_are_rejected_even_when_empty() -> TestResult {
    use arrow_array::RecordBatch;
    use arrow_schema::Schema;
    use std::collections::HashMap;
    let template = tempfile::tempdir()?;
    drop(LanceProjection::open(template.path()).await?);
    let template_connection = lancedb::connect(template.path().to_str().ok_or("invalid path")?)
        .execute()
        .await?;
    let valid_schema = template_connection
        .open_table("personal_records")
        .execute()
        .await?
        .schema()
        .await?;
    for (profile, dimensions, version) in [
        ("different-model", "0", "1"),
        ("personal-lexical-v1", "10", "1"),
        ("personal-lexical-v1", "0", "99"),
    ] {
        let directory = tempfile::tempdir()?;
        let connection = lancedb::connect(directory.path().to_str().ok_or("invalid path")?)
            .execute()
            .await?;
        let schema = Arc::new(Schema::new_with_metadata(
            valid_schema.fields().clone(),
            HashMap::from([
                ("agentix.personal.schema".into(), version.into()),
                ("agentix.personal.profile".into(), profile.into()),
                ("agentix.personal.dimensions".into(), dimensions.into()),
            ]),
        ));
        let batch = RecordBatch::new_empty(Arc::clone(&schema));
        connection
            .create_table("personal_records", batch)
            .execute()
            .await?;
        drop(connection);
        assert!(matches!(
            LanceProjection::open(directory.path()).await,
            Err(Error::Schema(_))
        ));
    }
    Ok(())
}

struct PauseAfterApply {
    index: LanceProjection,
    applied: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl Projection for PauseAfterApply {
    async fn apply(&self, record: Record) -> agentix_assistant_core::Result<()> {
        self.index.apply(record).await?;
        self.applied.notify_one();
        std::future::pending().await
    }

    async fn search(
        &self,
        request: SearchRequest,
    ) -> agentix_assistant_core::Result<Vec<agentix_assistant_core::SearchHit>> {
        self.index.search(request).await
    }
}

#[tokio::test]
async fn cancelled_worker_keeps_committed_projection_replayable_until_acknowledged() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path().join("originals")).await?;
    let index = LanceProjection::open(directory.path().join("index")).await?;
    create(&store, note("remember pineapple")).await?;
    let applied = Arc::new(tokio::sync::Notify::new());
    let worker = {
        let store = store.clone();
        let projection = PauseAfterApply {
            index: index.clone(),
            applied: Arc::clone(&applied),
        };
        tokio::spawn(async move { project_pending(&store, &projection, 1).await })
    };
    applied.notified().await;
    worker.abort();
    assert!(worker.await.is_err_and(|error| error.is_cancelled()));
    assert_eq!(index.search(search("pineapple")).await?.len(), 1);
    assert_eq!(store.pending_projections(10).await?.len(), 1);
    assert_eq!(project_pending(&store, &index, 1).await?.acknowledged, 1);
    Ok(())
}
