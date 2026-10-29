use agentix_assistant::Assistant;
use agentix_assistant_core::{
    Command, CommandReceipt, CommandRequest, Conversations, Error, Inference, InferenceContext,
    Intent, ProjectionJob, Record, RecordBody, Repository, Result, TellRequest, TellStatus,
    TurnState,
};
use agentix_assistant_sled::SledRepository;
use async_trait::async_trait;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use uuid::Uuid;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
struct Script {
    intents: Mutex<VecDeque<Intent>>,
    contexts: Mutex<Vec<InferenceContext>>,
}
impl Script {
    fn new(intents: Vec<Intent>) -> Self {
        Self {
            intents: Mutex::new(intents.into()),
            contexts: Mutex::new(vec![]),
        }
    }
}
#[async_trait]
impl Inference for Script {
    async fn propose(&self, context: InferenceContext) -> Result<Intent> {
        self.contexts
            .lock()
            .map_err(|_| Error::Inference)?
            .push(context);
        self.intents
            .lock()
            .map_err(|_| Error::Inference)?
            .pop_front()
            .ok_or(Error::Inference)
    }
}
fn request(conversation_id: Uuid, text: &str) -> TellRequest {
    TellRequest {
        request_id: Uuid::new_v4(),
        conversation_id,
        text: text.into(),
        timezone: "America/New_York".into(),
    }
}
fn add() -> Intent {
    Intent::AddListItem {
        list_name: "groceries".into(),
        list_id: None,
        text: "pineapple".into(),
    }
}
fn service(store: &SledRepository, inference: Arc<dyn Inference>) -> Arc<Assistant> {
    let shared = Arc::new(store.clone());
    Arc::new(Assistant::new(shared.clone(), shared, inference))
}

#[tokio::test]
async fn grocery_save_read_correction_history_and_replay_use_actual_records() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let script = Arc::new(Script::new(vec![
        add(),
        Intent::ShowList {
            list_name: "groceries".into(),
            list_id: None,
        },
    ]));
    let assistant = service(&store, script.clone());
    let first = request(Uuid::new_v4(), "Add pineapple to my grocery list.");
    let saved = assistant.tell(first.clone()).await?;
    assert_eq!(saved.status, TellStatus::Saved);
    assert_eq!(saved.outcomes.len(), 2);
    assert_eq!(assistant.tell(first.clone()).await?, saved);
    let item = &saved.outcomes[1].record;
    let shown = assistant
        .tell(request(
            first.conversation_id,
            "What is on my grocery list?",
        ))
        .await?;
    assert_eq!(shown.status, TellStatus::Answer);
    assert_eq!(shown.records.len(), 2);
    assert!(shown.reply.contains("pineapple"));
    script
        .intents
        .lock()
        .map_err(|_| "poisoned script")?
        .push_back(Intent::EditListItem {
            id: item.id,
            expected_revision: 1,
            text: "mango".into(),
        });
    let corrected = assistant
        .tell(request(first.conversation_id, "No, make that mango."))
        .await?;
    assert_eq!(corrected.status, TellStatus::Saved);
    assert_eq!(corrected.outcomes[0].record.revision, 2);
    assert_eq!(store.records().await?.len(), 2);
    {
        let contexts = script.contexts.lock().map_err(|_| "poisoned script")?;
        assert_eq!(contexts.len(), 3);
        assert_eq!(contexts[1].messages.len(), 3);
        assert_eq!(contexts[2].messages.len(), 5);
        assert!(contexts[2]
            .records
            .iter()
            .any(|record| record.id == item.id));
        assert_eq!(contexts[0].request.timezone, "America/New_York");
        assert!(contexts[2]
            .messages
            .iter()
            .any(|message| message.text == saved.reply));
    }
    assert_eq!(
        store
            .recent_messages(first.conversation_id, 100)
            .await?
            .len(),
        6
    );
    Ok(())
}

