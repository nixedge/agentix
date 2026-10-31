// Real-store fixture assertions deliberately panic on test failures.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use agentix_assistant_core::*;
use agentix_assistant_sled::SledRepository;
use uuid::Uuid;

fn request(device: Uuid, text: &str) -> DeviceTurnRequest {
    DeviceTurnRequest {
        device_id: device,
        request: TellRequest {
            request_id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            text: text.into(),
            timezone: "America/New_York".into(),
        },
    }
}
fn create() -> ReminderIntent {
    ReminderIntent::Create {
        text: "laundry".into(),
        time: ReminderTime::After { seconds: 1200 },
    }
}

#[tokio::test]
async fn listing_speaks_three_local_deadlines_without_metadata_or_delivered_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = SledRepository::open(dir.path()).await.unwrap();
    let device = Uuid::new_v4();
    let server_id = store.enroll(device).await.unwrap().server_id;
    let mut first = None;
    for index in 0..21 {
        let req = request(device, "create synthetic reminder");
        store.begin_device_turn(req.clone()).await.unwrap();
        let result = store
            .finish_device_turn(
                req,
                ReminderIntent::Create {
                    text: format!("{index}: {}", "洋".repeat(300)),
                    time: ReminderTime::After { seconds: 1200 },
                },
                None,
            )
            .await
            .unwrap();
        if first.is_none() {
            first = Some(result.reminder_receipts[0].clone());
        }
    }
    let first = first.unwrap();
    store
        .report(ReminderReports {
            server_id,
            device_id: device,
            reports: vec![ReminderReport {
                report_id: Uuid::new_v4(),
                reminder_id: first.id,
                revision: 1,
                sequence: 1,
                state: ReminderDeliveryState::Armed,
            }],
        })
        .await
        .unwrap();
    let req = request(device, "list reminders");
    store.begin_device_turn(req.clone()).await.unwrap();
    let response = store
        .finish_device_turn(req.clone(), ReminderIntent::List, None)
        .await
        .unwrap();
    response.validate(&req).unwrap();
    let reply = &response.response.reply;
    assert!(reply.contains(" at "));
    assert!(reply.contains("18 more in Inbox."));
    assert_eq!(reply.matches("洋").count(), 3 * 77);
    assert!(!reply.contains(&first.id.to_string()));
    assert!(!reply.contains(&first.local_due().unwrap()));
    assert!(!reply.contains("Armed"));
    assert!(!reply.contains("reported"));
    assert!(response.reminder_receipts.is_empty());
    assert_eq!(store.snapshot(device).await.unwrap().reminders.len(), 21);
    store
        .report(ReminderReports {
            server_id,
            device_id: device,
            reports: vec![ReminderReport {
                report_id: Uuid::new_v4(),
                reminder_id: first.id,
                revision: 1,
                sequence: 2,
                state: ReminderDeliveryState::Posted,
            }],
        })
        .await
        .unwrap();
    let req = request(device, "list pending reminders before delivered history");
    store.begin_device_turn(req.clone()).await.unwrap();
    let response = store
        .finish_device_turn(req, ReminderIntent::List, None)
        .await
        .unwrap();
    assert!(!response.response.reply.contains(&first.id.to_string()));
    assert!(!response.response.reply.contains("0: "));
    assert!(response.response.reply.contains("17 more in Inbox."));
}

#[test]
fn spoken_times_use_owner_calendar_without_raw_timestamps() {
    let reference = "2026-10-31T23:30:00Z".parse().unwrap();
    assert_eq!(
        reminder_spoken_time(
            "2026-11-01T00:00:00Z".parse().unwrap(),
            reference,
            "America/New_York"
        )
        .unwrap(),
        "today at 8 PM"
    );
    assert_eq!(
        reminder_spoken_time(
            "2026-11-01T15:15:00Z".parse().unwrap(),
            reference,
            "America/New_York"
        )
        .unwrap(),
        "tomorrow at 10:15 AM"
    );
    assert!(reminder_spoken_time(reference, reference, "bad/zone").is_err());
}

