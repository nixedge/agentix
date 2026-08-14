use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::Path;

use super::embed::{embed_batch, vec_literal};
use super::lhs;

const EMBED_BATCH: usize = 8;
const MAX_CHUNK_CHARS: usize = 4_000;
const MIN_CHUNK_CHARS: usize = 50;

/// Truncate `s` to at most `max_bytes` bytes, snapping back to a char boundary.
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Walk back from max_bytes until we land on a char boundary.
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".direnv",
    "result",
];

pub async fn ingest_docs(
    pool: &PgPool,
    repo_path: &Path,
    force: bool,
    repo_path_override: Option<&str>,
    project: Option<&str>,
) -> Result<()> {
    let repo_str = match repo_path_override {
        Some(s) => s.to_string(),
        None => repo_path.canonicalize()?.to_string_lossy().into_owned(),
    };

    // Discover markdown files matching doc patterns
    let candidates = collect_docs(repo_path);
    if candidates.is_empty() {
        eprintln!("No documentation files found.");
        return Ok(());
    }
    eprintln!(
        "Found {} documentation files in {}",
        candidates.len(),
        repo_str
    );

    if force {
        let n: i64 = sqlx::query_scalar(
            "WITH d AS (DELETE FROM documents WHERE repo_path = $1 RETURNING 1)
             SELECT COUNT(*) FROM d",
        )
        .bind(&repo_str)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        eprintln!("Cleared {} existing document chunks.", n);
    }

    // Load existing (source_path → (mtime_nanos, content_hash)) for incremental skip.
    struct FileInfo {
        mtime_nanos: Option<i64>,
        content_hash: String,
    }
    let existing: HashMap<String, FileInfo> = if force {
        HashMap::new()
    } else {
        let rows = sqlx::query(
            "SELECT DISTINCT ON (source_path) source_path, file_mtime, content_hash
             FROM documents
             WHERE repo_path = $1 AND content_hash IS NOT NULL
             ORDER BY source_path, chunk_index",
        )
        .bind(&repo_str)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        rows.iter()
            .map(|r| {
                use sqlx::Row as _;
                let sp: String = r.get("source_path");
                let mt: Option<i64> = r.get("file_mtime");
                let hash: String = r.get("content_hash");
                (
                    sp,
                    FileInfo {
                        mtime_nanos: mt,
                        content_hash: hash,
                    },
                )
            })
            .collect()
    };

    let bar = ProgressBar::new(candidates.len() as u64);
    bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );

    let mut pending: Vec<DocRecord> = vec![];
    let mut total_chunks = 0usize;
    let mut skipped = 0usize;
    let mut incremental_skips = 0usize;

    for (abs_path, rel_path, doc_kind) in &candidates {
        bar.set_message(
            abs_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string(),
        );

        // Cheap mtime stat — avoids reading the file at all when unchanged.
        let file_mtime: Option<i64> = std::fs::metadata(abs_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64);

        if !force {
            if let Some(info) = existing.get(rel_path) {
                if let (Some(db_mt), Some(fs_mt)) = (info.mtime_nanos, file_mtime) {
                    if db_mt == fs_mt {
                        incremental_skips += 1;
                        bar.inc(1);
                        continue;
                    }
                }
            }
        }

        let source = match std::fs::read_to_string(abs_path) {
            Ok(s) => s,
            Err(_) => {
                skipped += 1;
                bar.inc(1);
                continue;
            }
        };

        let file_hash = sha256_hex(&source);

        if !force {
            if let Some(info) = existing.get(rel_path) {
                if info.content_hash == file_hash {
                    // Mtime drifted but content unchanged — update mtime for future fast path.
                    let _ = sqlx::query(
                        "UPDATE documents SET file_mtime = $1
                         WHERE repo_path = $2 AND source_path = $3",
                    )
                    .bind(file_mtime)
                    .bind(&repo_str)
                    .bind(rel_path)
                    .execute(pool)
                    .await;
                    incremental_skips += 1;
                    bar.inc(1);
                    continue;
                }
            }
        }

        let file_stem = abs_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

        if doc_kind == "lhs_prose" {
            // Extract prose blocks from the .lhs file and emit each paragraph
            // as a separate DocRecord with an optional adjacent-code header.
            let parsed = lhs::parse_lhs(&source);
            let blocks = &parsed.blocks;
            let mut chunk_idx = 0i32;

            for (block_idx, block) in blocks.iter().enumerate() {
                if block.kind != lhs::BlockKind::Prose {
                    continue;
                }
                // Find the nearest Code block that follows this Prose block.
                let next_code = blocks[block_idx + 1..]
                    .iter()
                    .find(|b| b.kind == lhs::BlockKind::Code);

                for para in chunk_by_paragraph(&block.content) {
                    if para.content.trim().len() < MIN_CHUNK_CHARS {
                        continue;
                    }
                    let content = match next_code {
                        Some(code) => format!(
                            "[Adjacent code: lines {}-{}]\n\n{}",
                            code.start_line, code.end_line, para.content
                        ),
                        None => para.content.clone(),
                    };
                    let preview: String = content.split_whitespace().collect::<Vec<_>>().join(" ");
                    let preview = truncate_to_char_boundary(&preview, 280);
                    pending.push(DocRecord {
                        repo_path: repo_str.clone(),
                        source_path: rel_path.clone(),
                        chunk_index: chunk_idx,
                        doc_kind: doc_kind.clone(),
                        title: file_stem.to_string(),
                        content,
                        preview: preview.to_string(),
                        content_hash: file_hash.clone(),
                        file_mtime,
                        project: project.map(|s| s.to_string()),
                    });
                    chunk_idx += 1;
                    if pending.len() >= EMBED_BATCH {
                        flush_docs(&mut pending, pool, &mut total_chunks).await?;
                    }
                }
            }
            bar.inc(1);
            continue;
        }

        let title = if doc_kind == "changelog" {
            file_stem.to_string()
        } else {
            extract_title(&source, file_stem)
        };
        let chunks = if doc_kind == "changelog" {
            chunk_by_paragraph(&source)
        } else {
            chunk_markdown(&source)
        };

        for chunk in chunks {
            if chunk.content.trim().len() < MIN_CHUNK_CHARS {
                continue;
            }
            let preview: String = chunk
                .content
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let preview = truncate_to_char_boundary(&preview, 280);
            pending.push(DocRecord {
                repo_path: repo_str.clone(),
                source_path: rel_path.clone(),
                chunk_index: chunk.index,
                doc_kind: doc_kind.clone(),
                title: title.clone(),
                content: chunk.content,
                preview: preview.to_string(),
                content_hash: file_hash.clone(),
                file_mtime,
                project: project.map(|s| s.to_string()),
            });
            if pending.len() >= EMBED_BATCH {
                flush_docs(&mut pending, pool, &mut total_chunks).await?;
            }
        }
        bar.inc(1);
    }
    flush_docs(&mut pending, pool, &mut total_chunks).await?;
    bar.finish_and_clear();

    eprintln!(
        "Done. Indexed {} doc chunks ({} files unchanged, {} skipped).",
        total_chunks, incremental_skips, skipped
    );
    Ok(())
}