struct Uncertain {
    store: SledRepository,
    fail_once: AtomicBool,
}
#[async_trait]
impl Repository for Uncertain {
    async fn apply(&self, command: CommandRequest) -> Result<CommandReceipt> {
        let receipt = self.store.apply(command).await?;
        if self.fail_once.swap(false, Ordering::SeqCst) {
            return Err(Error::Storage(
                "synthetic uncertain response after actual commit".into(),
            ));
        }
        Ok(receipt)
    }
    async fn get(&self, id: Uuid) -> Result<Option<Record>> {
        self.store.get(id).await
    }
    async fn records(&self) -> Result<Vec<Record>> {
        self.store.records().await
    }
    async fn pending_projections(&self, limit: usize) -> Result<Vec<ProjectionJob>> {
        self.store.pending_projections(limit).await
    }
    async fn ack_projection(&self, job: ProjectionJob) -> Result<bool> {
        self.store.ack_projection(job).await
    }
}

#[tokio::test]
async fn uncertain_save_reuses_prepared_commands_without_calling_model_again() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let script = Arc::new(Script::new(vec![add()]));
    let assistant = Arc::new(Assistant::new(
        Arc::new(Uncertain {
            store: store.clone(),
            fail_once: AtomicBool::new(true),
        }),
        Arc::new(store.clone()),
        script.clone(),
    ));
    let request = request(Uuid::new_v4(), "Add pineapple.");
    assert!(matches!(
        assistant.tell(request.clone()).await,
        Err(Error::Storage(_))
    ));
    let plan = match store.begin(request.clone()).await?.state {
        TurnState::Prepared { plan } => plan,
        _ => return Err("missing prepared plan".into()),
    };
    assert_eq!(store.records().await?.len(), 1);
    let result = assistant.tell(request.clone()).await?;
    assert_eq!(result.status, TellStatus::Saved);
    assert_eq!(result.outcomes[0].request_id, plan.commands[0].request_id);
    assert_eq!(result.outcomes[1].request_id, plan.commands[1].request_id);
    assert_eq!(store.records().await?.len(), 2);
    assert_eq!(
        script.contexts.lock().map_err(|_| "poisoned script")?.len(),
        1
    );
    assert_eq!(
        store
            .recent_messages(request.conversation_id, 100)
            .await?
            .len(),
        2
    );
    Ok(())
}

struct Paused {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait]
impl Inference for Paused {
    async fn propose(&self, _: InferenceContext) -> Result<Intent> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(add())
    }
}

#[tokio::test]
async fn cancelled_waiter_does_not_release_admitted_turn_or_claim_rollback() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let paused = Arc::new(Paused {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let assistant = service(&store, paused.clone());
    let original = request(Uuid::new_v4(), "Add pineapple.");
    let waiter = {
        let assistant = assistant.clone();
        let original = original.clone();
        tokio::spawn(async move { assistant.tell(original).await })
    };
    paused.entered.notified().await;
    assert!(matches!(
        assistant
            .tell(request(Uuid::new_v4(), "another instruction"))
            .await,
        Err(Error::Busy)
    ));
    waiter.abort();
    assert!(waiter.await.is_err_and(|error| error.is_cancelled()));
    paused.release.notify_one();
    let response = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match assistant.tell(original.clone()).await {
                Err(Error::Busy) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
                other => break other,
            }
        }
    })
    .await??;
    assert_eq!(response.status, TellStatus::Saved);
    assert_eq!(store.records().await?.len(), 2);
    assert_eq!(
        store
            .recent_messages(original.conversation_id, 100)
            .await?
            .len(),
        2
    );
    Ok(())
}

#[tokio::test]
async fn ambiguous_list_names_clarify_then_an_explicit_id_selects_the_original() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let mut lists = Vec::new();
    for _ in 0..2 {
        let id = Uuid::new_v4();
        store
            .apply(CommandRequest {
                request_id: Uuid::new_v4(),
                command: Command::Create {
                    id,
                    body: RecordBody::List {
                        name: "groceries".into(),
                    },
                },
            })
            .await?;
        lists.push(id);
    }
    let script = Arc::new(Script::new(vec![
        add(),
        Intent::AddListItem {
            list_name: "groceries".into(),
            list_id: Some(lists[1]),
            text: "pineapple".into(),
        },
    ]));
    let assistant = service(&store, script);
    let conversation = Uuid::new_v4();
    let clarified = assistant
        .tell(request(conversation, "Add pineapple."))
        .await?;
    assert_eq!(clarified.status, TellStatus::Clarification);
    assert!(clarified.outcomes.is_empty());
    assert_eq!(store.records().await?.len(), 2);
    let saved = assistant
        .tell(request(conversation, &lists[1].to_string()))
        .await?;
    assert_eq!(saved.outcomes.len(), 1);
    assert_eq!(saved.outcomes[0].record.body.list_id(), Some(lists[1]));
    Ok(())
}

