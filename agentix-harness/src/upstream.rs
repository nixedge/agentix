//! Upstream primitives for callers already running inside an authorized jail.
//! These adapters do not grant permissions or establish a sandbox.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{bail, Context, Result};

pub use agentix_codex_patch::{ApplyPatchArgs, Hunk, UpdateFileChunk};
pub use codex_execpolicy::Decision as CommandDecision;

pub fn parse_patch(text: &str) -> Result<ApplyPatchArgs> {
    if text.len() > 1024 * 1024 {
        bail!("patch exceeds 1 MiB");
    }
    let patch = agentix_codex_patch::parse_patch(text)?;
    if patch.environment_id.is_some() || patch.workdir.is_some() {
        bail!("patch cannot select an environment or working directory");
    }
    if patch.hunks.len() > 1000 {
        bail!("patch exceeds 1000 hunks");
    }
    Ok(patch)
}

pub fn update_text(
    path: &str,
    expected: &str,
    current: &str,
    chunks: &[UpdateFileChunk],
) -> Result<String> {
    if current != expected {
        bail!("patch conflict: original content changed");
    }
    Ok(agentix_codex_patch::update_text(path, current, chunks)?)
}
pub use codex_utils_pty::{ProcessSignal, SpawnedProcess, TerminalSize};

pub struct CommandPolicy(codex_execpolicy::Policy);

impl CommandPolicy {
    /// Only load rules approved by the operator, never model-provided Starlark.
    pub fn parse(name: &str, source: &str) -> Result<Self> {
        if source.len() > 64 * 1024 {
            bail!("command policy exceeds 64 KiB");
        }
        let mut parser = codex_execpolicy::PolicyParser::new();
        parser.parse(name, source)?;
        Ok(Self(parser.build()))
    }

    /// A policy decision cannot override launch restrictions or grant new roots.
    pub fn check(&self, argv: &[String]) -> CommandDecision {
        if argv.is_empty() {
            return CommandDecision::Forbidden;
        }
        self.0.check(argv, &|_| CommandDecision::Prompt).decision
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ProcessMode {
    Pipe,
    ClosedStdin,
    Terminal(TerminalSize),
}

pub struct ProcessRequest {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub mode: ProcessMode,
}

pub async fn spawn(request: &ProcessRequest) -> Result<SpawnedProcess> {
    if !request.program.is_absolute() || !request.cwd.is_absolute() {
        bail!("process program and working directory must be absolute");
    }
    match request.mode {
        ProcessMode::Pipe => {
            codex_utils_pty::spawn_pipe_process(
                &request.program,
                &request.args,
                &request.cwd,
                &request.env,
                &None,
                &[],
            )
            .await
        }
        ProcessMode::ClosedStdin => {
            codex_utils_pty::spawn_pipe_process_no_stdin(
                &request.program,
                &request.args,
                &request.cwd,
                &request.env,
                &None,
                &[],
            )
            .await
        }
        ProcessMode::Terminal(size) => {
            if size.rows == 0 || size.cols == 0 {
                bail!("terminal dimensions must be nonzero");
            }
            let program = request
                .program
                .to_str()
                .context("PTY program path is not UTF-8")?;
            codex_utils_pty::spawn_pty_process(
                program,
                &request.args,
                &request.cwd,
                &request.env,
                &None,
                size,
                codex_utils_pty::ChildFds::Inherited(&[]),
            )
            .await
        }
    }
}

#[derive(Debug)]
pub struct FileMatch {
    pub path: PathBuf,
    pub score: u32,
    pub is_directory: bool,
}

#[derive(Debug)]
pub struct SearchResults {
    pub matches: Vec<FileMatch>,
    pub total_match_count: usize,
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Uses upstream symlink-following behavior; the jail controls filesystem access.
pub async fn search_files(
    root: &Path,
    query: &str,
    limit: NonZeroUsize,
    cancel: Arc<AtomicBool>,
) -> Result<SearchResults> {
    if !root.is_absolute() {
        bail!("search root must be absolute");
    }
    if limit.get() > 1000 || query.len() > 4096 {
        bail!("search exceeds the result or query limit");
    }
    let root = root.to_path_buf();
    let query = query.to_owned();
    // A dropped caller must cancel the blocking walker, not just its JoinHandle.
    let _cancel_on_drop = CancelOnDrop(Arc::clone(&cancel));
    tokio::task::spawn_blocking(move || {
        let results = codex_file_search::run(
            &query,
            vec![root],
            codex_file_search::FileSearchOptions {
                limit,
                ..Default::default()
            },
            Some(cancel),
        )?;
        Ok(SearchResults {
            matches: results
                .matches
                .into_iter()
                .map(|entry| FileMatch {
                    path: entry.full_path(),
                    score: entry.score,
                    is_directory: entry.match_type == codex_file_search::MatchType::Directory,
                })
                .collect(),
            total_match_count: results.total_match_count,
        })
    })
    .await
    .context("file-search worker failed")?
}
