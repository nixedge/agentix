use agentix_assistant_core::{
    Command, CommandRequest, Error, ProjectionJob, RecordBody, Repository,
};
use agentix_assistant_sled::SledRepository;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn note(text: &str) -> RecordBody {
    RecordBody::Note { text: text.into() }
}
fn request(command: Command) -> CommandRequest {
    CommandRequest {
        request_id: Uuid::new_v4(),
        command,
    }
}
fn create(id: Uuid, body: RecordBody) -> CommandRequest {
    request(Command::Create { id, body })
}
fn replace(id: Uuid, revision: u64, body: RecordBody) -> CommandRequest {
    request(Command::Replace {
        id,
        expected_revision: revision,
        body,
    })
}
fn delete(id: Uuid, revision: u64) -> CommandRequest {
    request(Command::Delete {
        id,
        expected_revision: revision,
    })
}

#[tokio::test]
async fn receipt_replay_is_durable_and_preserves_original_result_after_edits() -> TestResult {
    let directory = tempfile::tempdir()?;
    let id = Uuid::new_v4();
    let command = create(id, note("Keep the spare key in the garage"));
    let receipt;
    {
        let store = SledRepository::open(directory.path()).await?;
        receipt = store.apply(command.clone()).await?;
        assert_eq!(store.apply(command.clone()).await?, receipt);
        store
            .apply(replace(id, 1, note("Keep it in the kitchen")))
            .await?;
        assert_eq!(store.apply(command.clone()).await?, receipt);
        let mut conflicting = command.clone();
        conflicting.command = Command::Create {
            id,
            body: note("different instruction"),
        };
        assert!(matches!(
            store.apply(conflicting).await,
            Err(Error::RequestConflict)
        ));
        store.close().await?;
    }
    let store = SledRepository::open(directory.path()).await?;
    assert_eq!(store.apply(command).await?, receipt);
    assert_eq!(store.records().await?.len(), 1);
    assert_eq!(store.get(id).await?.ok_or("missing record")?.revision, 2);
    assert_eq!(
        store.pending_projections(10).await?,
        vec![ProjectionJob {
            record_id: id,
            revision: 2
        }]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_create_once_and_stale_edits_cannot_overwrite() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let id = Uuid::new_v4();
    let command = create(id, note("original"));
    let mut requests = Vec::new();
    for _ in 0..16 {
        let store = store.clone();
        let command = command.clone();
        requests.push(tokio::spawn(async move { store.apply(command).await }));
    }
    for task in requests {
        assert_eq!(task.await??.record.revision, 1);
    }
    assert_eq!(store.records().await?.len(), 1);
    let mut edits = Vec::new();
    for n in 0..16 {
        let store = store.clone();
        edits.push(tokio::spawn(async move {
            store
                .apply(replace(id, 1, note(&format!("edit {n}"))))
                .await
        }));
    }
    let mut success = 0;
    let mut conflicts = 0;
    for edit in edits {
        match edit.await? {
            Ok(receipt) => {
                assert_eq!(receipt.record.revision, 2);
                success += 1;
            }
            Err(Error::RevisionConflict {
                expected: 1,
                actual: 2,
            }) => conflicts += 1,
            other => return Err(format!("unexpected edit result: {other:?}").into()),
        }
    }
    assert_eq!((success, conflicts), (1, 15));
    Ok(())
}

#[tokio::test]
async fn membership_moves_and_tombstones_preserve_referential_integrity() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let list_a = Uuid::new_v4();
    let list_b = Uuid::new_v4();
    let item = Uuid::new_v4();
    let item_body = |list_id| RecordBody::ListItem {
        list_id,
        text: "pineapple".into(),
        completed: false,
    };
    assert!(matches!(
        store.apply(create(item, item_body(list_a))).await,
        Err(Error::NotFound(_))
    ));
    store
        .apply(create(
            list_a,
            RecordBody::List {
                name: "groceries".into(),
            },
        ))
        .await?;
    store
        .apply(create(
            list_b,
            RecordBody::List {
                name: "shopping".into(),
            },
        ))
        .await?;
    store.apply(create(item, item_body(list_a))).await?;
    assert!(matches!(
        store.apply(delete(list_a, 1)).await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        store.apply(replace(item, 1, note("not an item"))).await,
        Err(Error::Invalid(_))
    ));
    store.apply(replace(item, 1, item_body(list_b))).await?;
    store.apply(delete(list_a, 1)).await?;
    assert!(matches!(
        store.apply(delete(list_b, 1)).await,
        Err(Error::Invalid(_))
    ));
    store.apply(delete(item, 2)).await?;
    store.apply(delete(list_b, 1)).await?;
    assert!(store.records().await?.iter().all(|record| record.deleted));
    assert!(matches!(
        store.apply(create(item, item_body(list_b))).await,
        Err(Error::AlreadyExists(_))
    ));
    assert_eq!(
        store.get(item).await?.ok_or("missing tombstone")?.revision,
        3
    );
    assert_eq!(store.pending_projections(10).await?.len(), 3);
    Ok(())
}

