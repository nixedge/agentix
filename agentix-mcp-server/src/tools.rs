use crate::operations::{self, Ingestion, Jobs};
use agentix_store::Store;
use rmcp::{
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    schemars, tool, Error as McpError, ServerHandler,
};
use serde::Deserialize;

use agentix_indexer::ingest::completion::package_complete;

/// Returns a warning string if the Ollama embed model is loaded on CPU (size_vram == 0).
/// Empty string if GPU is in use or the check fails.
async fn gpu_warning() -> String {
    let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let Ok(resp) = reqwest::get(format!("{host}/api/ps")).await else {
        return String::new();
    };
    let Ok(data) = resp.json::<serde_json::Value>().await else {
        return String::new();
    };
    let on_cpu = data["models"]
        .as_array()
        .map(|models| {
            models.iter().any(|m| {
                m["size"].as_u64().unwrap_or(0) > 0 && m["size_vram"].as_u64().unwrap_or(0) == 0
            })
        })
        .unwrap_or(false);

    if on_cpu {
        "\n\nWARNING: Ollama is running the embed model on CPU (size_vram=0). \
         Indexing will be slow. Check GPU/CUDA configuration on the Ollama host."
            .to_string()
    } else {
        String::new()
    }
}

use agentix_search::{db, embed, fmt, rerank};

const RERANK_POOL_SIZE: i32 = 20;

fn default_limit() -> i32 {
    10
}

