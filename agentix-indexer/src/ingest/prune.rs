use agentix_store::{literal, repo_filter, Store};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
pub async fn prune_repo(pool: &Store, repo: &str, dry_run: bool) -> Result<()> {
    let filter = repo_filter(repo);
    let code = pool.count("code_chunks", &filter).await?;
    let docs = pool.count("documents", &filter).await?;
    if dry_run {
        eprintln!("[dry-run] {repo}: {code} chunks, {docs} docs");
        return Ok(());
    }
    pool.delete("code_chunks", &filter).await?;
    pool.delete("documents", &filter).await?;
    pool.delete(
        "metadata",
        &format!(
            "namespace IN ('repo_index', 'git_ingest', 'package_ingest') AND key = {}",
            literal(repo)
        ),
    )
    .await?;
    eprintln!("Pruned {repo}: {code} chunks, {docs} docs.");
    Ok(())
}
pub async fn prune_dirty(pool: &Store, dry_run: bool) -> Result<()> {
    for row in pool.metadata_rows("repo_index").await? {
        if row["source_kind"] == "local" {
            if let Some(repo) = row["repo_path"].as_str() {
                prune_repo(pool, repo, dry_run).await?;
            }
        }
    }
    Ok(())
}
pub async fn prune_old_versions(pool: &Store, package: Option<&str>, dry_run: bool) -> Result<()> {
    for repo in old_version_repos(pool, package).await? {
        prune_repo(pool, &repo, dry_run).await?;
    }
    Ok(())
}
pub async fn old_version_repos(pool: &Store, package: Option<&str>) -> Result<Vec<String>> {
    let rows = pool.metadata_rows("repo_index").await?;
    let mut latest: BTreeMap<(String, String), chrono::DateTime<chrono::Utc>> = BTreeMap::new();
    let mut repos = Vec::new();
    for row in &rows {
        let Some(name) = row["package_name"].as_str() else {
            continue;
        };
        if package.is_some_and(|p| p != name) {
            continue;
        }
        let key = (
            row["source_kind"].as_str().unwrap_or_default().into(),
            name.into(),
        );
        let timestamp = indexed_at(row)?;
        let entry = latest.entry(key).or_insert(timestamp);
        if timestamp > *entry {
            *entry = timestamp;
        }
    }
    for row in &rows {
        let Some(name) = row["package_name"].as_str() else {
            continue;
        };
        let key = (
            row["source_kind"].as_str().unwrap_or_default().to_owned(),
            name.to_owned(),
        );
        if let Some(timestamp) = latest.get(&key) {
            if indexed_at(row)? < *timestamp {
                let repo = row["repo_path"]
                    .as_str()
                    .context("repository metadata lacks repo_path")?;
                repos.push(repo.to_owned());
            }
        }
    }
    Ok(repos)
}
fn indexed_at(row: &serde_json::Value) -> Result<chrono::DateTime<chrono::Utc>> {
    Ok(chrono::DateTime::parse_from_rfc3339(
        row["indexed_at"]
            .as_str()
            .context("repository metadata lacks indexed_at")?,
    )?
    .with_timezone(&chrono::Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn old_versions_compare_instants_across_imported_timezones() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let store = Store::open(tmp.path(), "fixture", 4).await?;
        for (repo, time) in [
            ("crates.io::pkg-1", "2026-10-28T12:00:00+10:00"),
            ("crates.io::pkg-2", "2026-10-28T04:00:00Z"),
        ] {
            store.set_metadata("repo_index",repo,json!({"repo_path":repo,"source_kind":"crates.io","package_name":"pkg","indexed_at":time})).await?;
            store.put("code_chunks",vec![agentix_store::Record{row:json!({"repo_path":repo,"file_path":"lib.rs","chunk_index":0,"content":"packagetoken"}),embedding:Some(vec![1.0;4])}]).await?;
        }
        prune_old_versions(&store, None, true).await?;
        assert_eq!(store.count("code_chunks", "true").await?, 2);
        prune_old_versions(&store, None, false).await?;
        assert_eq!(
            store
                .count("code_chunks", &repo_filter("crates.io::pkg-1"))
                .await?,
            0
        );
        assert_eq!(
            store
                .count("code_chunks", &repo_filter("crates.io::pkg-2"))
                .await?,
            1
        );
        Ok(())
    }
}
