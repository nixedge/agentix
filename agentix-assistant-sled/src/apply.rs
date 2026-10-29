use crate::{decode, encode, keys, storage};
use agentix_assistant_core::{
    Command, CommandReceipt, CommandRequest, Error, ProjectionJob, Record, RecordBody, Result,
    SCHEMA_VERSION,
};
use chrono::Utc;
use uuid::Uuid;

pub(crate) fn load_record(db: &sled::Db, id: Uuid) -> Result<Option<Record>> {
    db.get(keys::record(id))
        .map_err(storage)?
        .map(|bytes| {
            let record: Record = decode(&bytes)?;
            record.validate()?;
            if record.id != id {
                return Err(Error::Storage("record identity disagrees with key".into()));
            }
            Ok(record)
        })
        .transpose()
}

fn check_parent(db: &sled::Db, body: &RecordBody) -> Result<()> {
    if let Some(list_id) = body.list_id() {
        let parent = load_record(db, list_id)?
            .filter(|record| !record.deleted)
            .ok_or(Error::NotFound(list_id))?;
        if !matches!(parent.body, RecordBody::List { .. }) {
            return Err(Error::Invalid("list item parent must be a list".into()));
        }
    }
    Ok(())
}

pub(crate) fn apply(db: &sled::Db, request: CommandRequest) -> Result<CommandReceipt> {
    request.validate()?;
    let request_key = keys::request(request.request_id);
    if let Some(bytes) = db.get(&request_key).map_err(storage)? {
        let receipt: CommandReceipt = decode(&bytes)?;
        receipt.validate()?;
        if receipt.request_id != request.request_id {
            return Err(Error::Storage("receipt identity disagrees with key".into()));
        }
        if receipt.command != request.command {
            return Err(Error::RequestConflict);
        }
        // A previous flush may have failed after the batch applied. Resolve replay durably too.
        db.flush().map_err(storage)?;
        return Ok(receipt);
    }

    let id = request.command.id();
    let previous = load_record(db, id)?;
    let now = Utc::now();
    let record = match &request.command {
        Command::Create { body, .. } => {
            if previous.is_some() {
                return Err(Error::AlreadyExists(id));
            }
            check_parent(db, body)?;
            Record {
                schema_version: SCHEMA_VERSION,
                id,
                revision: 1,
                created_at: now,
                updated_at: now,
                deleted: false,
                body: body.clone(),
            }
        }
        Command::Replace {
            expected_revision, ..
        }
        | Command::Delete {
            expected_revision, ..
        } => {
            let mut record = previous
                .clone()
                .filter(|record| !record.deleted)
                .ok_or(Error::NotFound(id))?;
            if record.revision != *expected_revision {
                return Err(Error::RevisionConflict {
                    expected: *expected_revision,
                    actual: record.revision,
                });
            }
            match &request.command {
                Command::Replace { body, .. } => {
                    if record.body.kind() != body.kind() {
                        return Err(Error::Invalid("record kind cannot change".into()));
                    }
                    check_parent(db, body)?;
                    record.body = body.clone();
                }
                Command::Delete { .. } => {
                    if matches!(record.body, RecordBody::List { .. })
                        && db
                            .scan_prefix(keys::members(id))
                            .next()
                            .transpose()
                            .map_err(storage)?
                            .is_some()
                    {
                        return Err(Error::Invalid(
                            "cannot delete a list with live items".into(),
                        ));
                    }
                    record.deleted = true;
                }
                Command::Create { .. } => {
                    return Err(Error::Invalid("invalid update transition".into()))
                }
            }
            record.revision = record
                .revision
                .checked_add(1)
                .ok_or_else(|| Error::Invalid("revision exhausted".into()))?;
            record.updated_at = now.max(record.updated_at);
            record
        }
    };
    record.validate()?;
    let receipt = CommandReceipt {
        request_id: request.request_id,
        command: request.command,
        record,
    };
    let mut batch = sled::Batch::default();
    if let Some(parent) = previous.and_then(|record| record.body.list_id()) {
        batch.remove(keys::member(parent, id));
    }
    if !receipt.record.deleted {
        if let Some(parent) = receipt.record.body.list_id() {
            batch.insert(keys::member(parent, id), Vec::new());
        }
    }
    batch.insert(keys::record(id), encode(&receipt.record)?);
    batch.insert(request_key, encode(&receipt)?);
    batch.insert(
        keys::projection(id),
        encode(&ProjectionJob {
            record_id: id,
            revision: receipt.record.revision,
        })?,
    );
    db.apply_batch(batch).map_err(storage)?;
    db.flush().map_err(storage)?;
    Ok(receipt)
}
