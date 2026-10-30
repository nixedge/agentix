mod groceries;
mod planner;
mod reminders;
mod schema;

use agentix_assistant_core::{
    search_records, Conversations, Error, Inference, InferenceContext, PreparedPlan, Projection,
    Record, RecordKind, Repository, Result, SearchRequest, TellRequest, TellResponse, TellStatus,
    TurnState,
};
use chrono::{DateTime, Utc};
pub use schema::decision_schema;
use std::{cmp::Reverse, collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub struct Assistant {
    repository: Arc<dyn Repository>,
    conversations: Arc<dyn Conversations>,
    inference: Arc<dyn Inference>,
    projection: Option<Arc<dyn Projection>>,
    gate: Arc<Mutex<()>>,
    inference_timeout: Duration,
    groceries: Option<Arc<dyn agentix_assistant_core::Groceries>>,
    reminders: Option<(
        Arc<dyn agentix_assistant_core::Reminders>,
        Arc<dyn agentix_assistant_core::ReminderInference>,
    )>,
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
            groceries: None,
            reminders: None,
        }
    }

    pub fn with_projection(mut self, projection: Arc<dyn Projection>) -> Self {
        self.projection = Some(projection);
        self
    }

    pub fn with_groceries(mut self, groceries: Arc<dyn agentix_assistant_core::Groceries>) -> Self {
        self.groceries = Some(groceries);
        self
    }

    async fn execute_grocery(
        &self,
        request_id: uuid::Uuid,
        plan: agentix_assistant_core::GroceryPlan,
    ) -> Result<TellResponse> {
        let outcome = if self
            .conversations
            .claim_grocery_dispatch(request_id)
            .await?
        {
            if let Some(provider) = &self.groceries {
                tokio::time::timeout(Duration::from_secs(60), provider.execute(&plan))
                    .await
                    .unwrap_or(agentix_assistant_core::GroceryOutcome::Uncertain)
            } else {
                agentix_assistant_core::GroceryOutcome::Rejected
            }
        } else {
            agentix_assistant_core::GroceryOutcome::Uncertain
        };
        self.conversations
            .complete_grocery(request_id, outcome)
            .await
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
            if let Some((store, _)) = &service.reminders {
                if store.device_bound(request.request_id).await? {
                    return Err(Error::RequestConflict);
                }
            }
            service.run(request).await
        })
        .await
        .map_err(|_| Error::Storage("admitted turn interrupted; retry original request".into()))?
    }

    async fn run(&self, request: TellRequest) -> Result<TellResponse> {
        let turn = self.conversations.begin(request.clone()).await?;
        let plan = match turn.state {
            TurnState::Completed { response } | TurnState::GroceryCompleted { response, .. } => {
                return Ok(response)
            }
            TurnState::GroceryPrepared { plan } => {
                return self.execute_grocery(request.request_id, plan).await
            }
            TurnState::GroceryDispatched { .. } => {
                return self
                    .conversations
                    .complete_grocery(
                        request.request_id,
                        agentix_assistant_core::GroceryOutcome::Uncertain,
                    )
                    .await
            }
            TurnState::Prepared { plan } => plan,
            TurnState::Received => {
                let originals = self.repository.records().await?;
                let remote = if let Some(provider) = &self.groceries {
                    match tokio::time::timeout(Duration::from_secs(25), provider.snapshot()).await {
                        Ok(result) => result.ok(),
                        Err(_) => {
                            tracing::warn!(
                                stage = "snapshot",
                                reason = "timeout",
                                "OurGroceries request failed"
                            );
                            None
                        }
                    }
                } else {
                    None
                };
                let mut context = self.context(&request, turn.accepted_at, &originals).await?;
                context.groceries_configured = self.groceries.is_some();
                context.groceries = remote.as_ref().map(groceries::context);
                context.context_incomplete |= self.groceries.is_some()
                    && remote.as_ref().is_none_or(|list| {
                        context
                            .groceries
                            .as_ref()
                            .is_none_or(|shown| list.items.len() != shown.items.len())
                    });
                let proposed =
                    tokio::time::timeout(self.inference_timeout, self.inference.propose(context))
                        .await;
                let plan = match proposed {
                    Ok(Ok(intent)) => match groceries::prepare(&intent, &originals, remote.as_ref(), self.groceries.is_some(), &request.text) {
                        Ok(Some(groceries::Decision::Change(plan))) => {
                            self.conversations.prepare_grocery(request.request_id, plan.clone()).await?;
                            return self.execute_grocery(request.request_id, plan).await;
                        }
                        Ok(Some(groceries::Decision::Read(plan))) => plan,
                        Ok(None) => planner::prepare(intent, &originals).unwrap_or_else(|_| failure_plan("I could not safely apply that instruction. Nothing was changed.")),
                        Err(_) => failure_plan("I could not safely identify that OurGroceries item. Nothing was changed. Read the list and identify the item again."),
                    },
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
            .filter(|record| {
                !record.deleted
                    && (self.groceries.is_none() || !groceries::local_grocery(record, originals))
            })
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
            groceries_configured: false,
            groceries: None,
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
