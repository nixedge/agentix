mod planner;
mod schema;

use agentix_assistant_core::{
    search_records, Conversations, Error, Inference, InferenceContext, PreparedPlan, Projection,
    Record, RecordKind, Repository, Result, SearchRequest, TellRequest, TellResponse, TellStatus,
    TurnState,
};
use chrono::{DateTime, Utc};
pub use schema::{decision_schema, SYSTEM_PROMPT};
use std::{cmp::Reverse, collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub struct Assistant {
    repository: Arc<dyn Repository>,
    conversations: Arc<dyn Conversations>,
    inference: Arc<dyn Inference>,
    projection: Option<Arc<dyn Projection>>,
    gate: Arc<Mutex<()>>,
    inference_timeout: Duration,
}

impl Assistant {
    pub fn new(
        repository: Arc<dyn Repository>,
        conversations: Arc<dyn Conversations>,
        inference: Arc<dyn Inference>,
    ) -> Self {
        Self {
            repository,
            conversations,
            inference,
            projection: None,
            gate: Arc::new(Mutex::new(())),
            inference_timeout: Duration::from_secs(30),
        }
    }

    pub fn with_projection(mut self, projection: Arc<dyn Projection>) -> Self {
        self.projection = Some(projection);
        self
    }

    pub fn with_inference_timeout(mut self, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() || timeout > Duration::from_secs(120) {
            return Err(Error::Invalid(
                "inference timeout must be positive and at most 120 seconds".into(),
            ));
        }
        self.inference_timeout = timeout;
        Ok(self)
    }

    pub async fn tell(self: &Arc<Self>, request: TellRequest) -> Result<TellResponse> {
        request.validate()?;
        let guard = Arc::clone(&self.gate)
            .try_lock_owned()
            .map_err(|_| Error::Busy)?;
        let service = Arc::clone(self);
        // An admitted turn owns its gate through durable completion, even if HTTP is cancelled.
        tokio::spawn(async move {
            let _guard = guard;
            service.run(request).await
        })
        .await
        .map_err(|_| Error::Storage("admitted turn interrupted; retry original request".into()))?
    }

    async fn run(&self, request: TellRequest) -> Result<TellResponse> {
        let turn = self.conversations.begin(request.clone()).await?;
        let plan = match turn.state {
            TurnState::Completed { response } => return Ok(response),
            TurnState::Prepared { plan } => plan,
            TurnState::Received => {
                let originals = self.repository.records().await?;
                let context = self.context(&request, turn.accepted_at, &originals).await?;
                let proposed =
                    tokio::time::timeout(self.inference_timeout, self.inference.propose(context))
                        .await;
                let plan = match proposed {
                    Ok(Ok(intent)) => planner::prepare(intent, &originals).unwrap_or_else(|_| failure_plan("I could not safely apply that instruction. Nothing was changed.")),
                    _ => failure_plan("The local model could not interpret that instruction. Nothing was changed. Please try again."),
                };
                self.conversations
                    .prepare(request.request_id, plan.clone())
                    .await?;
                plan
            }
        };
        let mut response = TellResponse {
            request_id: request.request_id,
            conversation_id: request.conversation_id,
            status: plan.status,
            reply: plan.reply.clone(),
            outcomes: vec![],
            records: plan.records.clone(),
        };
        for command in &plan.commands {
            match self.repository.apply(command.clone()).await {
                Ok(receipt) => response.outcomes.push(receipt),
                Err(error @ (Error::Storage(_) | Error::Schema(_))) => return Err(error),
                Err(_) => {
                    response.status = if response.outcomes.is_empty() {
                        TellStatus::Failed
                    } else {
                        TellStatus::Partial
                    };
                    response.reply = if response.outcomes.is_empty() {
                        "The record changed or the instruction could not be applied. Nothing was saved.".into()
                    } else {
                        "Some changes were saved, but the remaining change could not be applied. Review the saved outcomes before trying another instruction.".into()
                    };
                    break;
                }
            }
        }
        self.conversations
            .complete(request.request_id, response)
            .await
    }

    async fn context(
        &self,
        request: &TellRequest,
        accepted_at: DateTime<Utc>,
        originals: &[Record],
    ) -> Result<InferenceContext> {
        let mut incomplete = false;
        let mut preferred = HashSet::new();
        if let Some(projection) = &self.projection {
            let query = SearchRequest {
                text: request.text.clone(),
                kind: None,
                limit: 10,
            };
            match tokio::time::timeout(
                Duration::from_secs(2),
                search_records(self.repository.as_ref(), projection.as_ref(), query),
            )
            .await
            {
                Ok(Ok(found)) => {
                    incomplete |= found.projection_pending || found.discarded_candidates > 0;
                    preferred.extend(found.records.into_iter().map(|hit| hit.record.id));
                }
                _ => incomplete = true,
            }
        }
        let mut candidates = originals
            .iter()
            .filter(|record| !record.deleted)
            .collect::<Vec<_>>();
        candidates.sort_by_key(|record| {
            (
                !preferred.contains(&record.id),
                match record.body.kind() {
                    RecordKind::List => 0,
                    RecordKind::ListItem => 1,
                    RecordKind::Task => 2,
                    RecordKind::Note => 3,
                },
                Reverse(record.updated_at),
            )
        });
        let mut records = Vec::new();
        let mut bytes = 0;
        for record in candidates {
            let size = serde_json::to_vec(record)
                .map_err(|_| Error::Storage("cannot encode context record".into()))?
                .len();
            if records.len() < 32 && bytes + size <= 12_288 {
                bytes += size;
                records.push(record.clone());
            } else {
                incomplete = true;
            }
        }
        let history = self
            .conversations
            .recent_messages(request.conversation_id, 24)
            .await?;
        let mut messages = Vec::new();
        let mut bytes = 0;
        for message in history.into_iter().rev() {
            if bytes + message.text.len() > 8_192 {
                incomplete = true;
                break;
            }
            bytes += message.text.len();
            messages.push(message);
        }
        messages.reverse();
        Ok(InferenceContext {
            request: request.clone(),
            accepted_at,
            messages,
            records,
            context_incomplete: incomplete,
        })
    }
}

fn failure_plan(reply: &str) -> PreparedPlan {
    PreparedPlan {
        commands: vec![],
        status: TellStatus::Failed,
        reply: reply.into(),
        records: vec![],
    }
}
