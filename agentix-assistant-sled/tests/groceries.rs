use agentix_assistant_core::{
    Conversations, Error, GroceryMutation, GroceryOutcome, GroceryPlan, Repository, TellRequest,
    TellStatus, TurnState,
};
use agentix_assistant_sled::SledRepository;
use uuid::Uuid;
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn request() -> TellRequest {
    TellRequest {
        request_id: Uuid::new_v4(),
        conversation_id: Uuid::new_v4(),
        text: "add milk".into(),
        timezone: "UTC".into(),
    }
}
fn plan() -> GroceryPlan {
    GroceryPlan {
        team_id: "household-1".into(),
        list_id: "remote-1".into(),
        list_name: "Kitchen".into(),
        mutation: GroceryMutation::Add {
            text: "milk".into(),
        },
    }
}

#[tokio::test]
async fn prepared_dispatch_and_outcome_survive_reopen_without_local_records() -> TestResult {
    let directory = tempfile::tempdir()?;
    let request = request();
    let store = SledRepository::open(directory.path()).await?;
    store.begin(request.clone()).await?;
    assert!(store
        .claim_grocery_dispatch(request.request_id)
        .await
        .is_err());
    store.prepare_grocery(request.request_id, plan()).await?;
    assert!(store
        .complete_grocery(request.request_id, GroceryOutcome::Confirmed)
        .await
        .is_err());
    store.close().await?;
    let store = SledRepository::open(directory.path()).await?;
    assert!(matches!(
        store.begin(request.clone()).await?.state,
        TurnState::GroceryPrepared { .. }
    ));
    let (first, second) = tokio::join!(
        store.claim_grocery_dispatch(request.request_id),
        store.claim_grocery_dispatch(request.request_id)
    );
    assert_ne!(first?, second?);
    store.close().await?;
    let store = SledRepository::open(directory.path()).await?;
    assert!(!store.claim_grocery_dispatch(request.request_id).await?);
    let response = store
        .complete_grocery(request.request_id, GroceryOutcome::Uncertain)
        .await?;
    assert_eq!(response.status, TellStatus::Answer);
    assert!(response.outcomes.is_empty() && response.records.is_empty());
    assert!(response.reply.contains("did not confirm"));
    assert_eq!(
        store
            .complete_grocery(request.request_id, GroceryOutcome::Uncertain)
            .await?,
        response
    );
    assert!(matches!(
        store
            .complete_grocery(request.request_id, GroceryOutcome::Confirmed)
            .await,
        Err(Error::RequestConflict)
    ));
    assert!(store.records().await?.is_empty());
    store.close().await?;
    let store = SledRepository::open(directory.path()).await?;
    assert!(
        matches!(store.begin(request.clone()).await?.state, TurnState::GroceryCompleted {response:stored,..} if stored == response)
    );
    assert_eq!(
        store
            .recent_messages(request.conversation_id, 10)
            .await?
            .len(),
        2
    );
    let mut next = request.clone();
    next.request_id = Uuid::new_v4();
    store.begin(next).await?;
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn prepared_identity_cannot_be_replaced_or_completed_via_local_receipts() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let request = request();
    store.begin(request.clone()).await?;
    store.prepare_grocery(request.request_id, plan()).await?;
    let mut other = plan();
    other.list_id = "another-list".into();
    assert!(matches!(
        store.prepare_grocery(request.request_id, other).await,
        Err(Error::RequestConflict)
    ));
    assert!(store
        .complete(
            request.request_id,
            plan().response(&request, GroceryOutcome::Confirmed)
        )
        .await
        .is_err());
    assert!(store.claim_grocery_dispatch(request.request_id).await?);
    let response = store
        .complete_grocery(request.request_id, GroceryOutcome::Confirmed)
        .await?;
    response.validate(&request)?;
    assert!(response.reply.starts_with("OurGroceries:"));
    store.close().await?;
    Ok(())
}
