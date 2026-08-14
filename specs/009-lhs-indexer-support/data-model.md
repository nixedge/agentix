# Data Model: 009-lhs-indexer-support

## Existing Tables (unchanged schema)

### `code_chunks`
Stores indexed source code. `.lhs` code blocks will be inserted here with `language = "haskell"`.

Key columns:
- `repo_path` — repository identifier
- `file_path` — relative path to the source file (e.g., `src/Foo.lhs`)
- `chunk_index` — ordering index within the file
- `content` — stripped Haskell source (Bird prefix removed / LaTeX delimiters removed)
- `start_line`, `end_line` — **original** `.lhs` file line numbers (not renumbered after stripping)
- `language` — `"haskell"` for all `.lhs` files
- `symbol_kind` — `"function"`, `"type"`, `"impl"`, etc. from tree-sitter; or `NULL` for line-window fallback chunks
- `content_hash` — SHA-256 of the raw (unstripped) `.lhs` file content
- `file_mtime` — mtime for incremental skip

### `documents`
Stores indexed documentation. `.lhs` prose blocks will be inserted here with `doc_kind = "lhs_prose"`.

Key columns:
- `repo_path` — repository identifier
- `source_path` — relative path to the `.lhs` file (same file as the code chunks)
- `chunk_index` — paragraph/section ordering index
- `doc_kind` — `"lhs_prose"` (new value; no schema change required — column is `text`)
- `title` — filename stem (e.g., `ObserveOutcome` from `ObserveOutcome.lhs`)
- `content` — raw prose text (LaTeX markup preserved as-is)
- `preview` — whitespace-collapsed first 280 characters
- `content_hash` — SHA-256 of the raw `.lhs` file content
- `file_mtime` — mtime for incremental skip

No schema migrations required — both tables already exist and the new data fits within the existing column types.

---

## New In-Memory Types (Rust, `src/ingest/lhs.rs`)

### `LhsStyle`

```
LhsStyle = Bird | LaTeX
```

- `Bird`: detected when file contains no `\begin{code}` marker
- `LaTeX`: detected when any `\begin{code}` marker is present

### `LhsBlock`

A contiguous run of lines with a uniform type.

```
LhsBlock {
    kind:       BlockKind,   // Code | Prose
    content:    String,      // Bird: prefix stripped; LaTeX: delimiter lines excluded
    start_line: usize,       // 1-indexed, original .lhs file line
    end_line:   usize,       // 1-indexed, original .lhs file line
}

BlockKind = Code | Prose
```

### `ParsedLhs`

Result of running the pre-processor on one `.lhs` file.

```
ParsedLhs {
    style:  LhsStyle,
    blocks: Vec<LhsBlock>,
}
```

- `code_blocks()` — iterator over `LhsBlock` where `kind == Code`
- `prose_blocks()` — iterator over `LhsBlock` where `kind == Prose`
- `has_code()` — whether any code blocks exist

---

## Line Number Mapping

For symbol extraction, tree-sitter receives the concatenated code text (all `Code` blocks joined). tree-sitter returns symbol positions relative to that concatenated buffer. To recover original file line numbers:

```
LineMap {
    entries: Vec<LineMapEntry>,
}

LineMapEntry {
    code_line:   usize,   // line number in concatenated code buffer (1-indexed)
    file_line:   usize,   // corresponding line in original .lhs file (1-indexed)
}
```

Built during pre-processing: for each code line emitted, record the mapping. Given a tree-sitter `start_line` from a symbol, binary-search `LineMap.entries` by `code_line` to find the original `file_line`.

---

## Extension Additions (no new types)

The following extensions require only additions to constants in `code.rs` and `detect_language()`, with no new types:

| Extension | `CODE_EXTENSIONS` entry | `detect_language()` arm | Notes |
|-----------|------------------------|------------------------|-------|
| `lhs` | `"lhs"` | `"lhs" => "haskell"` | Pre-processor runs before chunking |
| `hsc` | `"hsc"` | `"hsc" => "haskell"` | No pre-processing; tree-sitter fallback handles directives |
| `chs` | `"chs"` | `"chs" => "haskell"` | No pre-processing; tree-sitter fallback handles `{# #}` hooks |
| `hs-boot` | special-case filename | `"haskell"` | `.lhs-boot` also needs the LHS pre-processor |

### `.hs-boot` / `.lhs-boot` Discovery

`Path::extension()` returns `"boot"` for `Foo.hs-boot` — the extension-based lookup sees `"boot"`, not `"hs-boot"`. These require the same filename-based special-casing used for `cabal.project`:

```rust
// In collect_files():
let is_boot = filename.ends_with(".hs-boot") || filename.ends_with(".lhs-boot");
```

---

## Changelog Discovery

New filenames to special-case in `collect_docs()` in `docs.rs`:

```rust
// Recognized changelog filenames (case-sensitive, common conventions):
const CHANGELOG_NAMES: &[&str] = &[
    "CHANGELOG", "ChangeLog", "CHANGES", "NEWS", "HISTORY",
    "CHANGELOG.txt", "CHANGES.txt", "NEWS.txt",
    "CHANGELOG.rst", "CHANGES.rst",
];
```

`doc_kind = "changelog"` for all of these. Chunk by paragraph (blank-line splitting) rather than Markdown heading splitting, since `.txt` / `.rst` files don't use `##` headings.
