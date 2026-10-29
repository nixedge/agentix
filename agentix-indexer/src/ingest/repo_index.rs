use agentix_store::Store;
use anyhow::Result;
use serde_json::json;
pub struct RepoMeta<'a> {
    pub source_kind: &'a str,
    pub package_name: Option<&'a str>,
    pub version: Option<&'a str>,
    pub git_url: Option<&'a str>,
    pub git_rev: Option<&'a str>,
    pub project: Option<&'a str>,
}
pub async fn upsert_repo(pool: &Store, repo_path: &str, meta: &RepoMeta<'_>) -> Result<()> {
    pool.update_metadata("repo_index",repo_path,|previous|{
        let mut projects=previous.as_ref().and_then(|v|v["project"].as_array()).cloned().unwrap_or_default();
        if let Some(project)=meta.project{if !projects.iter().any(|p|p==project){projects.push(json!(project));}}
        Ok(json!({"repo_path":repo_path,"source_kind":meta.source_kind,"package_name":meta.package_name,"version":meta.version,
            "git_url":meta.git_url,"git_rev":meta.git_rev,"project":projects,"indexed_at":chrono::Utc::now().to_rfc3339()}))
    }).await
}
