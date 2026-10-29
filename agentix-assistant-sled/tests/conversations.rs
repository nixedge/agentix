use agentix_assistant_core::{
    Command, CommandRequest, Conversations, Error, PreparedPlan, RecordBody, Repository,
    TellRequest, TellResponse, TellStatus, TurnState,
};
use agentix_assistant_sled::SledRepository;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn request(conversation_id: Uuid) -> TellRequest {
    TellRequest {
        request_id: Uuid::new_v4(),
        conversation_id,
        text: "add pineapple to groceries".into(),
        timezone: "America/New_York".into(),
    }
}
fn clarification() -> PreparedPlan {
    PreparedPlan {
        commands: vec![],
        status: TellStatus::Clarification,
        reply: "Which list?".into(),
        records: vec![],
    }
}
fn response(request: &TellRequest, plan: &PreparedPlan) -> TellResponse {
    TellResponse {
        request_id: request.request_id,
        conversation_id: request.conversation_id,
        status: plan.status,
        reply: plan.reply.clone(),
        outcomes: vec![],
        records: plan.records.clone(),
    }
}
fn note(text: &str) -> CommandRequest {
    CommandRequest {
        request_id: Uuid::new_v4(),
        command: Command::Create {
            id: Uuid::new_v4(),
            body: RecordBody::Note { text: text.into() },
        },
    }
}

#[tokio::test]
async fn request_replay_and_append_only_history_survive_reopen() -> TestResult {
    let directory = tempfile::tempdir()?;
    let first = request(Uuid::new_v4());
    let result;
    {
        let store = SledRepository::open(directory.path()).await?;
        let received = store.begin(first.clone()).await?;
        assert_eq!(store.begin(first.clone()).await?, received);
        let mut changed = first.clone();
        changed.timezone = "UTC".into();
        assert!(matches!(
            store.begin(changed).await,
            Err(Error::RequestConflict)
        ));
        assert!(matches!(
            store.begin(request(first.conversation_id)).await,
            Err(Error::Busy)
        ));
        let plan = clarification();
        store.prepare(first.request_id, plan.clone()).await?;
        result = response(&first, &plan);
        assert_eq!(
            store.complete(first.request_id, result.clone()).await?,
            result
        );
        assert_eq!(
            store.complete(first.request_id, result.clone()).await?,
            result
        );
        let second = request(first.conversation_id);
        store.begin(second.clone()).await?;
        store.prepare(second.request_id, plan.clone()).await?;
        store
            .complete(second.request_id, response(&second, &plan))
            .await?;
        let history = store.recent_messages(first.conversation_id, 100).await?;
        assert_eq!(
            history
                .iter()
                .map(|message| message.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(
            store.recent_messages(first.conversation_id, 2).await?,
            history[2..]
        );
        store.close().await?;
    }
    let store = SledRepository::open(directory.path()).await?;
    assert!(
        matches!(store.begin(first.clone()).await?.state, TurnState::Completed { response } if response == result)
    );
    assert_eq!(
        store
            .recent_messages(first.conversation_id, 100)
            .await?
            .len(),
        4
    );
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn prepared_commands_resume_after_a_partial_execution_without_regeneration() -> TestResult {
    let directory = tempfile::tempdir()?;
    let request = request(Uuid::new_v4());
    let plan = PreparedPlan {
        commands: vec![note("first"), note("second")],
        status: TellStatus::Saved,
        reply: "Saved both notes.".into(),
        records: vec![],
    };
    {
        let store = SledRepository::open(directory.path()).await?;
        store.begin(request.clone()).await?;
        let prepared = store.prepare(request.request_id, plan.clone()).await?;
        assert_eq!(
            store.prepare(request.request_id, plan.clone()).await?,
            prepared
        );
        assert!(matches!(
            store.prepare(request.request_id, clarification()).await,
            Err(Error::RequestConflict)
        ));
        store.apply(plan.commands[0].clone()).await?;
        store.close().await?;
    }
    let store = SledRepository::open(directory.path()).await?;
    assert!(
        matches!(store.begin(request.clone()).await?.state, TurnState::Prepared { plan: stored } if stored == plan)
    );
    let mut result = response(&request, &plan);
    for command in &plan.commands {
        result.outcomes.push(store.apply(command.clone()).await?);
    }
    store.complete(request.request_id, result).await?;
    assert_eq!(store.records().await?.len(), 2);
    assert_eq!(store.pending_projections(10).await?.len(), 2);
    assert_eq!(
        store
            .recent_messages(request.conversation_id, 100)
            .await?
            .len(),
        2
    );
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn completion_rejects_forged_or_omitted_receipts_and_preserves_partial_truth() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let request = request(Uuid::new_v4());
    let plan = PreparedPlan {
        commands: vec![note("first"), note("second")],
        status: TellStatus::Saved,
        reply: "Saved.".into(),
        records: vec![],
    };
    store.begin(request.clone()).await?;
    assert!(matches!(
        store
            .complete(request.request_id, response(&request, &clarification()))
            .await,
        Err(Error::Invalid(_))
    ));
    store.prepare(request.request_id, plan.clone()).await?;
    let receipt = store.apply(plan.commands[0].clone()).await?;
    let mut failed = response(&request, &plan);
    failed.status = TellStatus::Failed;
    assert!(matches!(
        store.complete(request.request_id, failed).await,
        Err(Error::Storage(_))
    ));
    let mut partial = response(&request, &plan);
    partial.status = TellStatus::Partial;
    partial.reply = "Saved the first note; the second was not saved.".into();
    partial.outcomes.push(receipt.clone());
    let mut forged = partial.clone();
    forged.outcomes[0].record.updated_at += chrono::Duration::seconds(1);
    assert!(matches!(
        store.complete(request.request_id, forged).await,
        Err(Error::Storage(_))
    ));
    store.complete(request.request_id, partial.clone()).await?;
    assert_eq!(
        store.complete(request.request_id, partial).await?.outcomes,
        vec![receipt]
    );
    assert_eq!(store.records().await?.len(), 1);
    store.close().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_turn_admission_cannot_append_two_unresolved_user_messages() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let conversation = Uuid::new_v4();
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            store.begin(request(conversation)).await
        }));
    }
    let mut accepted = 0;
    for task in tasks {
        match task.await? {
            Ok(_) => accepted += 1,
            Err(Error::Busy) => {}
            other => return Err(format!("unexpected admission {other:?}").into()),
        }
    }
    assert_eq!(accepted, 1);
    assert_eq!(store.recent_messages(conversation, 100).await?.len(), 1);
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn invalid_envelopes_and_history_bounds_do_not_consume_request_identity() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let mut request = request(Uuid::new_v4());
    request.timezone = "not a timezone".into();
    assert!(matches!(
        store.begin(request.clone()).await,
        Err(Error::Invalid(_))
    ));
    request.timezone = "UTC".into();
    store.begin(request.clone()).await?;
    assert!(store
        .recent_messages(request.conversation_id, 0)
        .await
        .is_err());
    assert!(store
        .recent_messages(request.conversation_id, 101)
        .await
        .is_err());
    assert_eq!(
        store
            .recent_messages(request.conversation_id, 1)
            .await?
            .len(),
        1
    );
    store.close().await?;
    Ok(())
}
