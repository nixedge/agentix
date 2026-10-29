//! Persistent local search storage shared by ingestion and retrieval.
pub mod server;
pub mod transport;
use anyhow::{ensure, Context, Result};
use arrow_array::{
    builder::{FixedSizeListBuilder, Float32Builder, ListBuilder, StringBuilder},
    Array, ArrayRef, FixedSizeListArray, Float32Array, Int32Array, RecordBatch,
    RecordBatchIterator, RecordBatchReader, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::{
    index::{
        scalar::{FtsIndexBuilder, FullTextSearchQuery},
        vector::IvfHnswFlatIndexBuilder,
        Index,
    },
    query::{ExecutableQuery, QueryBase, Select},
    Connection, DistanceType, Table,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, OwnedMutexGuard};
use transport::{Operation, Remote};

pub const COLLECTIONS: &[&str] = &[
    "code_chunks",
    "documents",
    "github_issues",
    "github_prs",
    "github_issue_comments",
    "github_pr_comments",
];
const VERSION: &str = "1";
const CANDIDATES: usize = 60;

pub fn embedding_model() -> String {
    std::env::var("EMBED_MODEL")
        .unwrap_or_else(|_| "hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0".into())
}
pub fn configured_projects() -> Vec<String> {
    std::env::var("PROJECTS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}
pub fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
pub fn project_filter(projects: &[String]) -> String {
    if projects.is_empty() {
        return "true".into();
    }
    format!(
        "({})",
        projects
            .iter()
            .map(|p| format!("array_has(projects, {})", literal(p)))
            .collect::<Vec<_>>()
            .join(" OR ")
    )
}
pub fn file_filter(repo: &str, file: &str) -> String {
    format!(
        "repo_path = {} AND file_path = {}",
        literal(repo),
        literal(file)
    )
}
pub fn repo_filter(repo: &str) -> String {
    format!("repo_path = {}", literal(repo))
}
pub fn checked_limit(limit: i32) -> Result<usize> {
    ensure!(
        (1..=100).contains(&limit),
        "limit must be between 1 and 100"
    );
    Ok(limit as usize)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub row: Value,
    pub embedding: Option<Vec<f32>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoSummary {
    pub repo_path: String,
    pub chunks: i64,
    pub files: i64,
    pub last_indexed: String,
}
impl Record {
    pub fn key(&self, collection: &str) -> Result<String> {
        record_key(collection, &self.row)
    }
}
fn string<'a>(row: &'a Value, field: &str) -> &'a str {
    row[field].as_str().unwrap_or("")
}
pub fn record_key(collection: &str, row: &Value) -> Result<String> {
    ensure!(
        COLLECTIONS.contains(&collection),
        "unknown collection {collection}"
    );
    let key = match collection {
        "code_chunks" | "documents" => {
            let file = if collection == "documents" {
                "source_path"
            } else {
                "file_path"
            };
            ensure!(
                !string(row, "repo_path").is_empty() && !string(row, file).is_empty(),
                "record lacks repository/file path"
            );
            json!([
                row["repo_path"],
                row[file],
                row["chunk_index"].as_i64().context("missing chunk_index")?
            ])
        }
        "github_issues" | "github_prs" => {
            ensure!(!string(row, "repo").is_empty(), "record lacks repo");
            json!([
                row["repo"],
                row["number"].as_i64().context("missing number")?
            ])
        }
        "github_pr_comments" => json!([
            row["repo"],
            row["comment_type"],
            row["comment_id"].as_i64().context("missing comment_id")?
        ]),
        _ => json!([
            row["repo"],
            row["comment_id"].as_i64().context("missing comment_id")?
        ]),
    };
    Ok(key.to_string())
}

#[derive(Clone)]
pub struct Store {
    connection: Option<Connection>,
    remote: Option<Remote>,
    path: PathBuf,
    model: String,
    dimensions: usize,
    writer: Arc<Mutex<()>>,
    nprobes: usize,
    ef: usize,
}
struct WriteGuard {
    file: File,
    _mutex: OwnedMutexGuard<()>,
}
impl Drop for WriteGuard {
    fn drop(&mut self) {
        // A concurrent fork can briefly inherit this descriptor before exec. End the
        // lock explicitly rather than waiting for the last inherited descriptor to close.
        let _ = fs2::FileExt::unlock(&self.file);
    }
}
impl Store {
    pub async fn from_env() -> Result<Self> {
        if let Some(url) = std::env::var_os("AGENTIX_STORE_URL") {
            let url = url.to_str().context("AGENTIX_STORE_URL must be UTF-8")?;
            ensure!(
                std::env::var_os("AGENTIX_LANCE_PATH").is_none(),
                "set AGENTIX_STORE_URL or AGENTIX_LANCE_PATH, not both"
            );
            let dimensions = std::env::var("EMBED_DIMENSIONS")
                .unwrap_or_else(|_| "1536".into())
                .parse()
                .context("invalid EMBED_DIMENSIONS")?;
            return Self::connect(
                url,
                transport::credential()?,
                &embedding_model(),
                dimensions,
            )
            .await;
        }
        let path = match std::env::var_os("AGENTIX_LANCE_PATH") {
            Some(path) => PathBuf::from(path),
            None => {
                ensure!(std::env::var_os("PG_DSN").is_none(), "PG_DSN is no longer supported; set AGENTIX_LANCE_PATH and import or reindex the existing data");
                let root = std::env::var_os("XDG_DATA_HOME")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
                    })
                    .context("set AGENTIX_LANCE_PATH when HOME is unavailable")?;
                root.join("agentix/index")
            }
        };
        let dimensions = std::env::var("EMBED_DIMENSIONS")
            .unwrap_or_else(|_| "1536".into())
            .parse()
            .context("invalid EMBED_DIMENSIONS")?;
        let mut store = Self::open(&path, &embedding_model(), dimensions).await?;
        store.nprobes = std::env::var("LANCE_NPROBES")
            .unwrap_or_else(|_| "20".into())
            .parse()
            .context("invalid LANCE_NPROBES")?;
        store.ef = std::env::var("LANCE_EF")
            .unwrap_or_else(|_| "100".into())
            .parse()
            .context("invalid LANCE_EF")?;
        ensure!(
            (1..=64).contains(&store.nprobes) && (100..=400).contains(&store.ef),
            "LANCE_NPROBES must be 1..64 and LANCE_EF 100..400"
        );
        Ok(store)
    }
    pub async fn open(path: &Path, model: &str, dimensions: usize) -> Result<Self> {
        ensure!(
            !model.is_empty() && (1..=65536).contains(&dimensions),
            "invalid embedding model/dimensions"
        );
        ensure!(
            !path.to_string_lossy().contains("://"),
            "index must be a local filesystem path"
        );
        std::fs::create_dir_all(path)?;
        let path = path.canonicalize()?;
        let connection = lancedb::connect(path.to_str().context("index path must be UTF-8")?)
            .read_consistency_interval(Duration::ZERO)
            .execute()
            .await?;
        let store = Self {
            connection: Some(connection),
            remote: None,
            path,
            model: model.into(),
            dimensions,
            writer: Arc::new(Mutex::new(())),
            nprobes: 20,
            ef: 100,
        };
        for name in store.local_connection()?.table_names().execute().await? {
            ensure!(
                name == "metadata" || COLLECTIONS.contains(&name.as_str()),
                "unrecognized table {name}; use a dedicated Agentix index directory"
            );
            store.table(&name).await?;
        }
        Ok(store)
    }
    pub async fn connect(
        url: &str,
        token: Option<String>,
        model: &str,
        dimensions: usize,
    ) -> Result<Self> {
        ensure!(
            !model.is_empty() && (1..=65536).contains(&dimensions),
            "invalid embedding model/dimensions"
        );
        let remote = Remote::connect(url, token, model, dimensions).await?;
        Ok(Self {
            connection: None,
            remote: Some(remote),
            path: PathBuf::new(),
            model: model.into(),
            dimensions,
            writer: Arc::new(Mutex::new(())),
            nprobes: 20,
            ef: 100,
        })
    }
    fn local_connection(&self) -> Result<&Connection> {
        self.connection
            .as_ref()
            .context("remote store has no local connection")
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }
    async fn lock(&self) -> Result<WriteGuard> {
        let guard = self.writer.clone().lock_owned().await;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.join(".writer.lock"))?;
        fs2::FileExt::try_lock_exclusive(&file)
            .context("index is busy: another process is writing; retry after it completes")?;
        Ok(WriteGuard {
            file,
            _mutex: guard,
        })
    }
    async fn table(&self, name: &str) -> Result<Option<Table>> {
        ensure!(
            name == "metadata" || COLLECTIONS.contains(&name),
            "unknown collection {name}"
        );
        let table = match self.local_connection()?.open_table(name).execute().await {
            Ok(t) => t,
            Err(lancedb::Error::TableNotFound { .. }) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let schema = table.schema().await?;
        ensure!(
            schema.metadata().get("agentix.schema").map(String::as_str) == Some(VERSION),
            "incompatible Agentix schema in {name}"
        );
        if name != "metadata" {
            ensure!(
                schema.metadata().get("agentix.model") == Some(&self.model),
                "embedding model mismatch in {name}"
            );
            ensure!(
                schema.metadata().get("agentix.dimensions") == Some(&self.dimensions.to_string()),
                "embedding dimensions mismatch in {name}"
            );
        }
        Ok(Some(table))
    }
    fn validate_vector(&self, vector: &[f32]) -> Result<()> {
        ensure!(
            vector.len() == self.dimensions
                && vector.iter().all(|x| x.is_finite())
                && vector.iter().any(|x| *x != 0.0),
            "invalid embedding: expected {} finite dimensions and nonzero norm",
            self.dimensions
        );
        Ok(())
    }
    pub fn validate_record(&self, name: &str, record: &Record) -> Result<()> {
        record.key(name)?;
        ensure!(
            record.row["content"].is_string(),
            "record content must be a string"
        );
        if let Some(vector) = &record.embedding {
            self.validate_vector(vector)?;
        }
        if let Some(projects) = record.row.get("project").filter(|p| !p.is_null()) {
            ensure!(
                projects
                    .as_array()
                    .is_some_and(|tags| tags.iter().all(Value::is_string)),
                "project must be an array of strings or null"
            );
        }
        Ok(())
    }
    fn batch(&self, name: &str, records: &[Record]) -> Result<RecordBatch> {
        let keys = records
            .iter()
            .map(|r| r.key(name))
            .collect::<Result<Vec<_>>>()?;
        let payloads = records
            .iter()
            .map(|r| r.row.to_string())
            .collect::<Vec<_>>();
        let text_column = |field: &str| -> ArrayRef {
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|r| r.row[field].as_str())
                    .collect::<Vec<_>>(),
            ))
        };
        let repos = records
            .iter()
            .map(|r| {
                r.row["repo_path"]
                    .as_str()
                    .or_else(|| r.row["repo"].as_str())
            })
            .collect::<Vec<_>>();
        let files = records
            .iter()
            .map(|r| {
                r.row["file_path"]
                    .as_str()
                    .or_else(|| r.row["source_path"].as_str())
            })
            .collect::<Vec<_>>();
        let mut projects = ListBuilder::new(StringBuilder::new());
        let mut vectors = FixedSizeListBuilder::new(Float32Builder::new(), self.dimensions as i32);
        for record in records {
            for project in record.row["project"].as_array().into_iter().flatten() {
                projects
                    .values()
                    .append_value(project.as_str().context("project tags must be strings")?);
            }
            projects.append(true);
            if let Some(vector) = &record.embedding {
                self.validate_vector(vector)?;
                vectors.values().append_slice(vector);
                vectors.append(true);
            } else {
                vectors.values().append_nulls(self.dimensions);
                vectors.append(false);
            }
        }
        let columns: Vec<(&str, ArrayRef)> = vec![
            ("key", Arc::new(StringArray::from(keys))),
            ("payload", Arc::new(StringArray::from(payloads))),
            ("repo_path", Arc::new(StringArray::from(repos))),
            ("file_path", Arc::new(StringArray::from(files))),
            (
                "chunk_index",
                Arc::new(Int32Array::from(
                    records
                        .iter()
                        .map(|r| r.row["chunk_index"].as_i64().map(|n| n as i32))
                        .collect::<Vec<_>>(),
                )),
            ),
            ("content", text_column("content")),
            ("language", text_column("language")),
            ("symbol_kind", text_column("symbol_kind")),
            ("doc_kind", text_column("doc_kind")),
            ("state", text_column("state")),
            ("projects", Arc::new(projects.finish())),
            ("embedding", Arc::new(vectors.finish())),
        ];
        let metadata = HashMap::from([
            ("agentix.schema".into(), VERSION.into()),
            ("agentix.model".into(), self.model.clone()),
            ("agentix.dimensions".into(), self.dimensions.to_string()),
        ]);
        let schema = Schema::new_with_metadata(
            columns
                .iter()
                .map(|(n, c)| Field::new(*n, c.data_type().clone(), true))
                .collect::<Vec<_>>(),
            metadata,
        );
        Ok(RecordBatch::try_new(
            Arc::new(schema),
            columns.into_iter().map(|(_, c)| c).collect(),
        )?)
    }
    async fn read(&self, name: &str, filter: &str, embeddings: bool) -> Result<Vec<Record>> {
        let Some(table) = self.table(name).await? else {
            return Ok(vec![]);
        };
        let columns = if embeddings {
            vec!["payload".into(), "embedding".into()]
        } else {
            vec!["payload".into()]
        };
        let batches = table
            .query()
            .only_if(filter)
            .select(Select::Columns(columns))
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        decode(&batches)
    }
    pub async fn rows(&self, name: &str, filter: &str) -> Result<Vec<Value>> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::Rows {
                    name: name.into(),
                    filter: filter.into(),
                })
                .await;
        }
        Ok(self
            .read(name, filter, false)
            .await?
            .into_iter()
            .map(|r| r.row)
            .collect())
    }
    pub async fn repositories(&self, projects: &[String]) -> Result<Vec<RepoSummary>> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::Repositories {
                    projects: projects.into(),
                })
                .await;
        }
        let rows = self.rows("code_chunks", &project_filter(projects)).await?;
        let mut repos: BTreeMap<String, (usize, BTreeSet<String>, String)> = BTreeMap::new();
        for row in rows {
            let entry = repos
                .entry(string(&row, "repo_path").to_owned())
                .or_default();
            entry.0 += 1;
            entry.1.insert(string(&row, "file_path").to_owned());
            let indexed = string(&row, "indexed_at");
            if indexed > entry.2.as_str() {
                entry.2 = indexed.into();
            }
        }
        let mut result = repos
            .into_iter()
            .map(|(repo_path, (chunks, files, last_indexed))| RepoSummary {
                repo_path,
                chunks: chunks as i64,
                files: files.len() as i64,
                last_indexed,
            })
            .collect::<Vec<_>>();
        result.sort_by(|a, b| {
            b.last_indexed
                .cmp(&a.last_indexed)
                .then_with(|| a.repo_path.cmp(&b.repo_path))
        });
        Ok(result)
    }
    pub async fn count(&self, name: &str, filter: &str) -> Result<usize> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::Count {
                    name: name.into(),
                    filter: filter.into(),
                })
                .await;
        }
        let Some(table) = self.table(name).await? else {
            return Ok(0);
        };
        Ok(table.count_rows(Some(filter.into())).await?)
    }
    pub async fn put(&self, name: &str, records: Vec<Record>) -> Result<()> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::Put {
                    name: name.into(),
                    records,
                })
                .await;
        }
        if records.is_empty() {
            return Ok(());
        }
        let _guard = self.lock().await?;
        self.put_locked(name, records, None).await
    }
    async fn put_locked(
        &self,
        name: &str,
        mut records: Vec<Record>,
        replace: Option<&str>,
    ) -> Result<()> {
        let mut unique = BTreeSet::new();
        for r in &records {
            self.validate_record(name, r)?;
            ensure!(unique.insert(r.key(name)?), "duplicate record key in batch");
            if let Some(v) = &r.embedding {
                self.validate_vector(v)?;
            }
        }
        let table = self.table(name).await?;
        if let Some(table) = &table {
            let filter = replace.map(str::to_owned).unwrap_or_else(|| {
                format!(
                    "key IN ({})",
                    unique
                        .iter()
                        .map(|k| literal(k))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            });
            let old = self.rows(name, &filter).await?;
            let mut old_projects = BTreeSet::new();
            let keyed = old
                .iter()
                .map(|row| Ok((record_key(name, row)?, row)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            if replace.is_some() {
                for row in &old {
                    for p in row["project"].as_array().into_iter().flatten() {
                        if let Some(p) = p.as_str() {
                            old_projects.insert(p.to_owned());
                        }
                    }
                }
            }
            for record in &mut records {
                let mut tags = old_projects.clone();
                if let Some(row) = keyed.get(&record.key(name)?) {
                    for p in row["project"].as_array().into_iter().flatten() {
                        if let Some(p) = p.as_str() {
                            tags.insert(p.to_owned());
                        }
                    }
                }
                for p in record.row["project"].as_array().into_iter().flatten() {
                    tags.insert(p.as_str().context("invalid project tag")?.to_owned());
                }
                if !tags.is_empty() || record.row.get("project").is_some_and(Value::is_array) {
                    record.row["project"] = json!(tags);
                }
            }
            let batch = self.batch(name, &records)?;
            let mut merge = table.merge_insert(&["key"]);
            merge
                .when_matched_update_all(None)
                .when_not_matched_insert_all();
            if let Some(filter) = replace {
                merge.when_not_matched_by_source_delete(Some(filter.into()));
            }
            merge.execute(reader(batch)).await?;
        } else {
            let batch = self.batch(name, &records)?;
            let table = self
                .local_connection()?
                .create_table(name, reader(batch))
                .execute()
                .await?;
            Self::fts(&table).await?;
        }
        Ok(())
    }
    pub async fn replace_file(
        &self,
        name: &str,
        repo: &str,
        file: &str,
        records: Vec<Record>,
    ) -> Result<()> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::ReplaceFile {
                    name: name.into(),
                    repo: repo.into(),
                    file: file.into(),
                    records,
                })
                .await;
        }
        ensure!(
            matches!(name, "code_chunks" | "documents"),
            "file replacement requires code or documents"
        );
        for r in &records {
            ensure!(
                string(&r.row, "repo_path") == repo
                    && string(
                        &r.row,
                        if name == "documents" {
                            "source_path"
                        } else {
                            "file_path"
                        }
                    ) == file,
                "replacement batch contains another file"
            );
        }
        let _guard = self.lock().await?;
        let filter = file_filter(repo, file);
        if records.is_empty() {
            self.delete_locked(name, &filter).await?;
        } else {
            self.put_locked(name, records, Some(&filter)).await?;
        }
        Ok(())
    }
    pub async fn retag_file(
        &self,
        name: &str,
        repo: &str,
        file: &str,
        project: Option<&str>,
        mtime: Option<i64>,
    ) -> Result<()> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::RetagFile {
                    name: name.into(),
                    repo: repo.into(),
                    file: file.into(),
                    project: project.map(str::to_owned),
                    mtime,
                })
                .await;
        }
        let _guard = self.lock().await?;
        let mut records = self.read(name, &file_filter(repo, file), true).await?;
        for record in &mut records {
            if let Some(p) = project {
                let mut tags = record.row["project"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if !tags.iter().any(|v| v == p) {
                    tags.push(json!(p));
                }
                record.row["project"] = json!(tags);
            }
            record.row["file_mtime"] = json!(mtime);
        }
        if !records.is_empty() {
            self.put_locked(name, records, None).await?;
        }
        Ok(())
    }
    pub async fn tag_repository(&self, repo: &str, project: Option<&str>) -> Result<()> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::TagRepository {
                    repo: repo.into(),
                    project: project.map(str::to_owned),
                })
                .await;
        }
        let Some(project) = project else {
            return Ok(());
        };
        let _guard = self.lock().await?;
        for name in ["code_chunks", "documents"] {
            let records = self.read(name, &repo_filter(repo), true).await?;
            let mut changed = Vec::new();
            for mut record in records {
                let mut tags = record.row["project"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if !tags.iter().any(|p| p == project) {
                    tags.push(json!(project));
                    record.row["project"] = json!(tags);
                    changed.push(record);
                }
            }
            for batch in changed.chunks(128) {
                self.put_locked(name, batch.to_vec(), None).await?;
            }
        }
        if let Some(mut row) = self.metadata("repo_index", repo).await? {
            let mut tags = row["project"].as_array().cloned().unwrap_or_default();
            if !tags.iter().any(|tag| tag == project) {
                tags.push(json!(project));
                row["project"] = json!(tags);
                self.set_metadata_locked("repo_index", repo, row).await?;
            }
        }
        Ok(())
    }
    pub async fn delete(&self, name: &str, filter: &str) -> Result<usize> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::Delete {
                    name: name.into(),
                    filter: filter.into(),
                })
                .await;
        }
        let _guard = self.lock().await?;
        self.delete_locked(name, filter).await
    }
    async fn delete_locked(&self, name: &str, filter: &str) -> Result<usize> {
        let Some(table) = self.table(name).await? else {
            return Ok(0);
        };
        let count = table.count_rows(Some(filter.into())).await?;
        table.delete(filter).await?;
        Ok(count)
    }
    async fn fts(table: &Table) -> Result<()> {
        table
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
            .await?;
        Ok(())
    }
    pub async fn maintain(&self, name: &str) -> Result<()> {
        if let Some(remote) = &self.remote {
            return remote.call(Operation::Maintain { name: name.into() }).await;
        }
        let _guard = self.lock().await?;
        let Some(table) = self.table(name).await? else {
            return Ok(());
        };
        let count = table
            .count_rows(Some("embedding IS NOT NULL".into()))
            .await?;
        Self::fts(&table).await?;
        if count >= 64 {
            table
                .create_index(
                    &["embedding"],
                    Index::IvfHnswFlat(
                        IvfHnswFlatIndexBuilder::default()
                            .distance_type(DistanceType::Cosine)
                            .num_partitions(64)
                            .num_edges(20)
                            .ef_construction(400),
                    ),
                )
                .execute()
                .await?;
        }
        for column in [
            "key",
            "repo_path",
            "file_path",
            "language",
            "symbol_kind",
            "doc_kind",
            "state",
        ] {
            table.create_index(&[column], Index::Auto).execute().await?;
        }
        table
            .create_index(&["projects"], Index::LabelList(Default::default()))
            .execute()
            .await?;
        Ok(())
    }
    pub async fn maintain_if_needed(&self, name: &str) -> Result<()> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::MaintainIfNeeded { name: name.into() })
                .await;
        }
        let Some(table) = self.table(name).await? else {
            return Ok(());
        };
        if !table
            .list_indices()
            .await?
            .iter()
            .any(|i| i.columns == ["embedding"])
            && table
                .count_rows(Some("embedding IS NOT NULL".into()))
                .await?
                >= 64
        {
            self.maintain(name).await?;
        }
        Ok(())
    }
    async fn candidates(
        &self,
        name: &str,
        text: &str,
        vector: Option<&[f32]>,
        filter: &str,
        limit: usize,
    ) -> Result<Vec<(Record, f64)>> {
        let Some(table) = self.table(name).await? else {
            return Ok(vec![]);
        };
        let base = table
            .query()
            .select(Select::Columns(vec!["payload".into()]));
        let batches = if let Some(vector) = vector {
            self.validate_vector(vector)?;
            base.only_if(format!("({filter}) AND embedding IS NOT NULL"))
                .limit(limit)
                .nearest_to(vector)?
                .distance_type(DistanceType::Cosine)
                .nprobes(self.nprobes)
                .ef(self.ef.max(limit))
                .execute()
                .await?
                .try_collect::<Vec<_>>()
                .await?
        } else {
            let query = text
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '_' {
                        c
                    } else {
                        ' '
                    }
                })
                .collect::<String>();
            if query.trim().is_empty() {
                return Ok(vec![]);
            }
            base.only_if(filter)
                .limit(limit)
                .full_text_search(FullTextSearchQuery::new(query))
                .execute()
                .await?
                .try_collect::<Vec<_>>()
                .await?
        };
        let mut result = Vec::new();
        for batch in &batches {
            let rows = decode(std::slice::from_ref(batch))?;
            let scores = batch
                .column_by_name("_score")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
            for (i, row) in rows.into_iter().enumerate() {
                result.push((row, scores.map(|a| f64::from(a.value(i))).unwrap_or(0.0)));
            }
        }
        Ok(result)
    }
    pub async fn keyword(
        &self,
        name: &str,
        text: &str,
        filter: &str,
        limit: i32,
    ) -> Result<Vec<(Value, f64)>> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::Keyword {
                    name: name.into(),
                    text: text.into(),
                    filter: filter.into(),
                    limit,
                })
                .await;
        }
        let limit = checked_limit(limit)?;
        Ok(self
            .candidates(name, text, None, filter, limit)
            .await?
            .into_iter()
            .map(|(r, s)| (r.row, s))
            .collect())
    }
    pub async fn hybrid(
        &self,
        name: &str,
        text: &str,
        vector: &[f32],
        filter: &str,
        limit: i32,
    ) -> Result<Vec<(Value, f64)>> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::Hybrid {
                    name: name.into(),
                    text: text.into(),
                    vector: vector.into(),
                    filter: filter.into(),
                    limit,
                })
                .await;
        }
        let limit = checked_limit(limit)?;
        self.validate_vector(vector)?;
        let count = CANDIDATES.max(limit);
        let lexical = self.candidates(name, text, None, filter, count).await?;
        let dense = self
            .candidates(name, text, Some(vector), filter, count)
            .await?;
        let mut fused: BTreeMap<String, (Value, f64)> = BTreeMap::new();
        for list in [lexical, dense] {
            for (rank, (record, _)) in list.into_iter().enumerate() {
                let key = record.key(name)?;
                let entry = fused.entry(key).or_insert((record.row, 0.0));
                entry.1 += 1.0 / (60.0 + (rank + 1) as f64);
            }
        }
        let mut fused = fused.into_iter().collect::<Vec<_>>();
        fused.sort_by(|a, b| b.1 .1.total_cmp(&a.1 .1).then_with(|| a.0.cmp(&b.0)));
        Ok(fused.into_iter().take(limit).map(|(_, row)| row).collect())
    }
    pub async fn metadata(&self, namespace: &str, key: &str) -> Result<Option<Value>> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::Metadata {
                    namespace: namespace.into(),
                    key: key.into(),
                })
                .await;
        }
        let filter = format!(
            "namespace = {} AND key = {}",
            literal(namespace),
            literal(key)
        );
        Ok(self.rows("metadata", &filter).await?.into_iter().next())
    }
    pub async fn metadata_rows(&self, namespace: &str) -> Result<Vec<Value>> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::MetadataRows {
                    namespace: namespace.into(),
                })
                .await;
        }
        self.rows("metadata", &format!("namespace = {}", literal(namespace)))
            .await
    }
    pub async fn set_metadata(&self, namespace: &str, key: &str, value: Value) -> Result<()> {
        if let Some(remote) = &self.remote {
            return remote
                .call(Operation::SetMetadata {
                    namespace: namespace.into(),
                    key: key.into(),
                    value,
                })
                .await;
        }
        let _guard = self.lock().await?;
        self.set_metadata_locked(namespace, key, value).await
    }
    pub async fn update_metadata<F>(&self, namespace: &str, key: &str, update: F) -> Result<()>
    where
        F: Fn(Option<Value>) -> Result<Value>,
    {
        if let Some(remote) = &self.remote {
            for attempt in 0..32 {
                let expected = self.metadata(namespace, key).await?;
                let value = update(expected.clone())?;
                let changed: bool = remote
                    .call(Operation::CompareMetadata {
                        namespace: namespace.into(),
                        key: key.into(),
                        expected,
                        value,
                    })
                    .await?;
                if changed {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(2 * (attempt + 1))).await;
            }
            anyhow::bail!("metadata update conflicted repeatedly; retry after contention subsides");
        }
        let _guard = self.lock().await?;
        let previous = self.metadata(namespace, key).await?;
        self.set_metadata_locked(namespace, key, update(previous)?)
            .await
    }
    pub(crate) async fn compare_metadata(
        &self,
        namespace: &str,
        key: &str,
        expected: Option<Value>,
        value: Value,
    ) -> Result<bool> {
        let _guard = self.lock().await?;
        if self.metadata(namespace, key).await? != expected {
            return Ok(false);
        }
        self.set_metadata_locked(namespace, key, value).await?;
        Ok(true)
    }
    async fn set_metadata_locked(&self, namespace: &str, key: &str, value: Value) -> Result<()> {
        let names = ["namespace", "key", "payload"];
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(vec![namespace])),
            Arc::new(StringArray::from(vec![key])),
            Arc::new(StringArray::from(vec![value.to_string()])),
        ];
        let schema = Arc::new(Schema::new_with_metadata(
            names
                .iter()
                .map(|n| Field::new(*n, DataType::Utf8, false))
                .collect::<Vec<_>>(),
            HashMap::from([("agentix.schema".into(), VERSION.into())]),
        ));
        let batch = RecordBatch::try_new(schema, arrays)?;
        if let Some(table) = self.table("metadata").await? {
            let mut merge = table.merge_insert(&["namespace", "key"]);
            merge
                .when_matched_update_all(None)
                .when_not_matched_insert_all();
            merge.execute(reader(batch)).await?;
        } else {
            self.local_connection()?
                .create_table("metadata", reader(batch))
                .execute()
                .await?;
        }
        Ok(())
    }
}
fn reader(batch: RecordBatch) -> Box<dyn RecordBatchReader + Send> {
    Box::new(RecordBatchIterator::new(
        vec![Ok(batch.clone())],
        batch.schema(),
    ))
}
fn decode(batches: &[RecordBatch]) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    for batch in batches {
        let payload = batch
            .column_by_name("payload")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .context("missing stored payload")?;
        let vectors = batch
            .column_by_name("embedding")
            .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>());
        for i in 0..batch.num_rows() {
            let embedding = if let Some(vectors) = vectors {
                if vectors.is_null(i) {
                    None
                } else {
                    let values = vectors.value(i);
                    let values = values
                        .as_any()
                        .downcast_ref::<Float32Array>()
                        .context("invalid stored vector")?;
                    Some(values.values().to_vec())
                }
            } else {
                None
            };
            records.push(Record {
                row: serde_json::from_str(payload.value(i))?,
                embedding,
            });
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn guard_release_ends_lock_even_if_a_fork_retains_a_descriptor() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let store = Store::open(tmp.path(), "fixture", 4).await?;
        let guard = store.lock().await?;
        let inherited = guard.file.try_clone()?;
        drop(guard);
        store
            .set_metadata("sync_state", "test", json!({"watermark":"saved"}))
            .await?;
        drop(inherited);
        Ok(())
    }
}
