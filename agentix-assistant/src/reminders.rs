use super::*;
use agentix_assistant_core::*;

impl Assistant {
    pub fn with_reminders(
        mut self,
        store: Arc<dyn Reminders>,
        inference: Arc<dyn ReminderInference>,
    ) -> Self {
        self.reminders = Some((store, inference));
        self
    }
    pub fn reminder_store(&self) -> Result<Arc<dyn Reminders>> {
        self.reminders
            .as_ref()
            .map(|(store, _)| store.clone())
            .ok_or_else(|| Error::Invalid("Android reminders are unavailable".into()))
    }
    pub async fn tell_device(
        self: &Arc<Self>,
        request: DeviceTurnRequest,
    ) -> Result<DeviceTurnResponse> {
        request.validate()?;
        let guard = self
            .gate
            .clone()
            .try_lock_owned()
            .map_err(|_| Error::Busy)?;
        let service = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            service.run_device(request).await
        })
        .await
        .map_err(|_| Error::Storage("device turn interrupted; retry original identity".into()))?
    }
    async fn run_device(&self, request: DeviceTurnRequest) -> Result<DeviceTurnResponse> {
        let (store, inference) = self
            .reminders
            .as_ref()
            .ok_or_else(|| Error::Invalid("Android reminders unavailable".into()))?;
        let (turn, result) = store.begin_device_turn(request.clone()).await?;
        if let Some(result) = result {
            return Ok(result);
        }
        if turn.state != TurnState::Received {
            let ordinary = self.run(request.request.clone()).await?;
            return store
                .finish_device_turn(request, ReminderIntent::Other, Some(ordinary))
                .await;
        }
        let originals = self.repository.records().await?;
        let context = self
            .context(&request.request, turn.accepted_at, &originals)
            .await?;
        let snapshot = store.snapshot(request.device_id).await?;
        let posted: std::collections::HashSet<_> = snapshot
            .observations
            .iter()
            .filter(|o| {
                matches!(
                    o.state,
                    ReminderDeliveryState::Posted | ReminderDeliveryState::Dismissed
                )
            })
            .map(|o| o.reminder_id)
            .collect();
        let mut reminders = snapshot.reminders;
        reminders.sort_by_key(|r| (r.cancelled, posted.contains(&r.id), r.due_at));
        let mut bytes = 0;
        let mut selected = Vec::new();
        let total = reminders.len();
        for reminder in reminders {
            let size = serde_json::to_vec(&reminder)
                .map_err(|_| Error::Inference)?
                .len();
            if selected.len() < 32 && bytes + size <= 8192 {
                bytes += size;
                selected.push(reminder);
            }
        }
        let proposed = tokio::time::timeout(
            self.inference_timeout,
            inference.propose_reminder(ReminderContext {
                context,
                incomplete: selected.len() != total,
                reminders: selected,
            }),
        )
        .await;
        let intent=match proposed {
            Ok(Ok(intent))=>intent,
            _=>ReminderIntent::Clarify {question:"I could not safely interpret this request. Please try again with the reminder text and a clear date and time.".into()},
        };
        if intent == ReminderIntent::Other {
            let ordinary = self.run(request.request.clone()).await?;
            return store
                .finish_device_turn(request, intent, Some(ordinary))
                .await;
        }
        match store
            .finish_device_turn(request.clone(), intent, None)
            .await
        {
            Err(
                error @ (Error::Invalid(_) | Error::RevisionConflict { .. } | Error::NotFound(_)),
            ) => {
                let question = format!("Please clarify this reminder: {error}");
                store
                    .finish_device_turn(request, ReminderIntent::Clarify { question }, None)
                    .await
            }
            result => result,
        }
    }
}
