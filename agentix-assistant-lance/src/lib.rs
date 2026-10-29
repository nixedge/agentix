mod rows;

use agentix_assistant_core::{Error, Projection, Record, Result, SearchHit, SearchRequest};
use arrow_array::{Float32Array, RecordBatch, StringArray, UInt64Array};
use async_trait::async_trait;
use futures::TryStreamExt;
use lancedb::{
    index::{
        scalar::{FtsIndexBuilder, FullTextSearchQuery},
        Index,
    },
    query::{ExecutableQuery, QueryBase, Select},
    Table,
};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;
use uuid::Uuid;

pub const PROFILE: &str = "personal-lexical-v1";
const TABLE: &str = "personal_records";

#[derive(Clone)]
pub struct LanceProjection {
    inner: Arc<Inner>,
}

struct Owner(File);
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

struct Inner {
    table: Table,
    gate: Mutex<()>,
    _owner: Owner,
}

impl LanceProjection {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        tokio::spawn(Self::open_inner(path.as_ref().to_path_buf(), false))
            .await
            .map_err(failure)?
    }

    pub async fn open_fresh(path: impl AsRef<Path>) -> Result<Self> {
        tokio::spawn(Self::open_inner(path.as_ref().to_path_buf(), true))
            .await
            .map_err(failure)?
    }

    async fn open_inner(path: PathBuf, fresh: bool) -> Result<Self> {
        let (path, owner) = tokio::task::spawn_blocking(move || {
            if path.to_string_lossy().contains("://") {
                return Err(Error::Invalid(
                    "personal index requires a local path".into(),
                ));
            }
            std::fs::create_dir_all(&path).map_err(failure)?;
            let path = path.canonicalize().map_err(failure)?;
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path.join(".personal-owner.lock"))
                .map_err(failure)?;
            fs2::FileExt::try_lock_exclusive(&file)
                .map_err(|_| Error::Projection("personal index already has an owner".into()))?;
            Ok((path, Owner(file)))
        })
        .await
        .map_err(failure)??;
        let uri = path
            .to_str()
            .ok_or_else(|| Error::Invalid("index path must be UTF-8".into()))?;
        let connection = lancedb::connect(uri).execute().await.map_err(failure)?;
        let names = connection.table_names().execute().await.map_err(failure)?;
        if names.iter().any(|name| name != TABLE) || (fresh && !names.is_empty()) {
            return Err(Error::Schema(
                "use a dedicated, fresh personal index directory".into(),
            ));
        }
        let table = if names.is_empty() {
            connection
                .create_table(TABLE, rows::reader(RecordBatch::new_empty(rows::schema())))
                .execute()
                .await
                .map_err(failure)?
        } else {
            connection
                .open_table(TABLE)
                .execute()
                .await
                .map_err(failure)?
        };
        if table.schema().await.map_err(failure)?.as_ref() != rows::schema().as_ref() {
            return Err(Error::Schema("incompatible personal index schema/profile/dimensions; rebuild into a fresh directory".into()));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                table,
                gate: Mutex::new(()),
                _owner: owner,
            }),
        })
    }
}

impl Inner {
    async fn apply(&self, record: Record) -> Result<()> {
        let _guard = self.gate.lock().await;
        let batches = self
            .table
            .query()
            .only_if(format!("id = '{}'", record.id))
            .select(Select::Columns(vec!["revision".into(), "payload".into()]))
            .execute()
            .await
            .map_err(failure)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(failure)?;
        let payload = serde_json::to_string(&record).map_err(failure)?;
        let mut already_applied = false;
        for batch in &batches {
            let revisions = rows::column::<UInt64Array>(batch, "revision")?;
            let payloads = rows::column::<StringArray>(batch, "payload")?;
            for row in 0..batch.num_rows() {
                if revisions.value(row) > record.revision {
                    return Ok(());
                }
                if revisions.value(row) == record.revision {
                    if payloads.value(row) != payload {
                        return Err(Error::Projection(
                            "same revision has conflicting original".into(),
                        ));
                    }
                    already_applied = true;
                }
            }
        }
        if !already_applied {
            let mut merge = self.table.merge_insert(&["id"]);
            merge
                .when_matched_update_all(None)
                .when_not_matched_insert_all();
            merge
                .execute(rows::reader(rows::batch(&record)?))
                .await
                .map_err(failure)?;
        }
        // A retry must repair indexing if a previous call committed its row then failed here.
        self.table
            .create_index(
                &["content"],
                Index::FTS(
                    FtsIndexBuilder::default()
                        .stem(false)
                        .remove_stop_words(false)
                        .max_token_length(None),
                ),
            )
            .execute()
            .await
            .map_err(failure)?;
        Ok(())
    }

    async fn search(&self, request: SearchRequest) -> Result<Vec<SearchHit>> {
        let _guard = self.gate.lock().await;
        if self.table.count_rows(None).await.map_err(failure)? == 0 {
            return Ok(vec![]);
        }
        let text: String = request
            .text
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    ' '
                }
            })
            .collect();
        if text.trim().is_empty() {
            return Ok(vec![]);
        }
        let filter = request.kind.map_or_else(
            || "deleted = false".into(),
            |kind| format!("deleted = false AND kind = '{}'", rows::kind(kind)),
        );
        let batches = self
            .table
            .query()
            .only_if(filter)
            .limit(request.limit)
            .select(Select::Columns(vec!["id".into(), "revision".into()]))
            .full_text_search(FullTextSearchQuery::new(text))
            .execute()
            .await
            .map_err(failure)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(failure)?;
        let mut hits = Vec::new();
        for batch in &batches {
            let ids = rows::column::<StringArray>(batch, "id")?;
            let revisions = rows::column::<UInt64Array>(batch, "revision")?;
            let scores = rows::column::<Float32Array>(batch, "_score")?;
            for row in 0..batch.num_rows() {
                hits.push(SearchHit {
                    record_id: Uuid::parse_str(ids.value(row)).map_err(failure)?,
                    revision: revisions.value(row),
                    score: f64::from(scores.value(row)),
                });
            }
        }
        Ok(hits)
    }
}

#[async_trait]
impl Projection for LanceProjection {
    async fn apply(&self, record: Record) -> Result<()> {
        record.validate()?;
        let inner = Arc::clone(&self.inner);
        // Keep the gate and ownership alive through commit even if the caller is cancelled.
        tokio::spawn(async move { inner.apply(record).await })
            .await
            .map_err(failure)?
    }

    async fn search(&self, request: SearchRequest) -> Result<Vec<SearchHit>> {
        request.validate()?;
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move { inner.search(request).await })
            .await
            .map_err(failure)?
    }
}

fn failure(error: impl std::fmt::Display) -> Error {
    Error::Projection(error.to_string())
}