#[tokio::test]
async fn inference_failure_and_timeout_are_durable_honest_non_mutations() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let empty = Arc::new(Script::new(vec![]));
    let assistant = service(&store, empty.clone());
    let request = request(Uuid::new_v4(), "Do something unsupported.");
    let failed = assistant.tell(request.clone()).await?;
    assert_eq!(failed.status, TellStatus::Failed);
    assert_eq!(assistant.tell(request).await?, failed);
    assert!(store.records().await?.is_empty());
    assert_eq!(
        empty.contexts.lock().map_err(|_| "poisoned script")?.len(),
        1
    );
    let paused = Arc::new(Paused {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let shared = Arc::new(store.clone());
    let assistant = Arc::new(
        Assistant::new(shared.clone(), shared, paused)
            .with_inference_timeout(std::time::Duration::from_millis(10))?,
    );
    let timed_out = assistant.tell(super_request()).await?;
    assert_eq!(timed_out.status, TellStatus::Failed);
    assert!(timed_out.outcomes.is_empty());
    assert!(store.records().await?.is_empty());
    Ok(())
}
fn super_request() -> TellRequest {
    request(Uuid::new_v4(), "Add pineapple.")
}

#[tokio::test]
async fn a_model_no_op_cannot_increment_a_revision_or_claim_an_update() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let id = Uuid::new_v4();
    let list = Uuid::new_v4();
    store
        .apply(CommandRequest {
            request_id: Uuid::new_v4(),
            command: Command::Create {
                id: list,
                body: RecordBody::List {
                    name: "groceries".into(),
                },
            },
        })
        .await?;
    let body = RecordBody::ListItem {
        list_id: list,
        text: "pineapple".into(),
        completed: true,
    };
    store
        .apply(CommandRequest {
            request_id: Uuid::new_v4(),
            command: Command::Create {
                id,
                body: body.clone(),
            },
        })
        .await?;
    let script = Arc::new(Script::new(vec![Intent::EditListItem {
        id,
        expected_revision: 1,
        text: "pineapple".into(),
    }]));
    let assistant = service(&store, script);
    let answer = assistant.tell(super_request()).await?;
    assert_eq!(answer.status, TellStatus::Clarification);
    assert!(answer.outcomes.is_empty());
    assert_eq!(store.get(id).await?.ok_or("missing list")?.revision, 1);
    Ok(())
}

#[tokio::test]
async fn focused_edits_preserve_fields_and_reject_wrong_types_and_stale_revisions() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let list = Uuid::new_v4();
    let item = Uuid::new_v4();
    let task = Uuid::new_v4();
    let due_at = chrono::DateTime::parse_from_rfc3339("2026-11-03T14:30:00Z")?.to_utc();
    for (id, body) in [
        (
            list,
            RecordBody::List {
                name: "groceries".into(),
            },
        ),
        (
            item,
            RecordBody::ListItem {
                list_id: list,
                text: "pineapple".into(),
                completed: true,
            },
        ),
        (
            task,
            RecordBody::Task {
                title: "Replace air filter".into(),
                due_at: Some(due_at),
                completed: false,
            },
        ),
    ] {
        store
            .apply(CommandRequest {
                request_id: Uuid::new_v4(),
                command: Command::Create { id, body },
            })
            .await?;
    }
    let script = Arc::new(Script::new(vec![
        Intent::EditListItem {
            id: list,
            expected_revision: 1,
            text: "mango".into(),
        },
        Intent::SetTaskCompleted {
            id: item,
            expected_revision: 1,
            completed: false,
        },
        Intent::EditListItem {
            id: item,
            expected_revision: 7,
            text: "mango".into(),
        },
        Intent::EditListItem {
            id: item,
            expected_revision: 1,
            text: "mango".into(),
        },
        Intent::SetTaskCompleted {
            id: task,
            expected_revision: 7,
            completed: true,
        },
        Intent::SetTaskCompleted {
            id: task,
            expected_revision: 1,
            completed: true,
        },
        Intent::SetTaskCompleted {
            id: task,
            expected_revision: 2,
            completed: true,
        },
    ]));
    let assistant = service(&store, script);
    let conversation = Uuid::new_v4();
    for status in [
        TellStatus::Clarification,
        TellStatus::Clarification,
        TellStatus::Failed,
    ] {
        let result = assistant
            .tell(request(conversation, "Change the item."))
            .await?;
        assert_eq!(result.status, status);
        assert!(result.outcomes.is_empty());
        assert_eq!(store.get(item).await?.ok_or("missing item")?.revision, 1);
    }
    let edit = request(conversation, "Make that mango.");
    let edited = assistant.tell(edit.clone()).await?;
    assert_eq!(edited.status, TellStatus::Saved);
    assert_eq!(assistant.tell(edit).await?, edited);
    let actual_item = store.get(item).await?.ok_or("missing item")?;
    assert_eq!(actual_item.revision, 2);
    assert_eq!(
        actual_item.body,
        RecordBody::ListItem {
            list_id: list,
            text: "mango".into(),
            completed: true
        }
    );
    assert_eq!(store.get(list).await?.ok_or("missing list")?.revision, 1);
    let stale = assistant
        .tell(request(conversation, "Complete the task."))
        .await?;
    assert_eq!(stale.status, TellStatus::Failed);
    assert!(stale.outcomes.is_empty());
    let completed = assistant
        .tell(request(conversation, "Complete the task."))
        .await?;
    assert_eq!(completed.status, TellStatus::Saved);
    let actual_task = store.get(task).await?.ok_or("missing task")?;
    assert_eq!(actual_task.revision, 2);
    assert_eq!(
        actual_task.body,
        RecordBody::Task {
            title: "Replace air filter".into(),
            due_at: Some(due_at),
            completed: true
        }
    );
    let unchanged = assistant
        .tell(request(conversation, "Complete the task."))
        .await?;
    assert_eq!(unchanged.status, TellStatus::Clarification);
    assert!(unchanged.outcomes.is_empty());
    assert_eq!(store.get(task).await?.ok_or("missing task")?.revision, 2);
    Ok(())
}

