use agentix_store::Store;
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

use super::embed::ensure_embed_model;
use super::lhs;
use super::symbols::extract_symbols;

const CHUNK_LINES: usize = 120;
const OVERLAP_LINES: usize = 15;
const MAX_CHUNK_BYTES: usize = 8_000;
const MAX_CHUNKS_PER_FILE: usize = 256;

const CODE_EXTENSIONS: &[&str] = &[
    "hs", "lhs", "hsc", "chs", "rs", "py", "ts", "tsx", "js", "jsx", "nix", "go", "java", "scala",
    "ml", "mli", "c", "cpp", "h", "hpp", "sql", "sh", "toml", "yaml", "yml", "json", "tex",
    "cabal",
];

const SKIP_DIRS: &[&str] = &[
    "target",
    "dist",
    "build",
    "__pycache__",
    ".stack-work",
    "vendor",
    ".cache",
    "result",
    ".direnv",
    "coverage",
    ".next",
    "out",
];

pub async fn ingest_code(
    pool: &Store,
    repo_path: &Path,
    force: bool,
    extra_patterns: &[String],
    repo_path_override: Option<&str>,
    project: Option<&str>,
) -> Result<()> {
    let repo_str = match repo_path_override {
        Some(s) => s.to_owned(),
        None => repo_path.canonicalize()?.to_string_lossy().into_owned(),
    };
    ensure_embed_model().await?;
    let files = collect_files(repo_path, extra_patterns);
    let old = pool
        .rows("code_chunks", &agentix_store::repo_filter(&repo_str))
        .await?;
    let mut existing: HashMap<String, serde_json::Value> = HashMap::new();
    for row in old {
        if let Some(file) = row["file_path"].as_str() {
            existing.entry(file.into()).or_insert(row);
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut total = 0;
    let mut pending = Vec::new();
    let mut pending_chunks = 0;
    let mut unchanged = 0;
    for file in files {
        let relative = file
            .strip_prefix(repo_path)
            .unwrap_or(&file)
            .to_string_lossy()
            .into_owned();
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
                pool.retag_file("code_chunks", &repo_str, &relative, project, mtime)
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
            pool.retag_file("code_chunks", &repo_str, &relative, project, mtime)
                .await?;
            unchanged += 1;
            continue;
        }
        let language = detect_language(&file);
        let filename = file
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let chunks =
            if file.extension().is_some_and(|e| e == "lhs") || filename.ends_with(".lhs-boot") {
                make_lhs_chunks(&source)
            } else {
                make_chunks(&source, &language)
            };
        let records=chunks.into_iter().map(|chunk|agentix_store::Record{row:serde_json::json!({
            "repo_path":repo_str,"file_path":relative,"chunk_index":chunk.index,"content":chunk.content,
            "start_line":chunk.start_line,"end_line":chunk.end_line,"language":language,"symbol_kind":chunk.symbol_kind,
            "content_hash":hash,"file_mtime":mtime,"project":project.into_iter().collect::<Vec<_>>(),"indexed_at":chrono::Utc::now().to_rfc3339()
        }),embedding:None}).collect::<Vec<_>>();
        pending_chunks += records.len();
        pending.push((relative, records));
        if pending_chunks >= 16 {
            total +=
                super::embed::flush_files(pool, "code_chunks", &repo_str, &mut pending).await?;
            pending_chunks = 0;
        }
    }
    total += super::embed::flush_files(pool, "code_chunks", &repo_str, &mut pending).await?;
    for file in existing.keys() {
        if !seen.contains(file) && (force || !repo_path.join(file).exists()) {
            pool.replace_file("code_chunks", &repo_str, file, vec![])
                .await?;
        }
    }
    pool.maintain_if_needed("code_chunks").await?;
    eprintln!("Done. Indexed {total} chunks ({unchanged} files unchanged).");
    Ok(())
}

struct Chunk {
    index: i32,
    content: String,
    start_line: i32,
    end_line: i32,
    symbol_kind: Option<String>,
}

// ── File collection ───────────────────────────────────────────────────────────

fn collect_files(repo_path: &Path, extra_patterns: &[String]) -> Vec<std::path::PathBuf> {
    let mut files = vec![];

    let walker = ignore::WalkBuilder::new(repo_path)
        .standard_filters(true) // respects .gitignore, .ignore
        .hidden(false)
        .filter_entry(|entry| {
            let name = entry.file_name().to_str().unwrap_or("");
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
        let is_cabal_project = filename == "cabal.project"
            || filename == "cabal.project.freeze"
            || filename == "cabal.project.local";
        let is_boot = filename.ends_with(".hs-boot") || filename.ends_with(".lhs-boot");

        let matches = CODE_EXTENSIONS.contains(&ext.as_str())
            || is_cabal_project
            || is_boot
            || extra_patterns.iter().any(|p| {
                glob::Pattern::new(p)
                    .map(|pat| pat.matches_path(&path))
                    .unwrap_or(false)
            });

        if matches {
            files.push(path);
        }
    }
    files
}

// ── Language detection ────────────────────────────────────────────────────────

fn detect_language(path: &Path) -> String {
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if filename == "cabal.project"
        || filename == "cabal.project.freeze"
        || filename == "cabal.project.local"
    {
        return "cabal".to_string();
    }
    if filename.ends_with(".hs-boot") || filename.ends_with(".lhs-boot") {
        return "haskell".to_string();
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "hs" | "lhs" | "hsc" | "chs" => "haskell",
        "rs" => "rust",
        "py" => "python",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "nix" => "nix",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "scala" => "scala",
        "ml" | "mli" => "ocaml",
        "c" | "h" => "c",
        "cpp" | "hpp" => "cpp",
        "sql" => "sql",
        "tex" => "latex",
        "sh" => "bash",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        "cabal" => "cabal",
        _ => "text",
    }
    .to_string()
}

// ── Chunking ──────────────────────────────────────────────────────────────────

const SYMBOL_LANGUAGES: &[&str] = &[
    "typescript",
    "javascript",
    "python",
    "rust",
    "haskell",
    "latex",
    "nix",
    "c",
    "cpp",
    "java",
    "kotlin",
];

fn make_chunks(source: &str, language: &str) -> Vec<Chunk> {
    if !SYMBOL_LANGUAGES.contains(&language) {
        return chunk_lines(source);
    }

    let syms = extract_symbols(source, language);
    if syms.is_empty() {
        return chunk_lines(source);
    }

    let total_lines = source.lines().count();
    let max_covered = syms.iter().map(|s| s.end_line).max().unwrap_or(0);

    let mut chunks: Vec<Chunk> = syms
        .into_iter()
        .take(MAX_CHUNKS_PER_FILE)
        .enumerate()
        .map(|(i, s)| Chunk {
            index: i as i32,
            content: s.content,
            start_line: s.start_line as i32,
            end_line: s.end_line as i32,
            symbol_kind: Some(s.kind),
        })
        .collect();

    // If the parser only covered a fraction of the file (e.g. tikzpicture or
    // other constructs that confuse the grammar), fill in the uncovered tail
    // with overlapping line windows so nothing is silently dropped.
    if max_covered + CHUNK_LINES * 2 < total_lines && chunks.len() < MAX_CHUNKS_PER_FILE {
        let lines: Vec<&str> = source.lines().collect();
        let step = CHUNK_LINES.saturating_sub(OVERLAP_LINES).max(1);
        let start_from = max_covered.saturating_sub(OVERLAP_LINES);
        let mut i = start_from;
        let mut idx = chunks.len() as i32;
        while i < total_lines && chunks.len() < MAX_CHUNKS_PER_FILE {
            let end = (i + CHUNK_LINES).min(total_lines);
            let text = lines[i..end].join("\n");
            if !text.trim().is_empty() && text.len() <= MAX_CHUNK_BYTES {
                chunks.push(Chunk {
                    index: idx,
                    content: text,
                    start_line: (i + 1) as i32,
                    end_line: end as i32,
                    symbol_kind: None,
                });
                idx += 1;
            }
            i += step;
        }
    }

    chunks
}

fn chunk_lines(source: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = source.lines().collect();
    let step = CHUNK_LINES.saturating_sub(OVERLAP_LINES).max(1);
    let mut chunks = vec![];

    let mut i = 0usize;
    let mut chunk_idx = 0i32;
    while i < lines.len() && chunks.len() < MAX_CHUNKS_PER_FILE {
        let end = (i + CHUNK_LINES).min(lines.len());
        let text = lines[i..end].join("\n");
        if text.trim().is_empty() || text.len() > MAX_CHUNK_BYTES {
            i += step;
            continue;
        }
        chunks.push(Chunk {
            index: chunk_idx,
            content: text,
            start_line: (i + 1) as i32,
            end_line: end as i32,
            symbol_kind: None,
        });
        chunk_idx += 1;
        if end >= lines.len() {
            break;
        }
        i += step;
    }
    chunks
}

// ── Literate Haskell pre-processor ────────────────────────────────────────────

/// Strip Literate Haskell markup and return code chunks with line numbers
/// translated back to the original `.lhs` file coordinates.
fn make_lhs_chunks(source: &str) -> Vec<Chunk> {
    let parsed = lhs::parse_lhs(source);
    let code_blocks: Vec<&lhs::LhsBlock> = parsed.code_blocks().collect();
    if code_blocks.is_empty() {
        return vec![];
    }

    // Concatenate all code blocks into a single Haskell source string for
    // tree-sitter. Blocks are separated by a single newline so relative line
    // offsets within each block are preserved.
    let stripped: String = code_blocks
        .iter()
        .map(|b| b.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let line_map = lhs::LineMap::from_code_blocks(&code_blocks);
    let mut chunks = make_chunks(&stripped, "haskell");

    // Translate tree-sitter line numbers (relative to the concatenated buffer)
    // back to original file line numbers.
    for chunk in &mut chunks {
        chunk.start_line = line_map.file_line(chunk.start_line as usize) as i32;
        chunk.end_line = line_map.file_line(chunk.end_line as usize) as i32;
    }

    chunks
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── chunk_lines ────────────────────────────────────────────────────────────

    #[test]
    fn chunk_lines_empty() {
        assert!(chunk_lines("").is_empty());
    }

    #[test]
    fn chunk_lines_short_file() {
        let src = "line1\nline2\nline3\n";
        let chunks = chunk_lines(src);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 3);
        assert!(chunks[0].content.contains("line1"));
        assert!(chunks[0].content.contains("line3"));
    }

    #[test]
    fn chunk_lines_respects_cap() {
        // Build a file with more lines than MAX_CHUNKS_PER_FILE * CHUNK_LINES
        // so we would exceed the cap if it weren't enforced.
        let line = "x = 1\n";
        let n = MAX_CHUNKS_PER_FILE * CHUNK_LINES + 200;
        let src = line.repeat(n);
        let chunks = chunk_lines(&src);
        assert!(
            chunks.len() <= MAX_CHUNKS_PER_FILE,
            "got {} chunks, expected <= {}",
            chunks.len(),
            MAX_CHUNKS_PER_FILE
        );
    }

    #[test]
    fn chunk_lines_overlap_means_consecutive_chunks_share_lines() {
        let lines: Vec<String> = (1..=300).map(|i| format!("line{i}")).collect();
        let src = lines.join("\n");
        let chunks = chunk_lines(&src);
        assert!(chunks.len() >= 2, "expected multiple chunks");
        // The end of chunk 0 should be >= the start of chunk 1 (overlap).
        assert!(
            chunks[0].end_line >= chunks[1].start_line,
            "no overlap between chunk 0 (end {}) and chunk 1 (start {})",
            chunks[0].end_line,
            chunks[1].start_line
        );
    }

    // ── make_chunks ────────────────────────────────────────────────────────────

    #[test]
    fn make_chunks_unknown_language_falls_back_to_lines() {
        let src = "hello\nworld\n";
        let chunks = make_chunks(src, "cobol");
        assert!(!chunks.is_empty());
        assert!(chunks[0].symbol_kind.is_none());
    }

    #[test]
    fn make_chunks_haskell_extracts_instances() {
        let src = r#"module Foo where

data Color = Red | Green | Blue

instance Show Color where
    show Red   = "Red"
    show Green = "Green"
    show Blue  = "Blue"
"#;
        let chunks = make_chunks(src, "haskell");
        let has_instance = chunks
            .iter()
            .any(|c| c.symbol_kind.as_deref() == Some("impl") && c.content.contains("Show Color"));
        assert!(
            has_instance,
            "expected an impl chunk for 'instance Show Color'"
        );
    }

    // ── make_lhs_chunks ────────────────────────────────────────────────────────

    #[test]
    fn make_lhs_chunks_bird_strips_prefix() {
        let src = "> myFunc = 42\n\nsome prose line\n";
        let chunks = make_lhs_chunks(src);
        // All returned chunks must not contain the `> ` prefix.
        for chunk in &chunks {
            assert!(
                !chunk.content.contains("> "),
                "chunk still contains '> ' prefix: {:?}",
                chunk.content
            );
        }
        // At least one chunk should exist (from the code line).
        assert!(!chunks.is_empty());
    }

    #[test]
    fn make_lhs_chunks_latex_strips_delimiters_and_prose() {
        let src = "some prose\n\\begin{code}\nmyFunc = 42\n\\end{code}\nmore prose\n";
        let chunks = make_lhs_chunks(src);
        // The only code content is `myFunc = 42`.
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(
                !chunk.content.contains("begin{code}"),
                "chunk contains LaTeX delimiter: {:?}",
                chunk.content
            );
            assert!(
                !chunk.content.contains("some prose"),
                "chunk contains prose: {:?}",
                chunk.content
            );
        }
    }

    #[test]
    fn make_lhs_chunks_no_code_returns_empty() {
        let src = "This file has only prose.\nNo code lines at all.\n";
        let chunks = make_lhs_chunks(src);
        assert!(chunks.is_empty());
    }

    #[test]
    fn make_chunks_haskell_cpp_instance_is_extracted() {
        // An instance declaration wrapped in a CPP conditional block should not
        // be silently dropped by the Haskell symbol extractor.
        let src = r#"module Foo where

#if MIN_VERSION_base(4,10,0)
instance Show Foo where
    show _ = "Foo (new)"
#else
instance Show Foo where
    show _ = "Foo (old)"
#endif
"#;
        let chunks = make_chunks(src, "haskell");
        let has_instance = chunks
            .iter()
            .any(|c| c.symbol_kind.as_deref() == Some("impl") && c.content.contains("Show Foo"));
        assert!(
            has_instance,
            "instance inside CPP block was silently dropped"
        );
    }
}
