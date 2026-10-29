use agentix_indexer::ingest;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use ingest::repo_index::{upsert_repo, RepoMeta};

#[derive(Parser)]
#[command(
    name = "ingest",
    about = "Index code / docs / GitHub into local LanceDB for hybrid search"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Import a read-only legacy PostgreSQL JSONL export into a fresh local index.
    Import {
        input: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        /// Assert the historical embedding model; must match EMBED_MODEL for later search.
        #[arg(long)]
        model: String,
        #[arg(long, default_value_t = 1536)]
        dimensions: usize,
    },
    /// Rebuild lexical/scalar/HNSW indexes after incremental ingestion or import.
    Optimize,
    /// Show stored counts for every searchable collection.
    Stats,
    /// Show persisted GitHub sync watermarks and errors.
    SyncStatus,
    /// Index a local codebase and its markdown docs (incremental: unchanged files are skipped).
    /// The repo is marked dirty in repo_index since it is a local clone, not a released version.
    Code {
        /// Path to the repository root
        repo_path: PathBuf,
        /// Clear existing chunks and re-index everything (also clears documents unless --no-docs)
        #[arg(long)]
        force: bool,
        /// Extra glob patterns to include (e.g. '**/*.roc')
        #[arg(short, long)]
        pattern: Vec<String>,
        /// Skip markdown doc indexing for this repo
        #[arg(long)]
        no_docs: bool,
        /// Tag chunks with this project name for scoped search isolation
        #[arg(long)]
        project: Option<String>,
    },
    /// Index markdown documentation only (AGENTS.md, README, .agent/**, and all .md files)
    Docs {
        /// Path to the repository root
        repo_path: PathBuf,
        /// Clear existing documents and re-index
        #[arg(long)]
        force: bool,
        /// Tag chunks with this project name for scoped search isolation
        #[arg(long)]
        project: Option<String>,
    },
    /// Index GitHub issues and pull requests
    Github {
        /// Repository in OWNER/REPO format
        repo: String,
        /// Re-fetch all items, ignoring watermarks
        #[arg(long)]
        force: bool,
        /// Streams to run: issues, prs, issue_comments, pr_comments (default: all)
        #[arg(long)]
        stream: Vec<String>,
    },
    /// Fetch a Haskell package from CHaP or Hackage and index it (checks CHaP first)
    Hackage {
        /// Package name (e.g. serialise, cardano-ledger-core)
        package: String,
        /// Package version (e.g. 0.2.6.1)
        version: String,
        /// Re-index even if already present
        #[arg(long)]
        force: bool,
        /// Tag chunks with this project name for scoped search isolation
        #[arg(long)]
        project: Option<String>,
    },
    /// Fetch a Rust crate from crates.io and index it
    Crate {
        /// Crate name (e.g. serde, tokio)
        package: String,
        /// Crate version (e.g. 1.0.0)
        version: String,
        /// Re-index even if already present
        #[arg(long)]
        force: bool,
        /// Tag chunks with this project name for scoped search isolation
        #[arg(long)]
        project: Option<String>,
    },
    /// Fetch a Python package from PyPI and index it
    Pypi {
        /// Package name (e.g. requests, numpy)
        package: String,
        /// Package version (e.g. 2.31.0)
        version: String,
        /// Re-index even if already present
        #[arg(long)]
        force: bool,
        /// Tag chunks with this project name for scoped search isolation
        #[arg(long)]
        project: Option<String>,
    },
    /// Clone a git repository by branch, tag, or commit hash and index it
    Git {
        /// Repository URL (https or ssh)
        url: String,
        /// Commit hash to check out (requires a full clone)
        #[arg(long)]
        rev: Option<String>,
        /// Branch name to clone (shallow)
        #[arg(long)]
        branch: Option<String>,
        /// Tag name to clone (shallow)
        #[arg(long)]
        tag: Option<String>,
        /// Re-index even if already present
        #[arg(long)]
        force: bool,
        /// Skip markdown doc indexing
        #[arg(long)]
        no_docs: bool,
        /// Tag chunks with this project name for scoped search isolation
        #[arg(long)]
        project: Option<String>,
    },
    /// List all indexed repos from repo_index
    List {
        /// Show only local clone repos
        #[arg(long)]
        local: bool,
        /// Filter by source kind: local, hackage, chap, crates.io, git
        #[arg(long)]
        source_kind: Option<String>,
    },
    /// Prune indexed repos from the database
    Prune {
        #[command(subcommand)]
        target: PruneTarget,
        /// Show what would be deleted without deleting anything
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum PruneTarget {
    /// Delete all local clone repos (source_kind = 'local')
    Local,
    /// For each package, delete all but the most recently indexed version
    OldVersions {
        /// Limit to a specific package name
        #[arg(long)]
        package: Option<String>,
    },
    /// Delete a specific repo by its repo_path key
    Repo {
        /// The repo_path identifier (e.g. 'hackage::serialise-0.2.6.1')
        repo_path: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Commands::Import {
        input,
        destination,
        model,
        dimensions,
    } = &cli.command
    {
        let report =
            agentix_indexer::migration::import_postgres(input, destination, model, *dimensions)
                .await?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let pool = agentix_store::Store::from_env().await?;
    match cli.command {
        Commands::Import { .. } => anyhow::bail!("import must use its explicit destination"),
        Commands::Optimize => {
            for name in agentix_store::COLLECTIONS {
                pool.maintain(name).await?;
            }
        }
        Commands::Stats => {
            for name in agentix_store::COLLECTIONS {
                println!("{name}\t{}", pool.count(name, "true").await?);
            }
        }
        Commands::SyncStatus => {
            println!(
                "{}",
                serde_json::to_string_pretty(&pool.metadata_rows("sync_state").await?)?
            );
        }
        Commands::Code {
            repo_path,
            force,
            pattern,
            no_docs,
            project,
        } => {
            let canonical = repo_path.canonicalize()?;
            let repo_str = canonical.to_string_lossy().into_owned();
            ingest::code::ingest_code(&pool, &repo_path, force, &pattern, None, project.as_deref())
                .await?;
            if !no_docs {
                ingest::docs::ingest_docs(&pool, &repo_path, force, None, project.as_deref())
                    .await?;
            }
            upsert_repo(
                &pool,
                &repo_str,
                &RepoMeta {
                    source_kind: "local",
                    package_name: None,
                    version: None,
                    git_url: None,
                    git_rev: None,
                    project: project.as_deref(),
                },
            )
            .await?;
        }
        Commands::Docs {
            repo_path,
            force,
            project,
        } => {
            ingest::docs::ingest_docs(&pool, &repo_path, force, None, project.as_deref()).await?;
        }
        Commands::Github {
            repo,
            force,
            stream,
        } => {
            ingest::github::ingest_github(&pool, &repo, force, &stream).await?;
        }
        Commands::Hackage {
            package,
            version,
            force,
            project,
        } => {
            ingest::hackage::ingest_hackage(&pool, &package, &version, force, project.as_deref())
                .await?;
        }
        Commands::Crate {
            package,
            version,
            force,
            project,
        } => {
            ingest::crates::ingest_crate(&pool, &package, &version, force, project.as_deref())
                .await?;
        }
        Commands::Pypi {
            package,
            version,
            force,
            project,
        } => {
            ingest::pypi::ingest_pypi(&pool, &package, &version, force, project.as_deref()).await?;
        }
        Commands::Git {
            url,
            rev,
            branch,
            tag,
            force,
            no_docs,
            project,
        } => {
            ingest::git::ingest_git(
                &pool,
                &url,
                rev.as_deref(),
                branch.as_deref(),
                tag.as_deref(),
                force,
                no_docs,
                project.as_deref(),
            )
            .await?;
        }
        Commands::List { local, source_kind } => {
            list_repos(&pool, local, source_kind.as_deref()).await?;
        }
        Commands::Prune { target, dry_run } => match target {
            PruneTarget::Local => {
                ingest::prune::prune_dirty(&pool, dry_run).await?;
            }
            PruneTarget::OldVersions { package } => {
                ingest::prune::prune_old_versions(&pool, package.as_deref(), dry_run).await?;
            }
            PruneTarget::Repo { repo_path } => {
                ingest::prune::prune_repo(&pool, &repo_path, dry_run).await?;
            }
        },
    }
    Ok(())
}

async fn list_repos(
    pool: &agentix_store::Store,
    local: bool,
    source_kind: Option<&str>,
) -> Result<()> {
    let projects = agentix_store::configured_projects();
    let rows = pool
        .metadata_rows("repo_index")
        .await?
        .into_iter()
        .filter(|r| {
            (!local || r["source_kind"] == "local")
                && source_kind.is_none_or(|kind| r["source_kind"] == kind)
                && (projects.is_empty()
                    || r["project"].as_array().is_some_and(|tags| {
                        tags.iter().any(|tag| projects.iter().any(|p| tag == p))
                    }))
        })
        .map(|r| {
            (
                r["repo_path"].as_str().unwrap_or_default().to_owned(),
                r["source_kind"].as_str().unwrap_or_default().to_owned(),
                r["package_name"].as_str().map(str::to_owned),
                r["version"].as_str().map(str::to_owned),
            )
        })
        .collect::<Vec<_>>();

    if rows.is_empty() {
        eprintln!("No repos in index.");
        return Ok(());
    }

    println!(
        "{:<55} {:<10} {:<20} version",
        "repo_path", "kind", "package"
    );
    println!("{}", "-".repeat(100));
    for (repo_path, kind, package_name, version) in &rows {
        println!(
            "{:<55} {:<10} {:<20} {}",
            repo_path,
            kind,
            package_name.as_deref().unwrap_or("-"),
            version.as_deref().unwrap_or("-"),
        );
    }
    println!("\n{} repo(s) listed.", rows.len());
    Ok(())
}