#[tokio::test]
async fn dismissed_alarms_survive_reopen_reject_rearming_and_leave_spoken_list() {
    let dir = tempfile::tempdir().unwrap();
    let store = SledRepository::open(dir.path()).await.unwrap();
    let device = Uuid::new_v4();
    let server = store.enroll(device).await.unwrap().server_id;
    let req = request(device, "create");
    store.begin_device_turn(req.clone()).await.unwrap();
    let item = store
        .finish_device_turn(req, create(), None)
        .await
        .unwrap()
        .reminder_receipts[0]
        .clone();
    let ringing = ReminderReport {
        report_id: Uuid::new_v4(),
        reminder_id: item.id,
        revision: 1,
        sequence: 1,
        state: ReminderDeliveryState::Ringing,
    };
    let dismissed = ReminderReport {
        report_id: Uuid::new_v4(),
        sequence: 2,
        state: ReminderDeliveryState::Dismissed,
        ..ringing.clone()
    };
    for report in [ringing.clone(), dismissed.clone()] {
        store
            .report(ReminderReports {
                server_id: server,
                device_id: device,
                reports: vec![report],
            })
            .await
            .unwrap();
    }
    store.close().await.unwrap();
    let store = SledRepository::open(dir.path()).await.unwrap();
    assert_eq!(
        store.snapshot(device).await.unwrap().observations,
        vec![dismissed.clone()]
    );
    for state in [
        ReminderDeliveryState::Ringing,
        ReminderDeliveryState::Armed,
        ReminderDeliveryState::Posted,
    ] {
        let report = ReminderReport {
            report_id: Uuid::new_v4(),
            sequence: 3,
            state,
            ..dismissed.clone()
        };
        assert!(store
            .report(ReminderReports {
                server_id: server,
                device_id: device,
                reports: vec![report]
            })
            .await
            .is_err());
    }
    store
        .report(ReminderReports {
            server_id: server,
            device_id: device,
            reports: vec![ringing],
        })
        .await
        .unwrap();
    assert_eq!(
        store.snapshot(device).await.unwrap().observations,
        vec![dismissed]
    );
    let req = request(device, "list");
    store.begin_device_turn(req.clone()).await.unwrap();
    assert_eq!(
        store
            .finish_device_turn(req, ReminderIntent::List, None)
            .await
            .unwrap()
            .response
            .reply,
        "You have no pending reminders."
    );
    let req = request(device, "reschedule dismissed reminder");
    store.begin_device_turn(req.clone()).await.unwrap();
    let revised = store
        .finish_device_turn(
            req,
            ReminderIntent::Reschedule {
                id: item.id,
                expected_revision: 1,
                time: ReminderTime::After { seconds: 3600 },
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(revised.reminder_receipts[0].revision, 2);
    assert!(store
        .snapshot(device)
        .await
        .unwrap()
        .observations
        .is_empty());
}

#[tokio::test]
async fn create_receipt_deadline_and_device_binding_survive_reopen_and_replay() {
    let directory = tempfile::tempdir().unwrap();
    let device = Uuid::new_v4();
    let req = request(device, "remind me in twenty minutes");
    let store = SledRepository::open(directory.path()).await.unwrap();
    let namespace = store.enroll(device).await.unwrap().server_id;
    assert!(store.enroll(Uuid::new_v4()).await.is_err());
    let (turn, _) = store.begin_device_turn(req.clone()).await.unwrap();
    store.close().await.unwrap();
    let store = SledRepository::open(directory.path()).await.unwrap();
    assert_eq!(
        store
            .begin_device_turn(req.clone())
            .await
            .unwrap()
            .0
            .accepted_at,
        turn.accepted_at
    );
    let result = store
        .finish_device_turn(req.clone(), create(), None)
        .await
        .unwrap();
    assert_eq!(
        result.reminder_receipts[0].due_at,
        turn.accepted_at + chrono::Duration::seconds(1200)
    );
    assert!(result.response.outcomes.is_empty());
    assert_eq!(
        store
            .finish_device_turn(req.clone(), ReminderIntent::List, None)
            .await
            .unwrap(),
        result
    );
    let mut changed = req.clone();
    changed.request.text = "different".into();
    assert!(store.begin_device_turn(changed).await.is_err());
    assert!(store.device_bound(req.request.request_id).await.unwrap());
    store.close().await.unwrap();
    let store = SledRepository::open(directory.path()).await.unwrap();
    let snap = store.snapshot(device).await.unwrap();
    assert_eq!(snap.server_id, namespace);
    assert_eq!(snap.reminders.len(), 1);
    assert_eq!(store.begin_device_turn(req).await.unwrap().1, Some(result));
    store.close().await.unwrap();
    let db = sled::open(directory.path()).unwrap();
    assert_eq!(db.scan_prefix(b"reminder/due/").count(), 1);
}

#[tokio::test]
async fn cancellation_invalidates_due_index_and_reports_cannot_revive_old_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let store = SledRepository::open(dir.path()).await.unwrap();
    let device = Uuid::new_v4();
    let namespace = store.enroll(device).await.unwrap().server_id;
    let req = request(device, "create");
    store.begin_device_turn(req.clone()).await.unwrap();
    let item = store
        .finish_device_turn(req, create(), None)
        .await
        .unwrap()
        .reminder_receipts[0]
        .clone();
    let report = ReminderReport {
        report_id: Uuid::new_v4(),
        reminder_id: item.id,
        revision: 1,
        sequence: 1,
        state: ReminderDeliveryState::Armed,
    };
    let reports = ReminderReports {
        server_id: namespace,
        device_id: device,
        reports: vec![report.clone()],
    };
    assert_eq!(
        store.report(reports.clone()).await.unwrap(),
        vec![report.report_id]
    );
    assert!(store.report(reports).await.is_ok());
    let mut changed = report.clone();
    changed.state = ReminderDeliveryState::Posted;
    assert!(store
        .report(ReminderReports {
            server_id: namespace,
            device_id: device,
            reports: vec![changed]
        })
        .await
        .is_err());
    let req = request(device, "cancel");
    store.begin_device_turn(req.clone()).await.unwrap();
    let result = store
        .finish_device_turn(
            req,
            ReminderIntent::Cancel {
                id: item.id,
                expected_revision: 1,
            },
            None,
        )
        .await
        .unwrap();
    assert!(result.reminder_receipts[0].cancelled);
    let stale = ReminderReport {
        report_id: Uuid::new_v4(),
        sequence: 2,
        state: ReminderDeliveryState::Posted,
        ..report
    };
    store
        .report(ReminderReports {
            server_id: namespace,
            device_id: device,
            reports: vec![stale],
        })
        .await
        .unwrap();
    let snap = store.snapshot(device).await.unwrap();
    assert!(snap.observations.is_empty());
    assert!(snap.reminders[0].cancelled);
    let cancelled = ReminderReport {
        report_id: Uuid::new_v4(),
        reminder_id: item.id,
        revision: 2,
        sequence: 3,
        state: ReminderDeliveryState::CancelApplied,
    };
    store
        .report(ReminderReports {
            server_id: namespace,
            device_id: device,
            reports: vec![cancelled.clone()],
        })
        .await
        .unwrap();
    assert_eq!(
        store.snapshot(device).await.unwrap().observations,
        vec![cancelled]
    );
    store.close().await.unwrap();
    let db = sled::open(dir.path()).unwrap();
    assert_eq!(db.scan_prefix(b"reminder/due/").count(), 0);
}

#[tokio::test]
async fn stale_edits_are_atomic_and_concurrent_replays_create_one_reminder() {
    let dir = tempfile::tempdir().unwrap();
    let store = SledRepository::open(dir.path()).await.unwrap();
    let device = Uuid::new_v4();
    store.enroll(device).await.unwrap();
    let req = request(device, "create");
    store.begin_device_turn(req.clone()).await.unwrap();
    let (a, b) = tokio::join!(
        store.finish_device_turn(req.clone(), create(), None),
        store.finish_device_turn(req, create(), None)
    );
    assert_eq!(a.unwrap(), b.unwrap());
    let item = store.snapshot(device).await.unwrap().reminders[0].clone();
    let req = request(device, "move");
    store.begin_device_turn(req.clone()).await.unwrap();
    let result = store
        .finish_device_turn(
            req,
            ReminderIntent::Reschedule {
                id: item.id,
                expected_revision: 1,
                time: ReminderTime::After { seconds: 3600 },
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.reminder_receipts[0].revision, 2);
    let req = request(device, "stale cancel");
    store.begin_device_turn(req.clone()).await.unwrap();
    assert!(store
        .finish_device_turn(
            req.clone(),
            ReminderIntent::Cancel {
                id: item.id,
                expected_revision: 1
            },
            None
        )
        .await
        .is_err());
    assert_eq!(
        store.snapshot(device).await.unwrap().reminders[0],
        result.reminder_receipts[0]
    );
    store
        .finish_device_turn(
            req,
            ReminderIntent::Clarify {
                question: "The reminder changed; please identify it again.".into(),
            },
            None,
        )
        .await
        .unwrap();
}

#[test]
fn time_resolution_rejects_dst_gaps_and_folds_and_freezes_relative_instant() {
    let accepted = "2026-01-01T00:00:00Z".parse().unwrap();
    for (date, time) in [("2026-03-08", "02:30:00"), ("2026-11-01", "01:30:00")] {
        assert!(ReminderTime::Local {
            date: date.parse().unwrap(),
            time: time.parse().unwrap()
        }
        .resolve(accepted, "America/New_York")
        .is_err());
    }
    assert_eq!(
        ReminderTime::After { seconds: 1200 }
            .resolve(accepted, "UTC")
            .unwrap(),
        accepted + chrono::Duration::seconds(1200)
    );
    assert!(ReminderTime::After { seconds: 0 }
        .resolve(accepted, "UTC")
        .is_err());
}

#[tokio::test]
async fn posted_observations_are_terminal_and_unknown_reminder_schema_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = SledRepository::open(dir.path()).await.unwrap();
    let device = Uuid::new_v4();
    let server = store.enroll(device).await.unwrap().server_id;
    let req = request(device, "create");
    store.begin_device_turn(req.clone()).await.unwrap();
    let item = store
        .finish_device_turn(req, create(), None)
        .await
        .unwrap()
        .reminder_receipts[0]
        .clone();
    let posted = ReminderReport {
        report_id: Uuid::new_v4(),
        reminder_id: item.id,
        revision: 1,
        sequence: 2,
        state: ReminderDeliveryState::Posted,
    };
    store
        .report(ReminderReports {
            server_id: server,
            device_id: device,
            reports: vec![posted.clone()],
        })
        .await
        .unwrap();
    let armed = ReminderReport {
        report_id: Uuid::new_v4(),
        sequence: 3,
        state: ReminderDeliveryState::Armed,
        ..posted.clone()
    };
    assert!(store
        .report(ReminderReports {
            server_id: server,
            device_id: device,
            reports: vec![armed]
        })
        .await
        .is_err());
    assert_eq!(
        store.snapshot(device).await.unwrap().observations,
        vec![posted]
    );
    store.close().await.unwrap();
    {
        let db = sled::open(dir.path()).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&db.get(b"reminder/meta").unwrap().unwrap()).unwrap();
        value["version"] = serde_json::json!(99);
        db.insert(b"reminder/meta", serde_json::to_vec(&value).unwrap())
            .unwrap();
        db.flush().unwrap();
    }
    let store = SledRepository::open(dir.path()).await.unwrap();
    assert!(matches!(
        store.snapshot(device).await,
        Err(Error::Schema(_))
    ));
}
