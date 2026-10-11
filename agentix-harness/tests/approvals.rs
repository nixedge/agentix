use agentix_api::{
    agent::{ApprovalDecision, ApprovalResponse, InvocationContext},
    wire::Text,
};
use agentix_harness::{
    approval::{scoped_request, ApprovalBroker, Cancellation},
    AgentEvent,
};
use anyhow::{Context, Result};
use serde_json::json;
fn request() -> Result<agentix_api::agent::ApprovalRequest> {
    scoped_request(
        InvocationContext {
            session_id: uuid::Uuid::new_v4(),
            turn_id: uuid::Uuid::new_v4(),
            invocation_id: Text::new("call-one")?,
        },
        "write_file",
        std::path::Path::new("/project"),
        &json!({"path":"a","content":"new"}),
    )
}
#[tokio::test]
async fn approval_is_bound_to_action_and_consumed_once() -> Result<()> {
    let broker = ApprovalBroker::default();
    let cancel = Cancellation::default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let req = request()?;
    let pending = {
        let broker = broker.clone();
        let req = req.clone();
        tokio::spawn(async move { broker.request(req, &tx, &cancel).await })
    };
    assert!(matches!(
        rx.recv().await.context("event")?,
        AgentEvent::ApprovalRequired { .. }
    ));
    let response = ApprovalResponse {
        request_id: req.request_id,
        context: req.context,
        arguments_hash: req.arguments_hash,
        decision: ApprovalDecision::Allow,
    };
    let mut wrong = response.clone();
    wrong.context.turn_id = uuid::Uuid::new_v4();
    assert!(broker.respond(wrong).is_err());
    broker.respond(response.clone())?;
    assert_eq!(pending.await??, ApprovalDecision::Allow);
    assert!(broker.respond(response).is_err());
    Ok(())
}
#[tokio::test]
async fn expired_and_cancelled_approvals_cannot_execute() -> Result<()> {
    let broker = ApprovalBroker::default();
    let cancel = Cancellation::default();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut expired = request()?;
    expired.expires_at_unix_ms = 0;
    assert_eq!(
        broker.request(expired, &tx, &cancel).await?,
        ApprovalDecision::Deny
    );
    cancel.cancel();
    assert!(broker.request(request()?, &tx, &cancel).await.is_err());
    Ok(())
}
