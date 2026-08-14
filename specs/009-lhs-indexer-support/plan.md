# Implementation Plan: Literate Haskell (.lhs) Indexer Support

**Branch**: `009-lhs-indexer-support` | **Date**: 2026-08-13 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/009-lhs-indexer-support/spec.md`

## Summary

Add `.lhs` (Literate Haskell Source) support to the agentic-nix code indexer. The fix has two parts: (1) a pre-processor (`src/ingest/lhs.rs`) that strips Bird-style and LaTeX-style Literate Haskell markup, sending code to `code_chunks` and prose to `documents`; and (2) trivial extension additions for `.hsc`, `.chs`, `.hs-boot`, and Changelog files that require no pre-processing. All changes are in `src/ingest/` of the agentic-nix codebase.

## Technical Context

**Language/Version**: Rust 2021 edition (same as agentix workspace)
**Primary Dependencies**: `tree_sitter_haskell` (symbol extraction), `sqlx` (PostgreSQL), `ignore` (file walking with .gitignore support)
**Storage**: PostgreSQL — `code_chunks` table (code) and `documents` table (prose); no schema migrations required
**Testing**: `cargo test -p agentic-nix` (unit tests for `lhs.rs`); integration tests using small fixture `.lhs` files
**Target Platform**: Linux server (same as agentix daemon)
**Project Type**: Rust library/binary (ingest pipeline binary)
**Performance Goals**: Incremental indexing — mtime/hash skip means re-indexing a large repo after `.lhs` support is added only processes new/changed files
**Constraints**: tree-sitter-haskell does NOT parse raw `.lhs` source; stripping is mandatory before symbol extraction
**Key Files**:
- `src/ingest/code.rs` — `CODE_EXTENSIONS`, `detect_language()`, `make_chunks()`, `collect_files()`
- `src/ingest/docs.rs` — `collect_docs()`, `chunk_markdown()`, `classify_doc()`
- `src/ingest/lhs.rs` — **new module** (pre-processor)
- `src/ingest/symbols.rs` — `extract_haskell_symbols()` (no changes required)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked post-design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | ✓ PASS | Changes are inside the ingest library. No new network surface. |
| II. Local-First | N/A | No inference routing involved. |
| III. Reproducible Environments | ✓ PASS | No new Nix dependencies (tree-sitter-haskell already in flake). Must verify `nix build` passes. |
| IV. Isolation by Default | N/A | No agent execution changes. |
| V. Layered API | ✓ PASS | MCP tool contracts unchanged (additive only). No breaking changes. |
| VI. Comprehensive Testing | ✓ PASS | Unit tests for `lhs.rs` parser; fixture `.lhs` files for integration tests (must be small, pinned). |
| VII. Formal State Machine | N/A | No agent loop changes. |
| VIII. Code Quality Gates | ✓ PASS | Must pass `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace`. |

**No violations. No Complexity Tracking table required.**

## Project Structure

### Documentation (this feature)

```text
specs/009-lhs-indexer-support/
├── plan.md              # This file
├── research.md          # Phase 0 output — root cause, architecture decisions, additional file types
├── data-model.md        # Phase 1 output — in-memory types, table mapping, extension additions
├── contracts/
│   └── mcp-search-tools.md  # Phase 1 output — search_code / search_docs contract deltas
└── tasks.md             # Phase 2 output (/speckit.tasks — not yet created)
```

### Source Code (agentic-nix codebase at `/home/sam/home/agentic-nix`)

```text
src/ingest/
├── code.rs           # Modify: CODE_EXTENSIONS, detect_language(), collect_files(), make_chunks()
├── docs.rs           # Modify: collect_docs() — .lhs prose + changelog filenames
├── lhs.rs            # New: LhsStyle, LhsBlock, ParsedLhs, parse_lhs(), LineMap
└── symbols.rs        # No changes