// ── Types ─────────────────────────────────────────────────────────────────────

struct DocRecord {
    repo_path: String,
    source_path: String,
    chunk_index: i32,
    doc_kind: String,
    title: String,
    content: String,
    preview: String,
    content_hash: String,
    file_mtime: Option<i64>,
    project: Option<String>,
}

struct MarkdownChunk {
    index: i32,
    content: String,
}

// ── File discovery ────────────────────────────────────────────────────────────

fn classify_doc(rel_path: &str) -> Option<&'static str> {
    let p = format!("/{}", rel_path.replace('\\', "/"));
    // Order matters: more specific patterns first
    if p.ends_with("/AGENTS.md")
        || p == "/AGENTS.md"
        || p.ends_with("/CLAUDE.md")
        || p == "/CLAUDE.md"
    {
        Some("agent_instruction")
    } else if p.contains("/.agent/workflows/") && p.ends_with(".md") {
        Some("workflow")
    } else if p.contains("/.agent/skills/") && p.ends_with(".md") {
        Some("skill")
    } else if p.to_lowercase().contains("/.agent/sops/") && p.ends_with(".md") {
        Some("sop")
    } else if p.contains("/.agent/plans/") && p.ends_with(".md") {
        Some("plan")
    } else if p.to_lowercase().ends_with("/.agent/readme.md") {
        Some("agent_index")
    } else if p.to_lowercase().ends_with("/readme.md") || p.to_lowercase() == "/readme.md" {
        Some("readme")
    } else {
        Some("docs")
    }
}

