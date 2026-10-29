use crate::{decode, encode, keys, storage, SledRepository};
use agentix_assistant_core::{
    Conversations, Error, Message, MessageRole, PreparedPlan, Result, TellRequest, TellResponse,
    TellStatus, Turn, TurnState,
};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Conversation {
    next_sequence: u64,
    active: Option<Uuid>,
}

fn turn_key(id: Uuid) -> Vec<u8> {
    format!("turn/{id}").into_bytes()
}
fn conversation_key(id: Uuid) -> Vec<u8> {
    format!("conversation/{id}").into_bytes()
}
fn messages_prefix(id: Uuid) -> Vec<u8> {
    format!("message/{id}/").into_bytes()
}
fn message_key(id: Uuid, sequence: u64) -> Vec<u8> {
    format!("message/{id}/{sequence:020}").into_bytes()
}

fn load_turn(db: &sled::Db, id: Uuid) -> Result<Turn> {
    let turn: Turn = decode(
        &db.get(turn_key(id))
            .map_err(storage)?
            .ok_or(Error::NotFound(id))?,
    )?;
    turn.validate()?;
    if turn.request.request_id != id {
        return Err(Error::Storage("turn identity disagrees with key".into()));
    }
    Ok(turn)
}

fn conversation(db: &sled::Db, id: Uuid) -> Result<Conversation> {
    let value: Conversation = db
        .get(conversation_key(id))
        .map_err(storage)?
        .map(|bytes| decode(&bytes))
        .transpose()?
        .unwrap_or(Conversation {
            next_sequence: 1,
            active: None,
        });
    if value.next_sequence == 0 || value.active.is_some_and(|id| id.is_nil()) {
        return Err(Error::Storage("invalid conversation metadata".into()));
    }
    Ok(value)
}

fn append(
    db: &sled::Db,
    batch: &mut sled::Batch,
    meta: &mut Conversation,
    mut message: Message,
) -> Result<()> {
    message.sequence = meta.next_sequence;
    message.validate()?;
    let key = message_key(message.conversation_id, message.sequence);
    if db.contains_key(&key).map_err(storage)? {
        return Err(Error::Storage(
            "conversation sequence already exists".into(),
        ));
    }
    meta.next_sequence = meta
        .next_sequence
        .checked_add(1)
        .ok_or_else(|| Error::Storage("message sequence exhausted".into()))?;
    batch.insert(key, encode(&message)?);
    Ok(())
}

#[async_trait]
impl Conversations for SledRepository {
    async fn begin(&self, request: TellRequest) -> Result<Turn> {
        request.validate()?;
        self.blocking(move |db| {
            if db
                .contains_key(turn_key(request.request_id))
                .map_err(storage)?
            {
                let turn = load_turn(db, request.request_id)?;
                if turn.request != request {
                    return Err(Error::RequestConflict);
                }
                db.flush().map_err(storage)?;
                return Ok(turn);
            }
            let mut meta = conversation(db, request.conversation_id)?;
            if meta.active.is_some() {
                return Err(Error::Busy);
            }
            meta.active = Some(request.request_id);
            let turn = Turn {
                schema_version: 1,
                request,
                accepted_at: Utc::now(),
                state: TurnState::Received,
            };
            let mut batch = sled::Batch::default();
            append(
                db,
                &mut batch,
                &mut meta,
                Message {
                    conversation_id: turn.request.conversation_id,
                    sequence: 0,
                    request_id: turn.request.request_id,
                    role: MessageRole::User,
                    text: turn.request.text.clone(),
                    created_at: turn.accepted_at,
                },
            )?;
            batch.insert(
                conversation_key(turn.request.conversation_id),
                encode(&meta)?,
            );
            batch.insert(turn_key(turn.request.request_id), encode(&turn)?);
            db.apply_batch(batch).map_err(storage)?;
            db.flush().map_err(storage)?;
            Ok(turn)
        })
        .await
    }

    async fn prepare(&self, request_id: Uuid, plan: PreparedPlan) -> Result<Turn> {
        plan.validate()?;
        self.blocking(move |db| {
            let mut turn = load_turn(db, request_id)?;
            match &turn.state {
                TurnState::Received => {}
                TurnState::Prepared { plan: previous } if *previous == plan => {
                    db.flush().map_err(storage)?;
                    return Ok(turn);
                }
                TurnState::Prepared { .. } => return Err(Error::RequestConflict),
                TurnState::Completed { .. } => {
                    return Err(Error::Invalid(
                        "completed turn cannot be prepared again".into(),
                    ))
                }
            }
            if conversation(db, turn.request.conversation_id)?.active != Some(request_id) {
                return Err(Error::Storage(
                    "turn is not active in its conversation".into(),
                ));
            }
            turn.state = TurnState::Prepared { plan };
            db.insert(turn_key(request_id), encode(&turn)?)
                .map_err(storage)?;
            db.flush().map_err(storage)?;
            Ok(turn)
        })
        .await
    }

