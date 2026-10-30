// Scripted model proposals exercise actual Sled/orchestration, not model quality.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use agentix_assistant::Assistant;
use agentix_assistant_core::*;
use agentix_assistant_sled::SledRepository;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
struct Model(Mutex<Vec<ReminderIntent>>);
#[async_trait]
impl Inference for Model {
    async fn propose(&self, context: InferenceContext) -> Result<Intent> {
        Ok(Intent::SaveNote {
            text: context.request.text,
        })
    }
}
#[async_trait]
impl ReminderInference for Model {
    async fn propose_reminder(&self, _: ReminderContext) -> Result<ReminderIntent> {
        Ok(self.0.lock().unwrap().remove(0))
    }
}
fn request(device_id: Uuid, text: &str) -> DeviceTurnRequest {
    DeviceTurnRequest {
        device_id,
        request: TellRequest {
            request_id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            text: text.into(),
            timezone: "UTC".into(),
        },
    }
}
#[tokio::test]
async fn device_turn_replay_and_v1_identity_guard_preserve_real_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SledRepository::open(dir.path()).await.unwrap());
    let device = Uuid::new_v4();
    store.enroll(device).await.unwrap();
    let model = Arc::new(Model(Mutex::new(vec![
        ReminderIntent::Create {
            text: "laundry".into(),
            time: ReminderTime::After { seconds: 1200 },
        },
        ReminderIntent::Other,
    ])));
    let app = Arc::new(
        Assistant::new(store.clone(), store.clone(), model.clone())
            .with_reminders(store.clone(), model),
    );
    let req = request(device, "remind me in twenty minutes to take laundry out");
    let reply = app.tell_device(req.clone()).await.unwrap();
    assert_eq!(reply.reminder_receipts.len(), 1);
    assert!(reply.response.outcomes.is_empty());
    assert_eq!(app.tell_device(req.clone()).await.unwrap(), reply);
    assert!(matches!(
        app.tell(req.request).await,
        Err(Error::RequestConflict)
    ));
    let req = request(device, "spare key in garage");
    let reply = app.tell_device(req.clone()).await.unwrap();
    assert_eq!(reply.response.status, TellStatus::Saved);
    assert_eq!(reply.response.outcomes.len(), 1);
    assert!(reply.reminder_receipts.is_empty());
    assert_eq!(app.tell_device(req).await.unwrap(), reply);
    assert_eq!(store.records().await.unwrap().len(), 1);
}
#[tokio::test]
async fn invalid_time_completes_clarification_without_a_schedule_or_local_save() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SledRepository::open(dir.path()).await.unwrap());
    let device = Uuid::new_v4();
    store.enroll(device).await.unwrap();
    let model = Arc::new(Model(Mutex::new(vec![ReminderIntent::Create {
        text: "laundry".into(),
        time: ReminderTime::After { seconds: 0 },
    }])));
    let app = Arc::new(
        Assistant::new(store.clone(), store.clone(), model.clone())
            .with_reminders(store.clone(), model),
    );
    let req = request(device, "remind me");
    let reply = app.tell_device(req.clone()).await.unwrap();
    assert_eq!(reply.response.status, TellStatus::Clarification);
    assert!(store.snapshot(device).await.unwrap().reminders.is_empty());
    assert!(store.records().await.unwrap().is_empty());
    assert_eq!(app.tell_device(req).await.unwrap(), reply);
}