#[tokio::test]
async fn stale_projection_ack_cannot_erase_a_later_edit_and_delete_survives_reopen() -> TestResult {
    let directory = tempfile::tempdir()?;
    let id = Uuid::new_v4();
    let deletion;
    {
        let store = SledRepository::open(directory.path()).await?;
        store.apply(create(id, note("v1"))).await?;
        let old = store
            .pending_projections(1)
            .await?
            .pop()
            .ok_or("missing first job")?;
        store.apply(replace(id, 1, note("v2"))).await?;
        assert!(!store.ack_projection(old.clone()).await?);
        let current = store
            .pending_projections(1)
            .await?
            .pop()
            .ok_or("missing second job")?;
        assert_eq!(current.revision, 2);
        assert!(store.ack_projection(current.clone()).await?);
        assert!(!store.ack_projection(current).await?);
        assert!(store.pending_projections(1).await?.is_empty());
        deletion = delete(id, 2);
        store.apply(deletion.clone()).await?;
        assert!(!store.ack_projection(old).await?);
        store.close().await?;
    }
    let store = SledRepository::open(directory.path()).await?;
    let job = store
        .pending_projections(1)
        .await?
        .pop()
        .ok_or("missing deletion")?;
    assert_eq!(job.revision, 3);
    assert!(store.apply(deletion).await?.record.deleted);
    assert!(store.ack_projection(job).await?);
    store.close().await?;
    assert!(SledRepository::open(directory.path())
        .await?
        .pending_projections(1)
        .await?
        .is_empty());
    Ok(())
}

#[tokio::test]
async fn validation_failures_do_not_write_a_record_or_job_or_consume_request_id() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let id = Uuid::new_v4();
    let mut command = create(id, note(" \n"));
    assert!(matches!(
        store.apply(command.clone()).await,
        Err(Error::Invalid(_))
    ));
    assert!(store.records().await?.is_empty());
    assert!(store.pending_projections(1).await?.is_empty());
    command.command = Command::Create {
        id,
        body: note("valid"),
    };
    store.apply(command).await?;
    assert!(matches!(
        store.pending_projections(0).await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        store.pending_projections(1_001).await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        store.apply(replace(id, 0, note("invalid revision"))).await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(store.get(id).await?.ok_or("missing note")?.revision, 1);
    Ok(())
}

#[tokio::test]
async fn unknown_schema_and_legacy_databases_are_rejected() -> TestResult {
    let unknown = tempfile::tempdir()?;
    {
        let db = sled::open(unknown.path())?;
        db.insert(b"meta/schema", &99_u32.to_be_bytes())?;
        db.flush()?;
    }
    assert!(matches!(
        SledRepository::open(unknown.path()).await,
        Err(Error::Schema(_))
    ));
    let legacy = tempfile::tempdir()?;
    {
        let db = sled::open(legacy.path())?;
        db.open_tree("todos")?.insert(b"id", b"original")?;
        db.flush()?;
    }
    assert!(matches!(
        SledRepository::open(legacy.path()).await,
        Err(Error::Schema(_))
    ));
    {
        let db = sled::open(legacy.path())?;
        assert_eq!(
            db.open_tree("todos")?.get(b"id")?.as_deref(),
            Some(b"original".as_slice())
        );
        assert!(db.get(b"meta/schema")?.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn corrupt_keys_and_receipts_are_rejected_without_acknowledging_success() -> TestResult {
    let directory = tempfile::tempdir()?;
    let id = Uuid::new_v4();
    let command = create(id, note("original"));
    let receipt = {
        let store = SledRepository::open(directory.path()).await?;
        let receipt = store.apply(command.clone()).await?;
        store.close().await?;
        receipt
    };
    {
        let db = sled::open(directory.path())?;
        let mut invalid = receipt.clone();
        invalid.record.id = Uuid::new_v4();
        db.insert(
            format!("request/{}", command.request_id),
            serde_json::to_vec(&invalid)?,
        )?;
        db.insert(
            format!("record/{}", Uuid::new_v4()),
            serde_json::to_vec(&receipt.record)?,
        )?;
        db.flush()?;
    }
    let store = SledRepository::open(directory.path()).await?;
    assert!(matches!(store.apply(command).await, Err(Error::Storage(_))));
    assert!(matches!(store.records().await, Err(Error::Storage(_))));
    assert_eq!(
        store.get(id).await?.ok_or("missing original")?,
        receipt.record
    );
    assert_eq!(store.pending_projections(1).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn invalid_parent_edits_and_task_completion_preserve_current_state() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let list = Uuid::new_v4();
    let item = Uuid::new_v4();
    let task = Uuid::new_v4();
    let body = RecordBody::ListItem {
        list_id: list,
        text: "pineapple".into(),
        completed: false,
    };
    store
        .apply(create(
            list,
            RecordBody::List {
                name: "groceries".into(),
            },
        ))
        .await?;
    store.apply(create(item, body.clone())).await?;
    store
        .apply(create(
            task,
            RecordBody::Task {
                title: "buy groceries".into(),
                completed: false,
                due_at: Some(chrono::Utc::now()),
            },
        ))
        .await?;
    let wrong_parent = RecordBody::ListItem {
        list_id: task,
        text: "pineapple".into(),
        completed: false,
    };
    assert!(matches!(
        store.apply(replace(item, 1, wrong_parent)).await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(store.get(item).await?.ok_or("missing item")?.body, body);
    assert!(matches!(
        store.apply(delete(list, 1)).await,
        Err(Error::Invalid(_))
    ));
    let completed = RecordBody::Task {
        title: "buy groceries".into(),
        completed: true,
        due_at: None,
    };
    let result = store.apply(replace(task, 1, completed.clone())).await?;
    assert_eq!(result.record.body, completed);
    assert_eq!(result.record.revision, 2);
    assert!(result.record.updated_at >= result.record.created_at);
    Ok(())
}

#[tokio::test]
async fn orderly_close_requires_last_handle_and_allows_immediate_reopen() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let other = store.clone();
    assert!(matches!(other.close().await, Err(Error::Invalid(_))));
    let id = Uuid::new_v4();
    store
        .apply(create(id, note("still open after refusing close")))
        .await?;
    store.close().await?;
    let reopened = SledRepository::open(directory.path()).await?;
    assert!(reopened.get(id).await?.is_some());
    reopened.close().await?;
    Ok(())
}