    async fn complete(&self, request_id: Uuid, response: TellResponse) -> Result<TellResponse> {
        self.blocking(move |db| {
            let mut turn = load_turn(db, request_id)?;
            response.validate(&turn.request)?;
            let plan = match &turn.state {
                TurnState::Prepared { plan } => plan,
                TurnState::Completed { response: original } if *original == response => {
                    db.flush().map_err(storage)?;
                    return Ok(response);
                }
                TurnState::Completed { .. } => return Err(Error::RequestConflict),
                TurnState::Received => {
                    return Err(Error::Invalid("turn has no durable plan".into()))
                }
            };
            if response.outcomes.len() > plan.commands.len() {
                return Err(Error::Invalid("too many command outcomes".into()));
            }
            for (outcome, command) in response.outcomes.iter().zip(&plan.commands) {
                let actual = db
                    .get(keys::request(command.request_id))
                    .map_err(storage)?
                    .map(|bytes| decode(&bytes))
                    .transpose()?;
                if outcome.request_id != command.request_id
                    || outcome.command != command.command
                    || actual.as_ref() != Some(outcome)
                {
                    return Err(Error::Storage(
                        "turn outcome has no matching durable command receipt".into(),
                    ));
                }
            }
            for command in plan.commands.iter().skip(response.outcomes.len()) {
                if db
                    .contains_key(keys::request(command.request_id))
                    .map_err(storage)?
                {
                    return Err(Error::Storage(
                        "response omits a committed command outcome".into(),
                    ));
                }
            }
            match response.status {
                TellStatus::Saved
                    if plan.status == TellStatus::Saved
                        && response.outcomes.len() == plan.commands.len()
                        && response.reply == plan.reply => {}
                TellStatus::Partial
                    if !response.outcomes.is_empty()
                        && response.outcomes.len() < plan.commands.len() => {}
                TellStatus::Failed if response.outcomes.is_empty() => {}
                TellStatus::Answer | TellStatus::Clarification
                    if plan.commands.is_empty()
                        && response.status == plan.status
                        && response.reply == plan.reply => {}
                _ => {
                    return Err(Error::Invalid(
                        "response disagrees with execution plan".into(),
                    ))
                }
            }
            if response.records != plan.records {
                return Err(Error::Invalid(
                    "read result disagrees with durable plan".into(),
                ));
            }
            let mut meta = conversation(db, turn.request.conversation_id)?;
            if meta.active != Some(request_id) {
                return Err(Error::Storage("conversation active turn mismatch".into()));
            }
            meta.active = None;
            let mut batch = sled::Batch::default();
            append(
                db,
                &mut batch,
                &mut meta,
                Message {
                    conversation_id: turn.request.conversation_id,
                    sequence: 0,
                    request_id,
                    role: MessageRole::Assistant,
                    text: response.reply.clone(),
                    created_at: Utc::now().max(turn.accepted_at),
                },
            )?;
            turn.state = TurnState::Completed {
                response: response.clone(),
            };
            batch.insert(turn_key(request_id), encode(&turn)?);
            batch.insert(
                conversation_key(turn.request.conversation_id),
                encode(&meta)?,
            );
            db.apply_batch(batch).map_err(storage)?;
            db.flush().map_err(storage)?;
            Ok(response)
        })
        .await
    }

    async fn recent_messages(&self, conversation_id: Uuid, limit: usize) -> Result<Vec<Message>> {
        if conversation_id.is_nil() || !(1..=100).contains(&limit) {
            return Err(Error::Invalid("invalid history ID or limit".into()));
        }
        self.blocking(move |db| {
            let mut messages = db
                .scan_prefix(messages_prefix(conversation_id))
                .rev()
                .take(limit)
                .map(|entry| {
                    let (key, bytes) = entry.map_err(storage)?;
                    let message: Message = decode(&bytes)?;
                    message.validate()?;
                    if message.conversation_id != conversation_id
                        || message_key(conversation_id, message.sequence).as_slice() != key.as_ref()
                    {
                        return Err(Error::Storage("message identity disagrees with key".into()));
                    }
                    Ok(message)
                })
                .collect::<Result<Vec<_>>>()?;
            messages.reverse();
            Ok(messages)
        })
        .await
    }
}
