use crate::event::AgentEvent;
use agentix_api::{
    agent::{ApprovalDecision, ApprovalRequest, ApprovalResponse, InvocationContext},
    wire::{Sha256, Text},
};
use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256 as Hasher};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc::UnboundedSender, oneshot, watch};

#[derive(Clone, Debug)]
pub struct Cancellation(watch::Sender<bool>);
impl Default for Cancellation {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}
impl Cancellation {
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }
    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
    pub async fn cancelled(&self) {
        let mut rx = self.0.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }
}

#[derive(Debug)]
pub struct ApprovalRequired;
impl std::fmt::Display for ApprovalRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("approval required; run interactively to review this action")
    }
}
impl std::error::Error for ApprovalRequired {}
#[derive(Debug)]
pub struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("turn cancelled")
    }
}
impl std::error::Error for Cancelled {}

struct Pending {
    request: ApprovalRequest,
    tx: oneshot::Sender<ApprovalDecision>,
}
#[derive(Clone, Default)]
pub struct ApprovalBroker {
    pending: Arc<Mutex<HashMap<uuid::Uuid, Pending>>>,
}

impl ApprovalBroker {
    pub fn respond(&self, response: ApprovalResponse) -> Result<()> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow!("approval lock poisoned"))?;
        let expected = &pending
            .get(&response.request_id)
            .ok_or_else(|| anyhow!("stale approval"))?
            .request;
        if expected.context.session_id != response.context.session_id
            || expected.context.turn_id != response.context.turn_id
            || expected.context.invocation_id != response.context.invocation_id
            || expected.arguments_hash != response.arguments_hash
            || now_ms() > expected.expires_at_unix_ms
        {
            return Err(anyhow!("approval does not match this action"));
        }
        let item = pending
            .remove(&response.request_id)
            .ok_or_else(|| anyhow!("approval expired"))?;
        item.tx
            .send(response.decision)
            .map_err(|_| anyhow!("approval no longer pending"))
    }

    pub async fn request(
        &self,
        request: ApprovalRequest,
        events: &UnboundedSender<AgentEvent>,
        cancel: &Cancellation,
    ) -> Result<ApprovalDecision> {
        let id = request.request_id;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| anyhow!("approval lock poisoned"))?
            .insert(
                id,
                Pending {
                    request: request.clone(),
                    tx,
                },
            );
        let _cleanup = PendingGuard {
            broker: self.clone(),
            id,
        };
        events
            .send(AgentEvent::ApprovalRequired {
                request: request.clone(),
            })
            .map_err(|_| anyhow!("approval UI disconnected"))?;
        tokio::select! {
            _=cancel.cancelled()=>Err(Cancelled.into()),
            result=tokio::time::timeout(Duration::from_millis(request.expires_at_unix_ms.saturating_sub(now_ms())),rx)=>match result {
                Ok(Ok(decision))=>Ok(decision), _=>Ok(ApprovalDecision::Deny),
            }
        }
    }
}
struct PendingGuard {
    broker: ApprovalBroker,
    id: uuid::Uuid,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.broker.pending.lock() {
            pending.remove(&self.id);
        }
    }
}

pub fn scoped_request(
    context: InvocationContext,
    tool: &str,
    cwd: &Path,
    args: &serde_json::Value,
) -> Result<ApprovalRequest> {
    let encoded = serde_json::to_vec(
        &serde_json::json!({"tool":tool,"cwd":cwd,"arguments":args,"context":context}),
    )?;
    let hash = format!("{:x}", Hasher::digest(&encoded));
    Ok(ApprovalRequest {
        request_id: uuid::Uuid::new_v4(),
        context,
        tool: Text::new(tool)?,
        cwd: Text::new(cwd.to_string_lossy().into_owned())?,
        arguments_hash: Sha256::new(hash)?,
        effects: Text::new(serde_json::to_string(args)?)?,
        expires_at_unix_ms: now_ms().saturating_add(120_000),
    })
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPolicy {
    #[default]
    Require,
    Skip,
}