tests/fixtures/
├── bird_style.lhs    # New: small Bird-style fixture (< 50 lines)
├── latex_style.lhs   # New: small LaTeX-style fixture (< 50 lines)
└── ffi_binding.hsc   # New: small .hsc fixture for regression test
```

**Structure Decision**: Single-project layout. All changes in `src/ingest/`. No new crates, no new workspace members.

## Phase 0: Research — Complete

See [research.md](research.md) for full findings. Key decisions:

1. **Root cause confirmed**: `"lhs"` missing from `CODE_EXTENSIONS` + no markup stripping
2. **Architecture**: new `lhs.rs` pre-processor feeds both `code_chunks` and `documents`
3. **Style detection**: LaTeX-style if any `\begin{code}` present; Bird-style otherwise
4. **Bird extraction**: lines starting with `> ` (space required) are code; strip 2-char prefix
5. **LaTeX extraction**: content inside `\begin{code}...\end{code}` is code; rest is prose
6. **Prose chunking**: paragraph-based (blank-line splitting), max 4,000 chars; `doc_kind = "lhs_prose"`
7. **Line numbers**: original file line numbers preserved in all stored chunks
8. **Symbol extraction**: tree-sitter-haskell runs on stripped code; line numbers mapped back via `LineMap`
9. **Additional file types in scope**: `.hsc`, `.chs` (trivial), `.hs-boot` (filename special-case), Changelogs (docs.rs special-case)

## Phase 1: Design — Complete

See [data-model.md](data-model.md) and [contracts/mcp-search-tools.md](contracts/mcp-search-tools.md).

### Implementation Sequence

**Step 1 — `src/ingest/lhs.rs` (new module)**

```
pub enum LhsStyle { Bird, LaTeX }

pub struct LhsBlock {
    pub kind:       BlockKind,   // Code | Prose
    pub content:    String,      // stripped
    pub start_line: usize,       // original file line, 1-indexed
    pub end_line:   usize,
}

pub struct ParsedLhs {
    pub style:  LhsStyle,
    pub blocks: Vec<LhsBlock>,
}

pub fn parse_lhs(source: &str) -> ParsedLhs { ... }

pub struct LineMap { ... }   // code_line → file_line mapping for symbol offset recovery
impl LineMap {
    pub fn from_blocks(code_blocks: &[&LhsBlock]) -> Self { ... }
    pub fn file_line(&self, code_line: usize) -> usize { ... }
}
```

Unit tests: empty file, Bird-only file, LaTeX-only file, file with no code blocks, mixed (LaTeX wins), unclosed `\begin{code}`, bird line with no space.

**Step 2 — `src/ingest/code.rs` modifications**

- Add `"lhs"`, `"hsc"`, `"chs"` to `CODE_EXTENSIONS`
- Add filename special-case for `.hs-boot` and `.lhs-boot` in `collect_files()`
- Add `"lhs" | "hsc" | "chs" => "haskell"` arms to `detect_language()`
- In `make_chunks()`: detect `language == "haskell"` + file path has `.lhs` extension → call `parse_lhs()` first, run `extract_symbols()` on concatenated code content, apply `LineMap` to recover original line numbers

**Step 3 — `src/ingest/docs.rs` modifications**

- In `collect_docs()`: after the `.md` extension check, add a second pass for `.lhs` files and changelog filenames (`CHANGELOG`, `ChangeLog`, `CHANGES`, `NEWS`, `HISTORY`, `*.txt`, `*.rst` variants)
- For `.lhs` files: call `parse_lhs()`, emit prose blocks as `DocRecord` with `doc_kind = "lhs_prose"`, chunked by paragraph
- For changelog files: read as plain text, chunk by paragraph, `doc_kind = "changelog"`

**Step 4 — Fixture files and tests**

- `tests/fixtures/bird_style.lhs` — minimal Bird-style `.lhs` with 1-2 functions
- `tests/fixtures/latex_style.lhs` — minimal LaTeX-style `.lhs` with prose and code
- `tests/fixtures/ffi_binding.hsc` — minimal `.hsc` with one `#type` and one Haskell function
- Integration test: index each fixture, query `code_chunks` and `documents`, assert expected rows

## Complexity Tracking

*(No violations — table omitted.)*
