use agentix_store::Store;
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

use super::lhs;

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
    pool: &Store,
    repo_path: &Path,
    force: bool,
    repo_path_override: Option<&str>,
    project: Option<&str>,
) -> Result<()> {
    let repo_str = match repo_path_override {
        Some(s) => s.to_owned(),
        None => repo_path.canonicalize()?.to_string_lossy().into_owned(),
    };
    let files = collect_docs(repo_path);
    let mut existing: HashMap<String, serde_json::Value> = HashMap::new();
    for row in pool
        .rows("documents", &agentix_store::repo_filter(&repo_str))
        .await?
    {
        if let Some(file) = row["source_path"].as_str() {
            existing.entry(file.into()).or_insert(row);
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut total = 0;
    let mut pending = Vec::new();
    let mut pending_chunks = 0;
    let mut unchanged = 0;
    for (file, relative, kind) in files {
        seen.insert(relative.clone());
        let mtime = std::fs::metadata(&file)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64);
        let previous = existing.get(&relative);
        let same_mtime =
            mtime.is_some() && previous.is_some_and(|p| p["file_mtime"].as_i64() == mtime);
        let needs_tag = project.is_some_and(|p| {
            !previous.is_some_and(|r| {
                r["project"]
                    .as_array()
                    .is_some_and(|tags| tags.iter().any(|t| t == p))
            })
        });
        if !force && same_mtime {
            if needs_tag {
                pool.retag_file("documents", &repo_str, &relative, project, mtime)
                    .await?;
            }
            unchanged += 1;
            continue;
        }
        let source = match std::fs::read_to_string(&file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Skipping {}: {e}", file.display());
                continue;
            }
        };
        let hash = sha256_hex(&source);
        if !force && previous.is_some_and(|p| p["content_hash"].as_str() == Some(hash.as_str())) {
            pool.retag_file("documents", &repo_str, &relative, project, mtime)
                .await?;
            unchanged += 1;
            continue;
        }
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let title = if kind == "changelog" || kind == "lhs_prose" {
            stem.to_owned()
        } else {
            extract_title(&source, stem)
        };
        let mut chunks = Vec::new();
        if kind == "lhs_prose" {
            let parsed = lhs::parse_lhs(&source);
            for (i, block) in parsed.blocks.iter().enumerate() {
                if block.kind != lhs::BlockKind::Prose {
                    continue;
                }
                let next = parsed.blocks[i + 1..]
                    .iter()
                    .find(|b| b.kind == lhs::BlockKind::Code);
                for paragraph in chunk_by_paragraph(&block.content) {
                    if paragraph.content.trim().len() < MIN_CHUNK_CHARS {
                        continue;
                    }
                    let content = if let Some(code) = next {
                        format!(
                            "[Adjacent code: lines {}-{}]\n\n{}",
                            code.start_line, code.end_line, paragraph.content
                        )
                    } else {
                        paragraph.content
                    };
                    chunks.push(MarkdownChunk {
                        index: chunks.len() as i32,
                        content,
                    });
                }
            }
        } else if kind == "changelog" {
            chunks = chunk_by_paragraph(&source);
        } else {
            chunks = chunk_markdown(&source);
        }
        let records=chunks.into_iter().filter(|c|c.content.trim().len()>=MIN_CHUNK_CHARS).map(|chunk|{
            let preview=chunk.content.split_whitespace().collect::<Vec<_>>().join(" ");
            agentix_store::Record{row:serde_json::json!({"repo_path":repo_str,"source_path":relative,"chunk_index":chunk.index,
                "doc_kind":kind,"title":title,"content":chunk.content,"preview":truncate_to_char_boundary(&preview,280),
                "content_hash":hash,"file_mtime":mtime,"project":project.into_iter().collect::<Vec<_>>(),"indexed_at":chrono::Utc::now().to_rfc3339()}),embedding:None}
        }).collect::<Vec<_>>();
        pending_chunks += records.len();
        pending.push((relative, records));
        if pending_chunks >= 16 {
            total += super::embed::flush_files(pool, "documents", &repo_str, &mut pending).await?;
            pending_chunks = 0;
        }
    }
    total += super::embed::flush_files(pool, "documents", &repo_str, &mut pending).await?;
    for file in existing.keys() {
        if !seen.contains(file) && (force || !repo_path.join(file).exists()) {
            pool.replace_file("documents", &repo_str, file, vec![])
                .await?;
        }
    }
    pool.maintain_if_needed("documents").await?;
    eprintln!("Done. Indexed {total} doc chunks ({unchanged} files unchanged).");
    Ok(())
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
        assert!(
            records
                .iter()
                .any(|(_, content)| content.starts_with("[Adjacent code: lines")),
            "expected a prose record with an adjacent-code line header"
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
        assert!(
            records
                .last()
                .is_some_and(|(_, content)| !content.starts_with("[Adjacent code:")),
            "expected trailing prose without an adjacent-code header"
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
