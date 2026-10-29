use agentix_indexer::{ingest, maintenance};
use agentix_store::{Store, COLLECTIONS};
use anyhow::{ensure, Context, Result};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

const JOBS: &str = "mcp_jobs";
const HEARTBEAT: u64 = 5;
const STALE: u64 = 30;

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    Haskell,
    Rust,
    Python,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_jobs_before_launch_and_default_to_visible_project() -> Result<()> {
        let projects = vec!["team".into()];
        for invalid in [
            json!({"source":"git","url":"repo","rev":"a","branch":"b"}),
            json!({"source":"optimize","collections":["typo"]}),
            json!({"source":"github","repo":"owner/repo","streams":["typo"]}),
            json!({"source":"package","package":"../escape","version":"1","ecosystem":"rust"}),
            json!({"source":"code","repo_path":"/tmp","project":"other"}),
        ] {
            let mut request: Ingestion = serde_json::from_value(invalid)?;
            assert!(request.prepare(&projects).is_err());
        }
        let mut request: Ingestion =
            serde_json::from_value(json!({"source":"docs","repo_path":"/tmp"}))?;
        request.prepare(&projects)?;
        assert_eq!(serde_json::to_value(request)?["project"], "team");
        assert!(serde_json::from_value::<Ingestion>(
            json!({"source":"package","package":"fixture","version":"1","ecosystem":"unknown"})
        )
        .is_err());
        assert!(serde_json::from_value::<Ingestion>(
            json!({"source":"docs","repo_path":"/tmp","typo":true})
        )
        .is_err());
        let prune: Ingestion =
            serde_json::from_value(json!({"source":"prune","selection":{"target":"local"}}))?;
        assert_eq!(serde_json::to_value(prune)?["dry_run"], true);
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
pub enum PruneTarget {
    Repo { repo_path: String },
    Local,
    OldVersions { package: Option<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ingestion {
    /// Index a directory accessible to this MCP process, including markdown docs.
    Code {
        repo_path: PathBuf,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        patterns: Vec<String>,
        #[serde(default)]
        no_docs: bool,
        project: Option<String>,
    },
    Docs {
        repo_path: PathBuf,
        #[serde(default)]
        force: bool,
        project: Option<String>,
    },
    /// Fetch every package in a Git checkout, not merely a registry release.
    Git {
        url: String,
        rev: Option<String>,
        branch: Option<String>,
        tag: Option<String>,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        no_docs: bool,
        project: Option<String>,
    },
    Package {
        package: String,
        version: String,
        ecosystem: Ecosystem,
        #[serde(default)]
        force: bool,
        project: Option<String>,
    },
    Github {
        repo: String,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        streams: Vec<String>,
    },
    /// Offline migration: input/destination paths are on the MCP host, not the remote storage host.
    Import {
        input: PathBuf,
        destination: PathBuf,
        model: String,
        dimensions: usize,
    },
    Optimize {
        #[serde(default)]
        collections: Vec<String>,
    },
    Prune {
        selection: PruneTarget,
        #[serde(default = "default_dry_run")]
        dry_run: bool,
    },
}

pub fn default_dry_run() -> bool {
    true
}

impl Ingestion {
    fn prepare(&mut self, projects: &[String]) -> Result<()> {
        let project = match self {
            Self::Code { project, .. }
            | Self::Docs { project, .. }
            | Self::Git { project, .. }
            | Self::Package { project, .. } => Some(project),
            _ => None,
        };
        if let Some(project) = project {
            if let Some(name) = project.as_ref() {
                ensure!(
                    projects.is_empty() || projects.contains(name),
                    "project is outside this MCP's configured PROJECTS"
                );
            } else {
                *project = projects.first().cloned();
            }
        }
        match self {
            Self::Git {
                url,
                rev,
                branch,
                tag,
                ..
            } => {
                ensure!(!url.trim().is_empty(), "Git URL must not be empty");
                ensure!(
                    [rev, branch, tag].iter().filter(|v| v.is_some()).count() <= 1,
                    "choose only one of rev, branch or tag"
                );
            }
            Self::Package {
                package, version, ..
            } => {
                for value in [package, version] {
                    ensure!(
                        !value.is_empty()
                            && value
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
                        "invalid package name/version"
                    );
                }
            }
            Self::Github { repo, streams, .. } => {
                ensure!(
                    repo.split('/').count() == 2 && repo.split('/').all(|v| !v.is_empty()),
                    "GitHub repository must be OWNER/REPO"
                );
                ensure!(
                    streams.iter().all(|s| matches!(
                        s.as_str(),
                        "all" | "issues" | "prs" | "issue_comments" | "pr_comments"
                    )),
                    "unknown GitHub stream"
                );
            }
            Self::Optimize { collections } => ensure!(
                collections
                    .iter()
                    .all(|n| COLLECTIONS.contains(&n.as_str())),
                "unknown collection"
            ),
            Self::Import {
                model, dimensions, ..
            } => ensure!(
                !model.is_empty() && *dimensions > 0,
                "model and positive dimensions are required"
            ),
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Jobs {
    store: Store,
    projects: Vec<String>,
    workers: Arc<Semaphore>,
}

fn active(row: &Value) -> bool {
    matches!(row["state"].as_str(), Some("queued" | "running"))
}

impl Jobs {
    pub fn new(store: Store, projects: Vec<String>) -> Self {
        Self {
            store,
            projects,
            workers: Arc::new(Semaphore::new(2)),
        }
    }

    pub async fn start(&self, mut request: Ingestion) -> Result<Value> {
        request.prepare(&self.projects)?;
        let id = maintenance::operation_id();
        let time = maintenance::now();
        let row = json!({"id":id,"state":"queued","request":request,"created_at":time,
            "updated_at":time,"heartbeat_at":time,"cancel_requested":false});
        self.store.set_metadata(JOBS, &id, row.clone()).await?;
        let worker = self.clone();
        tokio::spawn(async move {
            if let Err(error) = worker.run(&id, request).await {
                tracing::error!(job=%id,error=%error,"MCP ingestion worker stopped");
            }
        });
        Ok(row)
    }

    async fn run(&self, id: &str, request: Ingestion) -> Result<()> {
        let (state, output, error) = {
            let work = async {
                let _permit = self.workers.acquire().await?;
                self.store
                    .update_metadata(JOBS, id, |previous| {
                        let mut row = previous.context("job disappeared")?;
                        row["state"] = json!("running");
                        row["updated_at"] = json!(maintenance::now());
                        Ok(row)
                    })
                    .await?;
                self.execute(request).await
            };
            // Poll the whole heartbeat future alongside ingestion: either may wait
            // on the local Store writer mutex held by the other.
            let monitor = async {
                let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT));
                loop {
                    heartbeat.tick().await;
                    let row = self
                        .store
                        .metadata(JOBS, id)
                        .await?
                        .context("job disappeared")?;
                    if row["cancel_requested"] == true {
                        return anyhow::Ok(());
                    }
                    self.store
                        .update_metadata(JOBS, id, |previous| {
                            let mut row = previous.context("job disappeared")?;
                            row["heartbeat_at"] = json!(maintenance::now());
                            row["updated_at"] = json!(maintenance::now());
                            Ok(row)
                        })
                        .await?;
                }
            };
            tokio::select! {
                result=work => match result {
                    Ok(value)=>("completed",Some(value),None),
                    Err(error)=>("failed",None,Some(format!("{error:#}"))),
                },
                result=monitor => match result {
                    Ok(())=>("cancelled",None,None),
                    Err(error)=>("failed",None,Some(format!("job heartbeat failed: {error:#}"))),
                },
            }
        }; // Drop both futures (and any write guard) before persisting terminal state.
        self.finish(id, state, output, error).await
    }

    async fn finish(
        &self,
        id: &str,
        state: &str,
        output: Option<Value>,
        error: Option<String>,
    ) -> Result<()> {
        self.store
            .update_metadata(JOBS, id, |previous| {
                let mut row = previous.context("job disappeared")?;
                row["state"] = json!(state);
                row["result"] = json!(output);
                row["error"] = json!(error);
                row["updated_at"] = json!(maintenance::now());
                Ok(row)
            })
            .await
    }

    pub async fn status(&self, id: Option<&str>) -> Result<Value> {
        if let Some(id) = id {
            return self.status_one(id).await;
        }
        let mut jobs = self.store.metadata_rows(JOBS).await?;
        jobs.sort_by_key(|row| std::cmp::Reverse(row["created_at"].as_u64().unwrap_or(0)));
        jobs.truncate(20);
        let mut statuses = Vec::new();
        for row in jobs {
            if let Some(id) = row["id"].as_str() {
                statuses.push(self.status_one(id).await?);
            }
        }
        Ok(json!(statuses))
    }

    async fn status_one(&self, id: &str) -> Result<Value> {
        let row = self
            .store
            .metadata(JOBS, id)
            .await?
            .context("unknown ingestion job")?;
        if active(&row)
            && maintenance::now().saturating_sub(row["heartbeat_at"].as_u64().unwrap_or(0)) > STALE
        {
            self.store.update_metadata(JOBS,id,|previous| {
                    let mut row=previous.context("job disappeared")?;
                    if active(&row) && maintenance::now().saturating_sub(row["heartbeat_at"].as_u64().unwrap_or(0))>STALE {
                        row["state"]=json!("interrupted");
                        row["error"]=json!("MCP worker heartbeat expired; retry_ingestion can resume committed work");
                        row["updated_at"]=json!(maintenance::now());
                    }
                    Ok(row)
                }).await?;
            return Ok(self.store.metadata(JOBS, id).await?.unwrap_or_default());
        }
        Ok(row)
    }

    pub async fn cancel(&self, id: &str) -> Result<Value> {
        self.store
            .update_metadata(JOBS, id, |previous| {
                let mut row = previous.context("unknown ingestion job")?;
                if active(&row) {
                    row["cancel_requested"] = json!(true);
                }
                Ok(row)
            })
            .await?;
        self.status(Some(id)).await
    }

    pub async fn retry(&self, id: &str) -> Result<Value> {
        let previous = self.status(Some(id)).await?;
        ensure!(
            !active(&previous),
            "job is still active; cancel it or wait before retrying"
        );
        self.start(serde_json::from_value(previous["request"].clone())?)
            .await
    }

    pub async fn execute(&self, mut request: Ingestion) -> Result<Value> {
        request.prepare(&self.projects)?;
        let mut touched = vec![];
        let result = match request {
            Ingestion::Code {
                repo_path,
                force,
                patterns,
                no_docs,
                project,
            } => {
                let path = repo_path.canonicalize()?;
                ingest::code::ingest_code(
                    &self.store,
                    &path,
                    force,
                    &patterns,
                    None,
                    project.as_deref(),
                )
                .await?;
                touched.push("code_chunks");
                if !no_docs {
                    ingest::docs::ingest_docs(&self.store, &path, force, None, project.as_deref())
                        .await?;
                    touched.push("documents");
                }
                ingest::repo_index::upsert_repo(
                    &self.store,
                    &path.to_string_lossy(),
                    &ingest::repo_index::RepoMeta {
                        source_kind: "local",
                        package_name: None,
                        version: None,
                        git_url: None,
                        git_rev: None,
                        project: project.as_deref(),
                    },
                )
                .await?;
                json!({"repo_path":path})
            }
            Ingestion::Docs {
                repo_path,
                force,
                project,
            } => {
                ingest::docs::ingest_docs(&self.store, &repo_path, force, None, project.as_deref())
                    .await?;
                touched.push("documents");
                json!({"repo_path":repo_path})
            }
            Ingestion::Git {
                url,
                rev,
                branch,
                tag,
                force,
                no_docs,
                project,
            } => {
                ingest::git::ingest_git(
                    &self.store,
                    &url,
                    rev.as_deref(),
                    branch.as_deref(),
                    tag.as_deref(),
                    force,
                    no_docs,
                    project.as_deref(),
                )
                .await?;
                touched.push("code_chunks");
                if !no_docs {
                    touched.push("documents");
                }
                json!({"url":url})
            }
            Ingestion::Package {
                package,
                version,
                ecosystem,
                force,
                project,
            } => {
                match ecosystem {
                    Ecosystem::Haskell => {
                        ingest::hackage::ingest_hackage(
                            &self.store,
                            &package,
                            &version,
                            force,
                            project.as_deref(),
                        )
                        .await?
                    }
                    Ecosystem::Rust => {
                        ingest::crates::ingest_crate(
                            &self.store,
                            &package,
                            &version,
                            force,
                            project.as_deref(),
                        )
                        .await?
                    }
                    Ecosystem::Python => {
                        ingest::pypi::ingest_pypi(
                            &self.store,
                            &package,
                            &version,
                            force,
                            project.as_deref(),
                        )
                        .await?
                    }
                }
                touched.push("code_chunks");
                json!({"package":package,"version":version})
            }
            Ingestion::Github {
                repo,
                force,
                streams,
            } => {
                ingest::github::ingest_github(&self.store, &repo, force, &streams).await?;
                touched.extend([
                    "github_issues",
                    "github_prs",
                    "github_issue_comments",
                    "github_pr_comments",
                ]);
                json!({"repo":repo})
            }
            Ingestion::Import {
                input,
                destination,
                model,
                dimensions,
            } => {
                let report = agentix_indexer::migration::import_postgres(
                    &input,
                    &destination,
                    &model,
                    dimensions,
                )
                .await?;
                return Ok(json!({"local_destination":destination,"report":report}));
            }
            Ingestion::Optimize { collections } => {
                let names = if collections.is_empty() {
                    COLLECTIONS.iter().map(|n| (*n).to_owned()).collect()
                } else {
                    collections
                };
                let mut reports = vec![];
                for name in names {
                    reports.push(maintenance::maintain(&self.store, &name, false, true).await?);
                }
                return Ok(json!({"maintenance":reports}));
            }
            Ingestion::Prune { selection, dry_run } => {
                let repos = match selection {
                    PruneTarget::Repo { repo_path } => vec![repo_path],
                    PruneTarget::Local => self
                        .store
                        .metadata_rows("repo_index")
                        .await?
                        .iter()
                        .filter(|r| r["source_kind"] == "local")
                        .filter_map(|r| r["repo_path"].as_str().map(str::to_owned))
                        .collect(),
                    PruneTarget::OldVersions { package } => {
                        ingest::prune::old_version_repos(&self.store, package.as_deref()).await?
                    }
                };
                let mut preview = vec![];
                for repo in repos {
                    let filter = agentix_store::repo_filter(&repo);
                    preview.push(json!({"repo_path":repo,"code_chunks":self.store.count("code_chunks",&filter).await?,"documents":self.store.count("documents",&filter).await?}));
                    ingest::prune::prune_repo(&self.store, &repo, dry_run).await?;
                }
                if !dry_run {
                    touched.extend(["code_chunks", "documents"]);
                }
                json!({"dry_run":dry_run,"global_scope":true,"repositories":preview})
            }
        };
        let reports = maintenance::after_ingest(&self.store, &touched)
            .await
            .context("ingestion committed; index maintenance failed and remains pending")?;
        Ok(json!({"ingestion":result,"maintenance":reports}))
    }
}