fn is_changelog_filename(name: &str) -> bool {
    matches!(
        name,
        "CHANGELOG"
            | "ChangeLog"
            | "CHANGES"
            | "NEWS"
            | "HISTORY"
            | "CHANGELOG.txt"
            | "CHANGES.txt"
            | "NEWS.txt"
            | "HISTORY.txt"
            | "CHANGELOG.rst"
            | "CHANGES.rst"
            | "NEWS.rst"
            | "HISTORY.rst"
    )
}

fn collect_docs(repo_path: &Path) -> Vec<(std::path::PathBuf, String, String)> {
    let mut result = vec![];

    let walker = ignore::WalkBuilder::new(repo_path)
        .standard_filters(true)
        .hidden(false)
        .filter_entry(|e| {
            let name = e.file_name().to_str().unwrap_or("");
            !SKIP_DIRS.contains(&name)
        })
        .build();

    for entry in walker.flatten() {
        let path = entry.path().to_path_buf();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let rel = path
            .strip_prefix(repo_path)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();

        let kind: Option<String> = if ext == "lhs" {
            Some("lhs_prose".to_string())
        } else if is_changelog_filename(filename) {
            Some("changelog".to_string())
        } else if ext == "md" {
            classify_doc(&rel).map(|k| k.to_string())
        } else {
            None
        };

        if let Some(k) = kind {
            result.push((path, rel, k));
        }
    }
    result
}

// ── Chunking ──────────────────────────────────────────────────────────────────

fn extract_title(text: &str, fallback: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if let Some(title) = line.strip_prefix("# ") {
            return title.trim().to_string();
        }
    }
    fallback.to_string()
}

/// Split plain text into paragraph-sized chunks, separated by blank lines.
/// Each chunk is trimmed and truncated to [`MAX_CHUNK_CHARS`].
fn chunk_by_paragraph(text: &str) -> Vec<MarkdownChunk> {
    let mut chunks = Vec::new();
    let mut idx = 0i32;
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        let content = truncate_to_char_boundary(para, MAX_CHUNK_CHARS);
        chunks.push(MarkdownChunk {
            index: idx,
            content: content.to_string(),
        });
        idx += 1;
    }
    chunks
}

fn chunk_markdown(source: &str) -> Vec<MarkdownChunk> {
    if source.len() <= MAX_CHUNK_CHARS {
        return vec![MarkdownChunk {
            index: 0,
            content: source.to_string(),
        }];
    }

    // Split on H2 headings
    let sections = split_on_heading(source, "## ");
    let mut chunks = vec![];
    let mut idx = 0i32;

    for section in sections {
        if section.trim().is_empty() {
            continue;
        }
        if section.len() <= MAX_CHUNK_CHARS {
            chunks.push(MarkdownChunk {
                index: idx,
                content: section.trim_end().to_string(),
            });
            idx += 1;
        } else {
            // Further split on H3
            for sub in split_on_heading(&section, "### ") {
                if sub.trim().is_empty() {
                    continue;
                }
                let content = truncate_to_char_boundary(&sub, MAX_CHUNK_CHARS);
                chunks.push(MarkdownChunk {
                    index: idx,
                    content: content.trim_end().to_string(),
                });
                idx += 1;
            }
        }
    }

    if chunks.is_empty() {
        chunks.push(MarkdownChunk {
            index: 0,
            content: truncate_to_char_boundary(source, MAX_CHUNK_CHARS).to_string(),
        });
    }
    chunks
}

