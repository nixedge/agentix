use crate::{conversations::*, decode, encode, storage, SledRepository};
use agentix_assistant_core::*;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

const BOOK: &[u8] = b"reminder/meta";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Book {
    version: u32,
    server_id: Uuid,
    device_id: Uuid,
    sequence: u64,
    reminders: BTreeMap<Uuid, Reminder>,
    observations: BTreeMap<Uuid, ReminderReport>,
    observed_at: BTreeMap<Uuid, chrono::DateTime<Utc>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundTurn {
    request: DeviceTurnRequest,
    result: Option<DeviceTurnResponse>,
}
fn bound(id: Uuid) -> Vec<u8> {
    format!("reminder/turn/{id}").into_bytes()
}
fn report_key(id: Uuid) -> Vec<u8> {
    format!("reminder/report/{id}").into_bytes()
}
fn due(item: &Reminder) -> Vec<u8> {
    let order = (item.due_at.timestamp_millis() as u64) ^ (1_u64 << 63);
    format!("reminder/due/{order:016x}/{}", item.id).into_bytes()
}
fn book(db: &sled::Db, device: Uuid) -> Result<Book> {
    reminder_uuid(device)?;
    let bytes = db
        .get(BOOK)
        .map_err(storage)?
        .ok_or_else(|| Error::Invalid("Enroll this phone before scheduling reminders.".into()))?;
    let book: Book = decode(&bytes)?;
    if book.version != 1 {
        return Err(Error::Schema("unsupported reminder schema".into()));
    }
    if book.device_id != device {
        return Err(Error::RequestConflict);
    }
    snapshot(&book).validate()?;
    Ok(book)
}
fn snapshot(book: &Book) -> ReminderSnapshot {
    ReminderSnapshot {
        server_id: book.server_id,
        device_id: book.device_id,
        sequence: book.sequence,
        reminders: book.reminders.values().cloned().collect(),
        observations: book.observations.values().cloned().collect(),
    }
}
fn read_bound(db: &sled::Db, request: &DeviceTurnRequest) -> Result<BoundTurn> {
    let value: BoundTurn = decode(
        &db.get(bound(request.request.request_id))
            .map_err(storage)?
            .ok_or(Error::RequestConflict)?,
    )?;
    if value.request != *request {
        return Err(Error::RequestConflict);
    }
    if let Some(result) = &value.result {
        result.validate(request)?;
    }
    Ok(value)
}

#[async_trait]
impl Reminders for SledRepository {
    async fn enroll(&self, device_id: Uuid) -> Result<ReminderSnapshot> {
        reminder_uuid(device_id)?;
        self.blocking(move |db| {
            if db.contains_key(BOOK).map_err(storage)? {
                return Ok(snapshot(&book(db, device_id)?));
            }
            let value = Book {
                version: 1,
                server_id: Uuid::new_v4(),
                device_id,
                sequence: 0,
                reminders: BTreeMap::new(),
                observations: BTreeMap::new(),
                observed_at: BTreeMap::new(),
            };
            db.insert(BOOK, encode(&value)?).map_err(storage)?;
            db.flush().map_err(storage)?;
            Ok(snapshot(&value))
        })
        .await
    }
    async fn snapshot(&self, device_id: Uuid) -> Result<ReminderSnapshot> {
        self.blocking(move |db| Ok(snapshot(&book(db, device_id)?)))
            .await
    }
    async fn device_bound(&self, request_id: Uuid) -> Result<bool> {
        self.blocking(move |db| db.contains_key(bound(request_id)).map_err(storage))
            .await
    }
    async fn report(&self, request: ReminderReports) -> Result<Vec<Uuid>> {
        if request.reports.len() > 100 {
            return Err(Error::Invalid("too many device reports".into()));
        }
        for report in &request.reports {
            report.validate()?;
        }
        self.blocking(move |db| {
            let mut value = book(db, request.device_id)?;
            if value.server_id != request.server_id {
                return Err(Error::RequestConflict);
            }
            let mut batch = sled::Batch::default();
            let mut accepted = Vec::new();
            let mut seen = BTreeMap::new();
            for report in &request.reports {
                if let Some(previous) = seen.insert(report.report_id, report) {
                    if previous != report {
                        return Err(Error::RequestConflict);
                    }
                }
                if let Some(bytes) = db.get(report_key(report.report_id)).map_err(storage)? {
                    let previous: ReminderReport = decode(&bytes)?;
                    if previous != *report {
                        return Err(Error::RequestConflict);
                    }
                    accepted.push(report.report_id);
                    continue;
                }
                let item = value
                    .reminders
                    .get(&report.reminder_id)
                    .ok_or(Error::NotFound(report.reminder_id))?;
                if report.revision > item.revision {
                    return Err(Error::RequestConflict);
                }
                if report.revision == item.revision {
                    if (report.state == ReminderDeliveryState::CancelApplied) != item.cancelled {
                        return Err(Error::Invalid(
                            "device outcome disagrees with cancellation".into(),
                        ));
                    }
                    let old = value.observations.get(&item.id);
                    if old.is_some_and(|old| {
                        old.revision == report.revision
                            && old.sequence < report.sequence
                            && old.state == ReminderDeliveryState::Posted
                            && report.state != ReminderDeliveryState::Posted
                    }) {
                        return Err(Error::Invalid(
                            "a posted reminder cannot be rearmed at the same revision".into(),
                        ));
                    }
                    if old.is_some_and(|old| {
                        old.revision == report.revision
                            && old.sequence == report.sequence
                            && old != report
                    }) {
                        return Err(Error::RequestConflict);
                    }
                    if old.is_none_or(|old| {
                        old.revision < report.revision || old.sequence < report.sequence
                    }) {
                        value.observations.insert(item.id, report.clone());
                        value.observed_at.insert(item.id, Utc::now());
                    }
                }
                batch.insert(report_key(report.report_id), encode(report)?);
                accepted.push(report.report_id);
            }
            batch.insert(BOOK, encode(&value)?);
            db.apply_batch(batch).map_err(storage)?;
            db.flush().map_err(storage)?;
            Ok(accepted)
        })
        .await
    }
    async fn begin_device_turn(
        &self,
        request: DeviceTurnRequest,
    ) -> Result<(Turn, Option<DeviceTurnResponse>)> {
        request.validate()?;
        self.blocking(move |db| {
            book(db, request.device_id)?;
            let id = request.request.request_id;
            if db.contains_key(bound(id)).map_err(storage)? {
                let existing = read_bound(db, &request)?;
                db.flush().map_err(storage)?;
                return Ok((load_turn(db, id)?, existing.result));
            }
            if db.contains_key(turn_key(id)).map_err(storage)?
                || db.contains_key(crate::keys::request(id)).map_err(storage)?
            {
                return Err(Error::RequestConflict);
            }
            let mut meta = conversation(db, request.request.conversation_id)?;
            if meta.active.is_some() {
                return Err(Error::Busy);
            }
            meta.active = Some(id);
            let turn = Turn {
                schema_version: 1,
                request: request.request.clone(),
                accepted_at: Utc::now(),
                state: TurnState::Received,
            };
            let mut batch = sled::Batch::default();
            append(
                db,
                &mut batch,
                &mut meta,
                Message {
                    conversation_id: request.request.conversation_id,
                    sequence: 0,
                    request_id: id,
                    role: MessageRole::User,
                    text: request.request.text.clone(),
                    created_at: turn.accepted_at,
                },
            )?;
            batch.insert(
                conversation_key(request.request.conversation_id),
                encode(&meta)?,
            );
            batch.insert(turn_key(id), encode(&turn)?);
            batch.insert(
                bound(id),
                encode(&BoundTurn {
                    request,
                    result: None,
                })?,
            );
            db.apply_batch(batch).map_err(storage)?;
            db.flush().map_err(storage)?;
            Ok((turn, None))
        })
        .await
    }
    async fn finish_device_turn(
        &self,
        request: DeviceTurnRequest,
        intent: ReminderIntent,
        ordinary: Option<TellResponse>,
    ) -> Result<DeviceTurnResponse> {
        request.validate()?;
        self.blocking(move |db| {
            let mut binding = read_bound(db, &request)?;
            if let Some(response) = binding.result { db.flush().map_err(storage)?; return Ok(response); }
            let mut value = book(db, request.device_id)?;
            let id = request.request.request_id;
            let mut turn = load_turn(db, id)?;
            let mut batch = sled::Batch::default();
            let mut receipts = Vec::new();
            let response = if let Some(response) = ordinary {
                if intent != ReminderIntent::Other { return Err(Error::RequestConflict); }
                response.validate(&request.request)?;
                let actual = match &turn.state { TurnState::Completed {response} | TurnState::GroceryCompleted {response,..} => response, _ => return Err(Error::RequestConflict) };
                if *actual != response { return Err(Error::RequestConflict); }
                response
            } else {
                if turn.state != TurnState::Received { return Err(Error::RequestConflict); }
                let mut status = TellStatus::Answer;
                let reply = match intent {
                    ReminderIntent::Other => return Err(Error::Invalid("ordinary result required".into())),
                    ReminderIntent::Clarify {question} => { bounded_text(&question, 1024)?; status = TellStatus::Clarification; question },
                    ReminderIntent::List => {
                        let mut items = value.reminders.values().filter(|r| !r.cancelled).collect::<Vec<_>>();
                        items.sort_by_key(|r| (value.observations.get(&r.id).is_some_and(|o| o.state == ReminderDeliveryState::Posted), r.due_at));
                        if items.is_empty() { "You have no active reminders.".into() }
                        else { format!("Reminders: {}. Backend state; phone application is reported separately.{}", items.iter().take(20).map(|r| {
                            let text:String=r.text.chars().take(40).collect();
                            let state=value.observations.get(&r.id).map(|o| format!("last reported {:?}{}",o.state,value.observed_at.get(&r.id).map(|at| format!(" at {at}")).unwrap_or_default())).unwrap_or_else(|| "awaiting phone".into());
                            format!("{text} at {} [{}] ({state})",r.local_due().unwrap_or_else(|_| r.due_at.to_rfc3339()),r.id)
                        }).collect::<Vec<_>>().join("; "), if items.len()>20 {" More reminders are stored."} else {""}) }
                    },
                    ReminderIntent::Create {text,time} => {
                        bounded_text(&text,1024)?;
                        if value.reminders.len() >= MAX_REMINDERS { return Err(Error::Invalid("reminder capacity reached".into())); }
                        let item=Reminder { id:Uuid::new_v4(),revision:1,text,due_at:time.resolve(turn.accepted_at,&request.request.timezone)?,timezone:request.request.timezone.clone(),cancelled:false };
                        item.validate()?;
                        batch.insert(due(&item), item.id.as_bytes());
                        value.reminders.insert(item.id,item.clone()); receipts.push(item.clone());
                        format!("Saved reminder for {} ({}). Awaiting application on your Android phone.", item.local_due()?, item.timezone)
                    },
                    ReminderIntent::Reschedule {id,expected_revision,time} => {
                        let item=value.reminders.get_mut(&id).ok_or(Error::NotFound(id))?;
                        if item.revision != expected_revision { return Err(Error::RevisionConflict {expected:expected_revision,actual:item.revision}); }
                        if item.cancelled { return Err(Error::Invalid("That reminder is cancelled; create a new reminder.".into())); }
                        batch.remove(due(item));
                        item.due_at=time.resolve(turn.accepted_at,&request.request.timezone)?;
                        item.timezone=request.request.timezone.clone();
                        item.revision=item.revision.checked_add(1).ok_or_else(|| Error::Storage("revision exhausted".into()))?;
                        batch.insert(due(item),item.id.as_bytes()); receipts.push(item.clone()); value.observations.remove(&id); value.observed_at.remove(&id);
                        "Rescheduled on the backend. The previous alarm may still fire until your phone applies this change.".into()
                    },
                    ReminderIntent::Cancel {id,expected_revision} => {
                        let item=value.reminders.get_mut(&id).ok_or(Error::NotFound(id))?;
                        if item.revision != expected_revision { return Err(Error::RevisionConflict {expected:expected_revision,actual:item.revision}); }
                        if !item.cancelled { batch.remove(due(item)); item.cancelled=true;
                            item.revision=item.revision.checked_add(1).ok_or_else(|| Error::Storage("revision exhausted".into()))?;
                            value.observations.remove(&id); value.observed_at.remove(&id); }
                        receipts.push(item.clone());
                        "Cancelled on the backend. Awaiting phone application; an offline or in-flight notification may still arrive.".into()
                    },
                };
                if !receipts.is_empty() { value.sequence=value.sequence.checked_add(1).ok_or_else(|| Error::Storage("sync sequence exhausted".into()))?; }
                let response=TellResponse {request_id:id,conversation_id:request.request.conversation_id,status,reply,outcomes:vec![],records:vec![]};
                response.validate(&request.request)?;
                let mut meta=conversation(db,request.request.conversation_id)?;
                if meta.active != Some(id) { return Err(Error::RequestConflict); }
                append(db,&mut batch,&mut meta,Message { conversation_id:request.request.conversation_id,sequence:0,request_id:id,role:MessageRole::Assistant,text:response.reply.clone(),created_at:Utc::now() })?;
                meta.active=None;
                turn.state=TurnState::Completed {response:response.clone()};
                batch.insert(conversation_key(request.request.conversation_id),encode(&meta)?);
                batch.insert(turn_key(id),encode(&turn)?);
                batch.insert(BOOK,encode(&value)?);
                response
            };
            let result=DeviceTurnResponse {device_id:request.device_id,response,reminder_receipts:receipts};
            result.validate(&request)?;
            binding.result=Some(result.clone()); batch.insert(bound(id),encode(&binding)?);
            db.apply_batch(batch).map_err(storage)?; db.flush().map_err(storage)?;
            Ok(result)
        }).await
    }
}