// ── Parameter structs ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchCodeParams {
    #[schemars(description = "Natural language or code search query")]
    pub query: String,
    #[schemars(description = "Number of results to return (1..100, default 10)")]
    #[serde(default = "default_limit")]
    pub limit: i32,
    #[schemars(description = "Filter by language: typescript, python, rust, go, etc.")]
    #[serde(default)]
    pub language: Option<String>,
    #[schemars(
        description = "Filter by symbol kind: function, class, interface, type, enum, chunk"
    )]
    #[serde(default)]
    pub symbol_kind: Option<String>,
    #[schemars(
        description = "Filter by repository. Supports SQL LIKE patterns: use an exact repo_path \
                       from list_repos, or a prefix pattern like 'hackage::%' to match all \
                       Hackage packages, 'chap::%' for CHaP, etc."
    )]
    #[serde(default)]
    pub repo_path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchDocsParams {
    #[schemars(description = "Natural language search query")]
    pub query: String,
    #[schemars(description = "Number of results to return (1..100, default 10)")]
    #[serde(default = "default_limit")]
    pub limit: i32,
    #[schemars(
        description = "Filter by document kind: readme, workflow, sop, plan, skill, agent_instruction"
    )]
    #[serde(default)]
    pub doc_kind: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchGithubParams {
    #[schemars(description = "Natural language search query")]
    pub query: String,
    #[schemars(description = "Number of results to return (1..100, default 10)")]
    #[serde(default = "default_limit")]
    pub limit: i32,
    #[schemars(description = "Filter by entity type: issues or prs (default: both)")]
    #[serde(default)]
    pub entity_type: Option<String>,
    #[schemars(description = "Filter by repo: OWNER/REPO")]
    #[serde(default)]
    pub repo: Option<String>,
    #[schemars(description = "Filter by state: open, closed, merged")]
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetFileParams {
    #[schemars(description = "Repository root path (from list_repos)")]
    pub repo_path: String,
    #[schemars(description = "File path relative to the repo root")]
    pub file_path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FetchPackageParams {
    #[schemars(description = "Package or crate name (e.g. 'serialise', 'tokio')")]
    pub package: String,
    #[schemars(description = "Package version (e.g. '0.2.6.1', '1.0.0')")]
    pub version: String,
    #[schemars(
        description = "Package ecosystem: 'haskell' (CHaP/Hackage, default), 'rust' (crates.io), or 'python' (PyPI)"
    )]
    #[serde(default = "default_ecosystem")]
    pub ecosystem: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StartIngestionParams {
    pub request: Ingestion,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct JobParams {
    pub job_id: String,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct JobStatusParams {
    pub job_id: Option<String>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OptimizeParams {
    #[serde(default)]
    pub collections: Vec<String>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PruneParams {
    pub selection: operations::PruneTarget,
    #[serde(default = "operations::default_dry_run")]
    pub dry_run: bool,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListSourcesParams {
    pub source_kind: Option<String>,
    #[serde(default)]
    pub local: bool,
}

fn default_ecosystem() -> String {
    "haskell".to_string()
}

// ── Server ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct CodeSearchServer {
    pool: Store,
    projects: Vec<String>,
    jobs: Jobs,
}

#[tool(tool_box)]
impl CodeSearchServer {
    pub fn new(pool: Store) -> Self {
        let projects: Vec<String> = std::env::var("PROJECTS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let jobs = Jobs::new(pool.clone(), projects.clone());
        Self {
            pool,
            projects,
            jobs,
        }
    }

    #[tool(
        description = "Start background ingestion: local code/docs, an entire Git checkout, Hackage/CHaP/crates/PyPI, GitHub streams, or offline import into a fresh LOCAL directory on the MCP host. Returns a job ID immediately; poll ingestion_status. Ingestion automatically schedules shared index maintenance."
    )]
    async fn start_ingestion(
        &self,
        #[tool(aggr)] params: StartIngestionParams,
    ) -> Result<CallToolResult, McpError> {
        admin_result(self.jobs.start(params.request).await)
    }

    #[tool(
        description = "Poll a background ingestion/optimization/prune job, including result, errors and maintenance. With no job_id, list the latest 20 jobs. Jobs are shared between MCP clients; a missing worker heartbeat becomes interrupted."
    )]
    async fn ingestion_status(
        &self,
        #[tool(aggr)] params: JobStatusParams,
    ) -> Result<CallToolResult, McpError> {
        admin_result(self.jobs.status(params.job_id.as_deref()).await)
    }

    #[tool(
        description = "Request cancellation of a shared background job. Committed files remain; the worker stops further work at its next heartbeat. Poll ingestion_status until terminal."
    )]
    async fn cancel_ingestion(
        &self,
        #[tool(aggr)] params: JobParams,
    ) -> Result<CallToolResult, McpError> {
        admin_result(self.jobs.cancel(&params.job_id).await)
    }

    #[tool(
        description = "Retry a finished/failed/cancelled/interrupted job using its original request. Returns a NEW job ID. Partial Git/package ingests resume safely; cannot retry a still-active job."
    )]
    async fn retry_ingestion(
        &self,
        #[tool(aggr)] params: JobParams,
    ) -> Result<CallToolResult, McpError> {
        admin_result(self.jobs.retry(&params.job_id).await)
    }

    #[tool(
        description = "Start explicit shared index optimization as a background job. Empty collections means all six searchable collections. Automatic ingestion maintenance normally batches small changes. Poll ingestion_status for completion or a competing optimization lease."
    )]
    async fn optimize_index(
        &self,
        #[tool(aggr)] params: OptimizeParams,
    ) -> Result<CallToolResult, McpError> {
        admin_result(
            self.jobs
                .start(Ingestion::Optimize {
                    collections: params.collections,
                })
                .await,
        )
    }

    #[tool(
        description = "Global index administration: start repository/local-clone/old-package-version pruning. Defaults to dry_run=true and returns affected repositories/counts. Set dry_run=false to delete from the SHARED index, including other project tags. Poll ingestion_status."
    )]
    async fn prune_index(
        &self,
        #[tool(aggr)] params: PruneParams,
    ) -> Result<CallToolResult, McpError> {
        admin_result(
            self.jobs
                .start(Ingestion::Prune {
                    selection: params.selection,
                    dry_run: params.dry_run,
                })
                .await,
        )
    }

    #[tool(
        description = "Global index health: collection counts, shared maintenance generations/leases/errors, GitHub sync state and latest background jobs. Use this to discover incomplete ingestion or pending optimization."
    )]
    async fn index_status(&self) -> Result<CallToolResult, McpError> {
        admin_result(
            async {
                let mut counts = serde_json::Map::new();
                let mut maintenance = Vec::new();
                for name in agentix_store::COLLECTIONS {
                    counts.insert(
                        (*name).into(),
                        serde_json::json!(self.pool.count(name, "true").await?),
                    );
                    if let Some(mut state) = self.pool.metadata("maintenance", name).await? {
                        state["collection"] = serde_json::json!(name);
                        maintenance.push(state);
                    }
                }
                let mut git = self.pool.metadata_rows("git_ingest").await?;
                // Older Git completion payloads did not include their metadata key.
                git.retain(|row| row["repo_path"].is_string());
                for source in self.pool.metadata_rows("repo_index").await? {
                    if source["source_kind"] == "git" {
                        if let Some(repo) = source["repo_path"].as_str() {
                            if let Some(mut row) = self.pool.metadata("git_ingest", repo).await? {
                                if !row["repo_path"].is_string() {
                                    row["repo_path"] = serde_json::json!(repo);
                                    git.push(row);
                                }
                            }
                        }
                    }
                }
                anyhow::Ok(
                    serde_json::json!({"counts":counts,"maintenance":maintenance,
                "sync_state":self.pool.metadata_rows("sync_state").await?,
                "git_ingest":git,
                "package_ingest":self.pool.metadata_rows("package_ingest").await?,
                "jobs":self.jobs.status(None).await?}),
                )
            }
            .await,
        )
    }

    #[tool(
        description = "List the global source registry with package versions, Git revisions and project tags. Filter source_kind (local, git, chap, hackage, crates.io, pypi) or local=true. For project-scoped searchable file/chunk summaries use list_repos."
    )]
    async fn list_sources(
        &self,
        #[tool(aggr)] params: ListSourcesParams,
    ) -> Result<CallToolResult, McpError> {
        admin_result(
            async {
                let rows = self.pool.metadata_rows("repo_index").await?;
                anyhow::Ok(serde_json::json!(rows
                    .into_iter()
                    .filter(|r| (!params.local || r["source_kind"] == "local")
                        && params
                            .source_kind
                            .as_deref()
                            .is_none_or(|kind| r["source_kind"] == kind))
                    .collect::<Vec<_>>()))
            }
            .await,
        )
    }

    // ── search_code ───────────────────────────────────────────────────────────

    #[tool(
        description = "Hybrid BM25+vector search over indexed code with optional cross-encoder reranking. \
                       Supports filtering by language and symbol_kind (function, class, etc.)."
    )]
    async fn search_code(
        &self,
        #[tool(aggr)] params: SearchCodeParams,
    ) -> Result<CallToolResult, McpError> {
        let SearchCodeParams {
            query,
            limit,
            language,
            symbol_kind,
            repo_path,
        } = params;

        let query_vec = match embed::embed(&query).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Embedding failed (is Ollama running?): {e}"
                ))]));
            }
        };

        let rerank_enabled = std::env::var("RERANK_MODEL")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        let candidates = if rerank_enabled {
            limit.max(RERANK_POOL_SIZE)
        } else {
            limit
        };

        let lang_ref = language.as_deref();
        let kind_ref = symbol_kind.as_deref();
        let repo_ref = repo_path.as_deref();

        let rows = match db::hybrid_search(
            &self.pool,
            &query,
            &query_vec,
            candidates,
            lang_ref,
            kind_ref,
            repo_ref,
            &self.projects,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Database error: {e}"
                ))]));
            }
        };

        if rows.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No results found.",
            )]));
        }

        let docs: Vec<String> = rows.iter().map(|r| r.content.clone()).collect();
        let scores = rerank::rerank(&query, &docs).await;

        let text = if let Some(ref score_vec) = scores {
            let mut indexed: Vec<(usize, f32)> = score_vec.iter().copied().enumerate().collect();
            indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let top: Vec<_> = indexed.into_iter().take(limit as usize).collect();
            let final_rows: Vec<db::ChunkRow> = top.iter().map(|(i, _)| rows[*i].clone()).collect();
            let final_scores: Vec<f32> = top.iter().map(|(_, s)| *s).collect();
            fmt::fmt_chunks(&final_rows, Some(&final_scores), "rerank")
        } else {
            fmt::fmt_chunks(&rows[..rows.len().min(limit as usize)], None, "rrf")
        };

        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    // ── bm25_search ───────────────────────────────────────────────────────────

    #[tool(
        description = "BM25 full-text only search over code. Best for exact identifiers, symbols, or keywords. \
                       Supports filtering by language and symbol_kind."
    )]
    async fn bm25_search(
        &self,
        #[tool(aggr)] params: SearchCodeParams,
    ) -> Result<CallToolResult, McpError> {
        let SearchCodeParams {
            query,
            limit,
            language,
            symbol_kind,
            repo_path,
        } = params;

        let rows = match db::bm25_search(
            &self.pool,
            &query,
            limit,
            language.as_deref(),
            symbol_kind.as_deref(),
            repo_path.as_deref(),
            &self.projects,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Database error: {e}"
                ))]));
            }
        };
        Ok(CallToolResult::success(vec![Content::text(
            fmt::fmt_chunks(&rows, None, "bm25"),
        )]))
    }

    // ── search_docs ───────────────────────────────────────────────────────────

    #[tool(
        description = "Hybrid BM25+vector search over indexed documentation: README files, \
                       AGENTS.md, CLAUDE.md, and .agent/** workflow/skill/plan/SOP files. \
                       Filter by doc_kind: readme, workflow, sop, plan, skill, agent_instruction."
    )]
    async fn search_docs(
        &self,
        #[tool(aggr)] params: SearchDocsParams,
    ) -> Result<CallToolResult, McpError> {
        let SearchDocsParams {
            query,
            limit,
            doc_kind,
        } = params;

        let query_vec = match embed::embed(&query).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Embedding failed (is Ollama running?): {e}"
                ))]));
            }
        };

        let rows = match db::search_docs_hybrid(
            &self.pool,
            &query,
            &query_vec,
            limit,
            doc_kind.as_deref(),
            &self.projects,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Database error: {e}"
                ))]));
            }
        };

        Ok(CallToolResult::success(vec![Content::text(fmt::fmt_docs(
            &rows,
        ))]))
    }

    // ── search_github ─────────────────────────────────────────────────────────

    #[tool(
        description = "Hybrid BM25+vector search over indexed GitHub issues and pull requests. \
                       Filter by entity_type (issues/prs), repo (OWNER/REPO), or state (open/closed/merged)."
    )]
    async fn search_github(
        &self,
        #[tool(aggr)] params: SearchGithubParams,
    ) -> Result<CallToolResult, McpError> {
        let SearchGithubParams {
            query,
            limit,
            entity_type,
            repo,
            state,
        } = params;

        let query_vec = match embed::embed(&query).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Embedding failed (is Ollama running?): {e}"
                ))]));
            }
        };

        let rows = match db::search_github_hybrid(
            &self.pool,
            &query,
            &query_vec,
            limit,
            repo.as_deref(),
            state.as_deref(),
            entity_type.as_deref(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Database error: {e}"
                ))]));
            }
        };

        Ok(CallToolResult::success(vec![Content::text(
            fmt::fmt_github(&rows),
        )]))
    }

    // ── list_repos ────────────────────────────────────────────────────────────

    #[tool(description = "List all indexed repositories with chunk and file counts.")]
    async fn list_repos(&self) -> Result<CallToolResult, McpError> {
        let repos = match db::list_repos(&self.pool, &self.projects).await {
            Ok(r) => r,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Database error: {e}"
                ))]));
            }
        };
        if repos.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No repositories indexed yet.\nRun: just index /path/to/repo",
            )]));
        }
        let mut lines = vec!["Indexed repositories:\n".to_string()];
        for r in &repos {
            lines.push(format!(
                "  {}\n    {} files · {} chunks · last indexed {}",
                r.repo_path, r.files, r.chunks, r.last_indexed
            ));
        }
        Ok(CallToolResult::success(vec![Content::text(
            lines.join("\n"),
        )]))
    }

    // ── fetch_package ─────────────────────────────────────────────────────────

    #[tool(
        description = "Fetch a package from crates.io (Rust), CHaP/Hackage (Haskell), or PyPI (Python) \
                       and index it for code search. Returns immediately if already indexed. Use this when \
                       you need source-level detail about a library that isn't in the current index. \
                       Set ecosystem='rust' for Rust crates, ecosystem='haskell' (default) for \
                       Haskell packages (checks CHaP first, falls back to Hackage), or \
                       ecosystem='python' for Python packages from PyPI."
    )]
    async fn fetch_package(
        &self,
        #[tool(aggr)] params: FetchPackageParams,
    ) -> Result<CallToolResult, McpError> {
        let FetchPackageParams {
            package,
            version,
            ecosystem,
            force,
        } = params;
        let pkg_ver = format!("{package}-{version}");

        let (subcommand, repo_path_keys): (&str, Vec<String>) = match ecosystem.as_str() {
            "rust" => ("crate", vec![format!("crates.io::{pkg_ver}")]),
            "python" => ("pypi", vec![format!("pypi::{pkg_ver}")]),
            "haskell" => (
                "hackage",
                vec![format!("chap::{pkg_ver}"), format!("hackage::{pkg_ver}")],
            ),
            _ => {
                return Err(McpError::invalid_params(
                    "ecosystem must be haskell, rust or python",
                    None,
                ))
            }
        };

        let language = match ecosystem.as_str() {
            "rust" => "rust",
            "python" => "python",
            _ => "haskell",
        };

        // Build a query that checks whether the package is already indexed
        // AND visible in the current project scope.
        //
        // When PROJECTS is set we only skip ingest if the package is already
        // tagged with the current project — otherwise we run ingest anyway so
        // the ON CONFLICT handler merges the new project into the existing array.
        let visible = count_package(&self.pool, &repo_path_keys, &self.projects)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let mut complete = false;
        for key in &repo_path_keys {
            complete |= package_complete(&self.pool, key)
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }
        if !force && visible > 0 && complete {
            agentix_indexer::maintenance::maintain(&self.pool, "code_chunks", false, false)
                .await
                .map_err(|e| {
                    McpError::internal_error(
                        format!("package complete; maintenance failed: {e:#}"),
                        None,
                    )
                })?;
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "Already indexed: {pkg_ver} ({visible} chunks).\n\
                 Use search_code with language={language} to query it."
            ))]));
        }

        // Check whether the package exists at all (unscoped) before we ingest.
        // If it does, ingest will just add the new project to existing chunks.
        let pre_existing = count_package(&self.pool, &repo_path_keys, &[])
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        // Call the ingest functions directly (no subprocess).
        // Tag newly indexed packages with the first configured project (if any).
        let ingest_project = self.projects.first().map(String::as_str);
        let gpu_warn = gpu_warning().await;
        if gpu_warn.is_empty() {
            eprintln!("[agentic-nix] Fetching and indexing {ecosystem} {pkg_ver}...");
        } else {
            eprintln!("[agentic-nix] Fetching and indexing {ecosystem} {pkg_ver}... (WARNING: Ollama on CPU — will be slow)");
        }
        let kind = match subcommand {
            "crate" => operations::Ecosystem::Rust,
            "pypi" => operations::Ecosystem::Python,
            _ => operations::Ecosystem::Haskell,
        };
        let result = self
            .jobs
            .execute(Ingestion::Package {
                package: package.clone(),
                version: version.clone(),
                ecosystem: kind,
                force,
                project: ingest_project.map(str::to_owned),
            })
            .await;

        if let Err(e) = result {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "Ingest failed: {e:#}"
            ))]));
        }

        // Query final visible chunk count.
        let chunks = count_package(&self.pool, &repo_path_keys, &self.projects)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let msg = if pre_existing > 0 && ingest_project.is_some() {
            // Package existed under a different scope — project array was extended.
            let proj = ingest_project.unwrap_or_default();
            format!(
                "Added {pkg_ver} to project '{proj}': {chunks} chunks now visible.\n\
                 Use search_code with language={language} to query it.{gpu_warn}"
            )
        } else {
            format!(
                "Indexed {pkg_ver} from {ecosystem}: {chunks} chunks.\n\
                 Use search_code with language={language} to query it.{gpu_warn}"
            )
        };
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    // ── get_file ──────────────────────────────────────────────────────────────

    #[tool(
        description = "Retrieve all indexed chunks for a specific file. Writes the reconstructed \
                       source to /tmp/agentic-nix/ and returns the path — use the Read tool to \
                       inspect it without flooding context."
    )]
    async fn get_file(
        &self,
        #[tool(aggr)] params: GetFileParams,
    ) -> Result<CallToolResult, McpError> {
        let GetFileParams {
            repo_path,
            file_path,
        } = params;

        let rows =
            match db::get_file_chunks(&self.pool, &repo_path, &file_path, &self.projects).await {
                Ok(r) => r,
                Err(e) => {
                    return Ok(CallToolResult::success(vec![Content::text(format!(
                        "Database error: {e}"
                    ))]));
                }
            };
        if rows.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No chunks found for {repo_path}/{file_path}"
            ))]));
        }

        // Reconstruct the file from ordered chunks and write to /tmp.
        // Chunks may overlap (overlapping-window fallback), so de-duplicate by
        // collecting all lines into a BTreeMap keyed by 1-indexed line number.
        let mut line_map: std::collections::BTreeMap<i32, String> =
            std::collections::BTreeMap::new();
        for r in &rows {
            let start = r.start_line.unwrap_or(1).max(1);
            for (offset, line) in r.content.lines().enumerate() {
                line_map
                    .entry(start + offset as i32)
                    .or_insert_with(|| line.to_string());
            }
        }
        let source = line_map.values().cloned().collect::<Vec<_>>().join("\n");

        // Build a stable, human-readable path under /tmp/agentic-nix/.
        // Sanitise repo_path (colons → underscores) so it's a valid directory name.
        let safe_repo = repo_path.replace("::", "__").replace([':', '/'], "_");
        let out_path = std::path::PathBuf::from("/tmp/agentic-nix")
            .join(&safe_repo)
            .join(&file_path);

        if let Some(parent) = out_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Failed to create temp directory: {e}"
                ))]));
            }
        }
        if let Err(e) = std::fs::write(&out_path, &source) {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "Failed to write temp file: {e}"
            ))]));
        }

        let lang = rows
            .first()
            .and_then(|r| r.language.as_deref())
            .unwrap_or("text");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "File written to: {}\n({} lines, language: {lang})\n\
             Use the Read tool to inspect it.",
            out_path.display(),
            line_map.len(),
        ))]))
    }
}