/// Split text on lines that start with `prefix`, keeping prefix with each section.
fn split_on_heading(text: &str, prefix: &str) -> Vec<String> {
    let mut sections = vec![];
    let mut current = String::new();
    for line in text.lines() {
        if line.starts_with(prefix) && !current.trim().is_empty() {
            sections.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        sections.push(current);
    }
    if sections.is_empty() {
        sections.push(text.to_string());
    }
    sections
}

// ── Flush to DB ───────────────────────────────────────────────────────────────

async fn flush_docs(pending: &mut Vec<DocRecord>, pool: &PgPool, total: &mut usize) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let texts: Vec<&str> = pending.iter().map(|d| d.content.as_str()).collect();
    let embeddings = embed_batch(&texts).await?;

    for (rec, emb) in pending.iter().zip(embeddings.iter()) {
        let project_arr: Option<Vec<String>> = rec.project.as_ref().map(|p| vec![p.clone()]);
        sqlx::query(
            "INSERT INTO documents
                 (repo_path, source_path, chunk_index, doc_kind, title,
                  content, preview, content_hash, file_mtime, embedding, project)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::vector, $11::text[])
             ON CONFLICT (repo_path, source_path, chunk_index) DO UPDATE
                 SET doc_kind     = EXCLUDED.doc_kind,
                     title        = EXCLUDED.title,
                     content      = EXCLUDED.content,
                     preview      = EXCLUDED.preview,
                     content_hash = EXCLUDED.content_hash,
                     file_mtime   = EXCLUDED.file_mtime,
                     embedding    = EXCLUDED.embedding,
                     project      = (SELECT array_agg(DISTINCT p)
                                     FROM unnest(coalesce(documents.project, '{}'::text[])
                                              || coalesce(EXCLUDED.project, '{}'::text[])) p),
                     indexed_at   = NOW()",
        )
        .bind(&rec.repo_path)
        .bind(&rec.source_path)
        .bind(rec.chunk_index)
        .bind(&rec.doc_kind)
        .bind(&rec.title)
        .bind(&rec.content)
        .bind(&rec.preview)
        .bind(&rec.content_hash)
        .bind(rec.file_mtime)
        .bind(vec_literal(emb))
        .bind(project_arr)
        .execute(pool)
        .await?;
    }

    *total += pending.len();
    pending.clear();
    Ok(())
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── chunk_by_paragraph ─────────────────────────────────────────────────────

    #[test]
    fn paragraph_split_on_blank_line() {
        let text = "First paragraph.\n\nSecond paragraph.\n";
        let chunks = chunk_by_paragraph(text);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].content.contains("First"));
        assert!(chunks[1].content.contains("Second"));
    }

    #[test]
    fn paragraph_empty_text_produces_no_chunks() {
        assert!(chunk_by_paragraph("").is_empty());
    }

    // ── lhs_prose extraction ───────────────────────────────────────────────────

    /// Simulate the prose extraction logic used in `ingest_docs` for lhs_prose,
    /// without touching the database.
    fn extract_prose_records(lhs_source: &str) -> Vec<(String, String)> {
        let parsed = lhs::parse_lhs(lhs_source);
        let blocks = &parsed.blocks;
        let mut records: Vec<(String, String)> = Vec::new(); // (doc_kind, content)

        for (block_idx, block) in blocks.iter().enumerate() {
            if block.kind != lhs::BlockKind::Prose {
                continue;
            }
            let next_code = blocks[block_idx + 1..]
                .iter()
                .find(|b| b.kind == lhs::BlockKind::Code);

            for para in chunk_by_paragraph(&block.content) {
                if para.content.trim().len() < MIN_CHUNK_CHARS {
                    continue;
                }
                let content = match next_code {
                    Some(code) => format!(
                        "[Adjacent code: lines {}-{}]\n\n{}",
                        code.start_line, code.end_line, para.content
                    ),
                    None => para.content.clone(),
                };
                records.push(("lhs_prose".to_string(), content));
            }
        }
        records
    }

    #[test]
    fn bird_prose_chunks_have_lhs_prose_kind_and_no_code_prefix() {
        // T023: Bird-style prose paragraphs become lhs_prose records without `> `.
        let src = concat!(
            "First prose paragraph about the module.\n",
            "It spans multiple lines for context.\n",
            "\n",
            "> myFunc = 42\n",
            "\n",
            "Second prose paragraph after the code.\n",
            "This also spans multiple lines for the test.\n",
        );
        let records = extract_prose_records(src);
        assert!(
            !records.is_empty(),
            "expected at least one lhs_prose record"
        );
        for (kind, content) in &records {
            assert_eq!(kind, "lhs_prose");
            assert!(
                !content.contains("> "),
                "prose record contains code prefix '> ': {content:?}"
            );
        }
    }

    #[test]
    fn latex_prose_chunk_carries_adjacent_code_header() {
        // T026: Prose blocks in LaTeX-style files get an [Adjacent code: lines N-M] header.
        let src = concat!(
            "This is an introductory prose paragraph that describes the module.\n",
            "It is long enough to exceed MIN_CHUNK_CHARS for the test to work.\n",
            "\\begin{code}\n",
            "module Foo where\n",
            "foo = 1\n",
            "\\end{code}\n",
            "Trailing prose after the code block.\n",
            "Also long enough to be indexed as a separate chunk here.\n",
        );
        let records = extract_prose_records(src);

        // The first prose record (before the code block) should carry the header.
        let first = records
            .iter()
            .find(|(_, c)| c.starts_with("[Adjacent code:"))
            .expect("expected at least one record with [Adjacent code:] header");
        assert!(
            first.1.contains("[Adjacent code: lines"),
            "header not found: {:?}",
            first.1
        );
    }

    #[test]
    fn trailing_prose_has_no_adjacent_code_header() {
        // A prose block at end of file (no following code) must not get the header.
        let src = concat!(
            "\\begin{code}\n",
            "module Bar where\n",
            "bar = 2\n",
            "\\end{code}\n",
            "This is trailing prose with no code block following it anywhere.\n",
            "It should be emitted without an [Adjacent code:] header.\n",
        );
        let records = extract_prose_records(src);
        let trailing = records.last().expect("expected at least one record");
        assert!(
            !trailing.1.starts_with("[Adjacent code:"),
            "trailing prose should not have adjacent-code header: {:?}",
            trailing.1
        );
    }

    // ── is_changelog_filename ──────────────────────────────────────────────────

    #[test]
    fn changelog_names_recognized() {
        for name in &[
            "CHANGELOG",
            "ChangeLog",
            "CHANGES",
            "NEWS",
            "HISTORY",
            "CHANGELOG.txt",
            "CHANGES.rst",
        ] {
            assert!(
                is_changelog_filename(name),
                "{name:?} should be recognized as a changelog"
            );
        }
    }

    #[test]
    fn non_changelog_names_rejected() {
        for name in &["README.md", "NOTES.txt", "changelog.json"] {
            assert!(
                !is_changelog_filename(name),
                "{name:?} should NOT be recognized as a changelog"
            );
        }
    }
}
