pub use agentix_store::RepoSummary;
use agentix_store::{checked_limit, literal, project_filter, Store};
use anyhow::{ensure, Result};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Deserialize)]
pub struct ChunkRow {
    pub repo_path: String,
    pub file_path: String,
    pub start_line: Option<i32>,
    pub end_line: Option<i32>,
    pub content: String,
    pub language: Option<String>,
    pub symbol_kind: Option<String>,
    pub rrf_score: Option<f64>,
}
fn code_filter(
    language: Option<&str>,
    kind: Option<&str>,
    repo: Option<&str>,
    projects: &[String],
) -> String {
    let mut filters = vec![project_filter(projects)];
    for (column, value, operator) in [
        ("language", language, "="),
        ("symbol_kind", kind, "="),
        ("repo_path", repo, "LIKE"),
    ] {
        if let Some(value) = value {
            filters.push(format!("{column} {operator} {}", literal(value)));
        }
    }
    filters.join(" AND ")
}
fn chunks(rows: Vec<(Value, f64)>) -> Result<Vec<ChunkRow>> {
    rows.into_iter()
        .map(|(row, score)| {
            let mut row: ChunkRow = serde_json::from_value(row)?;
            row.rrf_score = Some(score);
            Ok(row)
        })
        .collect()
}
#[allow(clippy::too_many_arguments)]
pub async fn hybrid_search(
    pool: &Store,
    text: &str,
    vector: &[f32],
    limit: i32,
    language: Option<&str>,
    kind: Option<&str>,
    repo: Option<&str>,
    projects: &[String],
) -> Result<Vec<ChunkRow>> {
    chunks(
        pool.hybrid(
            "code_chunks",
            text,
            vector,
            &code_filter(language, kind, repo, projects),
            limit,
        )
        .await?,
    )
}
pub async fn bm25_search(
    pool: &Store,
    text: &str,
    limit: i32,
    language: Option<&str>,
    kind: Option<&str>,
    repo: Option<&str>,
    projects: &[String],
) -> Result<Vec<ChunkRow>> {
    chunks(
        pool.keyword(
            "code_chunks",
            text,
            &code_filter(language, kind, repo, projects),
            limit,
        )
        .await?,
    )
}
pub async fn list_repos(pool: &Store, projects: &[String]) -> Result<Vec<RepoSummary>> {
    pool.repositories(projects).await
}
pub async fn get_file_chunks(
    pool: &Store,
    repo: &str,
    file: &str,
    projects: &[String],
) -> Result<Vec<ChunkRow>> {
    let filter = format!(
        "{} AND ({})",
        agentix_store::file_filter(repo, file),
        project_filter(projects)
    );
    let mut rows = pool.rows("code_chunks", &filter).await?;
    rows.sort_by_key(|r| r["chunk_index"].as_i64().unwrap_or_default());
    rows.into_iter()
        .map(|r| Ok(serde_json::from_value(r)?))
        .collect()
}
#[derive(Clone, Deserialize)]
pub struct DocRow {
    pub source_path: String,
    pub doc_kind: Option<String>,
    pub title: Option<String>,
    pub content: String,
    pub repo_path: String,
    #[serde(default)]
    pub rrf_score: f64,
}
pub async fn search_docs_hybrid(
    pool: &Store,
    text: &str,
    vector: &[f32],
    limit: i32,
    kind: Option<&str>,
    projects: &[String],
) -> Result<Vec<DocRow>> {
    let mut filter = project_filter(projects);
    if let Some(kind) = kind {
        filter.push_str(&format!(" AND doc_kind = {}", literal(kind)));
    }
    pool.hybrid("documents", text, vector, &filter, limit)
        .await?
        .into_iter()
        .map(|(row, score)| {
            let mut row: DocRow = serde_json::from_value(row)?;
            row.rrf_score = score;
            Ok(row)
        })
        .collect()
}
#[derive(Clone, Deserialize)]
pub struct GithubRow {
    #[serde(default)]
    pub entity_type: String,
    pub repo: String,
    pub number: i32,
    pub state: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub labels: Option<String>,
    pub content: String,
    pub created_at: Option<String>,
    #[serde(default)]
    pub rrf_score: f64,
}
pub async fn search_github_hybrid(
    pool: &Store,
    text: &str,
    vector: &[f32],
    limit: i32,
    repo: Option<&str>,
    state: Option<&str>,
    entity: Option<&str>,
) -> Result<Vec<GithubRow>> {
    let limit_usize = checked_limit(limit)?;
    ensure!(
        matches!(entity, None | Some("issues" | "prs")),
        "entity_type must be issues or prs"
    );
    let mut filter = "true".to_owned();
    for (column, value) in [("repo_path", repo), ("state", state)] {
        if let Some(value) = value {
            filter.push_str(&format!(" AND {column} = {}", literal(value)));
        }
    }
    let mut results = Vec::new();
    for (table, requested, kind) in [
        ("github_issues", "issues", "issue"),
        ("github_prs", "prs", "pr"),
    ] {
        if entity.is_some_and(|e| e != requested) {
            continue;
        }
        for (row, score) in pool.hybrid(table, text, vector, &filter, limit).await? {
            let mut row: GithubRow = serde_json::from_value(row)?;
            row.entity_type = kind.into();
            row.rrf_score = score;
            results.push(row);
        }
    }
    results.sort_by(|a, b| {
        b.rrf_score.total_cmp(&a.rrf_score).then_with(|| {
            (&a.repo, a.number, &a.entity_type).cmp(&(&b.repo, b.number, &b.entity_type))
        })
    });
    results.truncate(limit_usize);
    Ok(results)
}