#[tool(tool_box)]
impl ServerHandler for CodeSearchServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            instructions: Some({
                let project_note = if self.projects.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\nProject isolation: this server is scoped to project(s): {}. \
                         Only chunks tagged with these projects are visible.\n",
                        self.projects.join(", ")
                    )
                };
                format!(
                    "Hybrid BM25+vector code search with optional cross-encoder reranking.\n\
                     {project_note}\n\
                     Tools:\n\
                     - search_code: hybrid search over indexed code (language, symbol_kind filters)\n\
                     - bm25_search: BM25-only, best for exact identifiers and symbols\n\
                     - search_docs: hybrid search over indexed markdown docs (doc_kind filter)\n\
                     - search_github: hybrid search over indexed GitHub issues + PRs (entity_type, repo, state filters)\n\
                     - list_repos: list indexed repositories with file and chunk counts\n\
                     - get_file: retrieve all chunks for a specific file\n\
                     - fetch_package: synchronously download/index Haskell, Rust or Python packages\n\
                     - start_ingestion: background code/docs, whole Git checkout, package, GitHub streams or local offline import; returns a job ID\n\
                     - ingestion_status: poll a job or list recent shared jobs; keep the originating MCP running until terminal\n\
                     - cancel_ingestion / retry_ingestion: cancel work or start a new attempt from committed progress\n\
                     - index_status / list_sources: shared counts, completion markers, maintenance, sync errors and source registry\n\
                     - optimize_index: background optimization of selected collections (empty means all)\n\
                     - prune_index: global repository/local/old-version cleanup; dry_run defaults true\n\
                     Ingestion automatically batches index maintenance with a shared lease across MCP clients. Small changes may remain pending until a later ingestion or explicit optimize_index; there is no idle scheduler. Administration is global even when PROJECTS filters search. Local paths and offline import destinations refer to the MCP host.\n\
                     \n\
                     IMPORTANT — external packages:\n\
                     When you need source-level detail about a library that isn't in the current index, \
                     call fetch_package FIRST. It downloads and indexes the package so subsequent \
                     search_code calls can find it. It is a no-op if already indexed.\n\
                     - Haskell: fetch_package({{\"package\": \"serialise\", \"version\": \"0.2.6.1\", \"ecosystem\": \"haskell\"}})\n\
                     - Rust: fetch_package({{\"package\": \"tokio\", \"version\": \"1.0.0\", \"ecosystem\": \"rust\"}})\n\
                     - Python: fetch_package({{\"package\": \"requests\", \"version\": \"2.31.0\", \"ecosystem\": \"python\"}})\n\
                     Then use search_code with language=haskell, language=rust, or language=python to query the indexed source."
                )
            }),
            ..Default::default()
        }
    }
}

async fn count_package(
    store: &Store,
    keys: &[String],
    projects: &[String],
) -> anyhow::Result<usize> {
    let filter = format!(
        "({}) AND ({})",
        keys.iter()
            .map(|key| agentix_store::repo_filter(key))
            .collect::<Vec<_>>()
            .join(" OR "),
        agentix_store::project_filter(projects)
    );
    store.count("code_chunks", &filter).await
}

fn admin_result(result: anyhow::Result<serde_json::Value>) -> Result<CallToolResult, McpError> {
    let value = result.map_err(|error| McpError::internal_error(format!("{error:#}"), None))?;
    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}
