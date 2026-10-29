use crate::{failure, PROFILE};
use agentix_assistant_core::{Error, Record, RecordBody, RecordKind, Result};
use arrow_array::{
    ArrayRef, BooleanArray, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray,
    UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use std::{collections::HashMap, sync::Arc};

pub(crate) fn kind(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Note => "note",
        RecordKind::Task => "task",
        RecordKind::List => "list",
        RecordKind::ListItem => "list_item",
    }
}

pub(crate) fn schema() -> Arc<Schema> {
    Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("revision", DataType::UInt64, false),
            Field::new("deleted", DataType::Boolean, false),
            Field::new("kind", DataType::Utf8, false),
            Field::new("content", DataType::Utf8, false),
            Field::new("payload", DataType::Utf8, false),
        ],
        HashMap::from([
            ("agentix.personal.schema".into(), "1".into()),
            ("agentix.personal.profile".into(), PROFILE.into()),
            ("agentix.personal.dimensions".into(), "0".into()),
        ]),
    ))
}

pub(crate) fn batch(record: &Record) -> Result<RecordBatch> {
    let text = if record.deleted {
        ""
    } else {
        match &record.body {
            RecordBody::Note { text } | RecordBody::ListItem { text, .. } => text,
            RecordBody::Task { title, .. } => title,
            RecordBody::List { name } => name,
        }
    };
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec![record.id.to_string()])),
        Arc::new(UInt64Array::from(vec![record.revision])),
        Arc::new(BooleanArray::from(vec![record.deleted])),
        Arc::new(StringArray::from(vec![kind(record.body.kind())])),
        Arc::new(StringArray::from(vec![text])),
        Arc::new(StringArray::from(vec![
            serde_json::to_string(record).map_err(failure)?
        ])),
    ];
    RecordBatch::try_new(schema(), columns).map_err(failure)
}

pub(crate) fn reader(batch: RecordBatch) -> Box<dyn RecordBatchReader + Send> {
    let schema = batch.schema();
    Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema))
}

pub(crate) fn column<'a, T: 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<T>())
        .ok_or_else(|| Error::Projection(format!("invalid projection column {name}")))
}