struct RejectSecond {
    store: SledRepository,
    calls: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl Repository for RejectSecond {
    async fn apply(&self, command: CommandRequest) -> Result<CommandReceipt> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
            return Err(Error::RevisionConflict {
                expected: 1,
                actual: 2,
            });
        }
        self.store.apply(command).await
    }
    async fn get(&self, id: Uuid) -> Result<Option<Record>> {
        self.store.get(id).await
    }
    async fn records(&self) -> Result<Vec<Record>> {
        self.store.records().await
    }
    async fn pending_projections(&self, limit: usize) -> Result<Vec<ProjectionJob>> {
        self.store.pending_projections(limit).await
    }
    async fn ack_projection(&self, job: ProjectionJob) -> Result<bool> {
        self.store.ack_projection(job).await
    }
}

#[tokio::test]
async fn multi_command_conflict_reports_the_actual_partial_save_and_replays_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let script = Arc::new(Script::new(vec![add()]));
    let assistant = Arc::new(Assistant::new(
        Arc::new(RejectSecond {
            store: store.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
        Arc::new(store.clone()),
        script,
    ));
    let request = super_request();
    let partial = assistant.tell(request.clone()).await?;
    assert_eq!(partial.status, TellStatus::Partial);
    assert_eq!(partial.outcomes.len(), 1);
    assert_eq!(store.records().await?.len(), 1);
    assert!(matches!(
        partial.outcomes[0].record.body,
        RecordBody::List { .. }
    ));
    assert_eq!(assistant.tell(request).await?, partial);
    assert_eq!(store.records().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn oversized_originals_are_omitted_explicitly_and_latest_instruction_reaches_inference(
) -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    store
        .apply(CommandRequest {
            request_id: Uuid::new_v4(),
            command: Command::Create {
                id: Uuid::new_v4(),
                body: RecordBody::Note {
                    text: "x".repeat(60_000),
                },
            },
        })
        .await?;
    let script = Arc::new(Script::new(vec![Intent::Clarify {
        question: "Which note?".into(),
    }]));
    let assistant = service(&store, script.clone());
    let request = super_request();
    assistant.tell(request.clone()).await?;
    let contexts = script.contexts.lock().map_err(|_| "poisoned script")?;
    assert!(contexts[0].context_incomplete);
    assert!(contexts[0].records.is_empty());
    assert_eq!(
        contexts[0]
            .messages
            .last()
            .ok_or("missing latest instruction")?
            .text,
        request.text
    );
    Ok(())
}
