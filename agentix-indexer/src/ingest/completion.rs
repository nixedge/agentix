use agentix_store::Store;
use anyhow::Result;
use serde_json::json;

pub async fn package_complete(store: &Store, repo: &str) -> Result<bool> {
    Ok(store
        .metadata("package_ingest", repo)
        .await?
        .is_some_and(|state| state["complete"] == true))
}

pub async fn mark_package(store: &Store, repo: &str, complete: bool) -> Result<()> {
    store
        .set_metadata(
            "package_ingest",
            repo,
            json!({"repo_path":repo,"complete":complete}),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn rows_and_registry_do_not_imply_package_completion() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), "fixture", 4).await?;
        let repo = "crates.io::partial-1.0.0";
        store
            .set_metadata("repo_index", repo, json!({"repo_path":repo}))
            .await?;
        store.put("code_chunks", vec![agentix_store::Record {
            row:json!({"repo_path":repo,"file_path":"lib.rs","chunk_index":0,"content":"partial"}),
            embedding:Some(vec![1.0,0.2,0.3,0.4]),
        }]).await?;
        assert!(!package_complete(&store, repo).await?);
        mark_package(&store, repo, true).await?;
        assert!(package_complete(&store, repo).await?);
        mark_package(&store, repo, false).await?;
        assert!(!package_complete(&store, repo).await?);
        super::super::prune::prune_repo(&store, repo, false).await?;
        assert!(store.metadata("package_ingest", repo).await?.is_none());
        Ok(())
    }
}
