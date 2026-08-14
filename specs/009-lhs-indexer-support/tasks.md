# Tasks: Literate Haskell (.lhs) Indexer Support

**Input**: Design documents from `/specs/009-lhs-indexer-support/`
**Prerequisites**: plan.md ✓, spec.md ✓, research.md ✓, data-model.md ✓, contracts/ ✓

**Codebase**: `agentix-indexer/` crate (binary: `ingest`)
**Key files**: `agentix-indexer/src/ingest/code.rs`, `agentix-indexer/src/ingest/docs.rs`, `agentix-indexer/src/ingest/lhs.rs` (new), `agentix-indexer/src/ingest/symbols.rs` (unchanged)

## Format: `[ID] [P?] [Story?] Description`

- **[P]**: Can run in parallel (different functions / no dependency on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1 = code search, US2 = prose search, US3 = prose-to-code navigation)

---

## Phase 1: Setup (Fixture Files)

**Purpose**: Create small, self-contained test fixtures. These are needed by all later integration tests and must exist before any test can run.

- [X] T001 Create `agentix-indexer/agentix-indexer/tests/fixtures/bird_style.lhs` — a minimal Bird-style Literate Haskell file (< 50 lines) with at least two functions using `> ` prefix, two lines of prose between them, and one prose-only section at the end
- [X] T002 [P] Create `agentix-indexer/agentix-indexer/tests/fixtures/latex_style.lhs` — a minimal LaTeX-style Literate Haskell file (< 50 lines) with one `\begin{code}...\end{code}` block containing a function, prose before and after the code block
- [X] T003 [P] Create `agentix-indexer/agentix-indexer/tests/fixtures/ffi_binding.hsc` — a minimal `.hsc` file (< 30 lines) with one `#include`, one `#type` directive, and one plain Haskell function

---

## Phase 2: Foundational — `agentix-indexer/src/ingest/lhs.rs` Parser

**Purpose**: The pre-processor shared by both the code pipeline (US1) and the docs pipeline (US2/US3). No user story work can begin until this module compiles and passes unit tests.

**⚠️ CRITICAL**: Phases 3, 4, and 5 all depend on this phase being complete.

- [X] T004 Create `agentix-indexer/src/ingest/lhs.rs` with type definitions: `pub enum LhsStyle { Bird, LaTeX }`, `pub struct LhsBlock { pub kind: BlockKind, pub content: String, pub start_line: usize, pub end_line: usize }`, `pub enum BlockKind { Code, Prose }`, and `pub struct ParsedLhs { pub style: LhsStyle, pub blocks: Vec<LhsBlock> }`. Add inherent methods: `ParsedLhs::code_blocks()`, `ParsedLhs::prose_blocks()`, `ParsedLhs::has_code()`. Add `pub fn detect_style(source: &str) -> LhsStyle` (returns `LaTeX` if any line is exactly `\begin{code}`; `Bird` otherwise).
- [X] T005 [P] Implement `fn parse_bird_style(source: &str) -> ParsedLhs` in `agentix-indexer/src/ingest/lhs.rs`. Iterate lines; lines starting with `> ` (greater-than + space) are code (strip 2-char prefix); all other lines (including bare `>`) are prose. Group consecutive same-kind lines into `LhsBlock` values. Track original 1-indexed line numbers for each block's `start_line`/`end_line`.
- [X] T006 [P] Implement `fn parse_latex_style(source: &str) -> ParsedLhs` in `agentix-indexer/src/ingest/lhs.rs`. Scan for `\begin{code}` and `\end{code}` delimiter lines. Content between delimiters (exclusive) is a `Code` block; content outside (including delimiter lines themselves) is `Prose`. Handle unclosed `\begin{code}` by treating the rest of the file as code (emit an `eprintln!` warning). Track original 1-indexed line numbers.
- [X] T007 Implement `pub fn parse_lhs(source: &str) -> ParsedLhs` in `agentix-indexer/src/ingest/lhs.rs` (top-level entry point): call `detect_style(source)`, dispatch to `parse_bird_style()` or `parse_latex_style()`. Depends on T005 and T006.
- [X] T008 Implement `pub struct LineMap` and methods in `agentix-indexer/src/ingest/lhs.rs`: `LineMap::from_code_blocks(blocks: &[&LhsBlock]) -> LineMap` builds a sorted Vec mapping each code-buffer line (1-indexed) to its original file line; `LineMap::file_line(code_line: usize) -> usize` binary-searches and returns the original file line. Used to translate tree-sitter symbol positions back to `.lhs` file coordinates.
- [X] T009 Add `pub mod lhs;` to the `mod ingest { ... }` block in `agentix-indexer/src/main.rs` AND to the `pub mod ingest { ... }` block in `agentix-indexer/src/lib.rs` (both files declare the ingest module independently).
- [X] T010 [P] Unit tests in `agentix-indexer/src/ingest/lhs.rs` for Bird-style: empty file → `ParsedLhs { blocks: [] }`; file with only prose (no `> `) → one `Prose` block; file with only code lines → one `Code` block; mixed file → alternating `Code`/`Prose` blocks; bare `>` (no space) → treated as `Prose`; verify `start_line`/`end_line` values match original file.
- [X] T011 [P] Unit tests in `agentix-indexer/src/ingest/lhs.rs` for LaTeX-style: file with `\begin{code}...\end{code}` → one `Code` block + surrounding `Prose` blocks; multiple code blocks → correct alternation; unclosed `\begin{code}` → rest of file is `Code`; empty code block → zero-length `Code` block (or empty blocks filtered); verify line numbers.

**Checkpoint**: `cargo test -p agentix-indexer` passes for all `lhs.rs` unit tests. US1/US2/US3 implementation can now begin.

---

## Phase 3: User Story 1 — Code Symbol Search for .lhs / .hsc / .chs / .hs-boot (Priority: P1) 🎯 MVP

**Goal**: `search_code` with `language=haskell` returns results from `.lhs`, `.hsc`, `.chs`, and `.hs-boot` files alongside `.hs` files.

**Independent Test**: Run `search_code` with a symbol name defined in `agentix-indexer/agentix-indexer/tests/fixtures/bird_style.lhs`; verify at least one result with `file_path` ending in `.lhs`, `language = "haskell"`, and content that does NOT contain `> ` prefix.

### Implementation for User Story 1

- [X] T012 [US1] In `agentix-indexer/src/ingest/code.rs`, add `"lhs"`, `"hsc"`, `"chs"` to the `CODE_EXTENSIONS` constant (line ~7). Also add `.hs-boot` and `.lhs-boot` filename detection in `collect_files()` alongside the existing `is_cabal_project` pattern: `let is_boot = filename.ends_with(".hs-boot") || filename.ends_with(".lhs-boot");` and include it in the `matches` condition.
- [X] T013 [US1] In `detect_language()` in `agentix-indexer/src/ingest/code.rs`, add match arms: `"lhs" | "hsc" | "chs" => "haskell"`. For `.hs-boot` and `.lhs-boot`, the filename check in `collect_files()` is sufficient; `detect_language()` sees extension `"boot"` and returns `"text"` — add `"hs-boot"` handling via a filename check at the top of `detect_language()` mirroring the `cabal.project` pattern: `if filename.ends_with(".hs-boot") || filename.ends_with(".lhs-boot") { return "haskell".to_string(); }`.
- [X] T014 [US1] In `agentix-indexer/src/ingest/code.rs`, add `fn make_lhs_chunks(source: &str, file_path: &Path) -> Vec<Chunk>`: (1) call `lhs::parse_lhs(source)`; (2) concatenate all `Code` blocks' content (joined by `"\n"`) into a stripped source string; (3) build a `lhs::LineMap` from the code blocks; (4) if no code blocks, return an empty `Vec`; (5) call `extract_symbols(stripped_source.as_bytes(), ...)` directly, or call `make_chunks(&stripped_source, "haskell")`; (6) for each returned `Chunk`, map `start_line`/`end_line` through `LineMap::file_line()` to recover original `.lhs` file coordinates; (7) return the adjusted chunks.
- [X] T015 [US1] In `ingest_code()` in `agentix-indexer/src/ingest/code.rs`, after `let lang = detect_language(file_path);`, add a branch: if the file extension is `"lhs"` or the filename ends with `".lhs-boot"`, call `make_lhs_chunks(&source, file_path)` instead of `make_chunks(&source, &lang)`. All other extensions (including `.hsc`, `.chs`, `.hs-boot`) go through the existing `make_chunks()` path unchanged — tree-sitter falls back to `chunk_lines()` when the source has too many parse errors.
- [X] T016 [P] [US1] Unit test in `agentix-indexer/src/ingest/code.rs` test block: given `"> myFunc = 42\n\nsome prose\n"`, call `make_lhs_chunks()`, assert returned chunks have content not containing `"> "` and `language` resolves to `"haskell"`.
- [X] T017 [P] [US1] Unit test in `agentix-indexer/src/ingest/code.rs` test block: given LaTeX-style source with `\begin{code}\nmyFunc = 42\n\end{code}\nsome prose`, call `make_lhs_chunks()`, assert chunk content is `"myFunc = 42"` only (no delimiter lines, no prose).
- [X] T018 [P] [US1] Integration test (requires live PostgreSQL): index `agentix-indexer/agentix-indexer/tests/fixtures/bird_style.lhs` via `ingest_code()`, query `code_chunks WHERE file_path LIKE '%.lhs'`, assert at least one row with `language = 'haskell'` and `content` not containing `'> '`.
- [X] T019 [P] [US1] Integration test: index `agentix-indexer/agentix-indexer/tests/fixtures/ffi_binding.hsc` via `ingest_code()`, query `code_chunks WHERE file_path LIKE '%.hsc'`, assert at least one row with `language = 'haskell'`.

**Checkpoint**: US1 is fully functional. `search_code language=haskell` returns results from `.lhs`, `.hsc`, `.chs`, and `.hs-boot` files.

---

## Phase 4: User Story 2 — Prose Documentation Search (Priority: P2)

**Goal**: `search_docs` returns prose sections from `.lhs` files as documentation chunks with `doc_kind = "lhs_prose"`. Changelog files without `.md` extension are also discoverable.

**Independent Test**: Run `search_docs` with a phrase that appears only in the prose section of `agentix-indexer/agentix-indexer/tests/fixtures/bird_style.lhs`; verify at least one result with `source_path` ending in `.lhs` and `doc_kind = "lhs_prose"`. Confirm `search_code` for the same prose phrase returns no results.

### Implementation for User Story 2

- [X] T020 [US2] In `collect_docs()` in `agentix-indexer/src/ingest/docs.rs`, extend the file walker to also match `.lhs` files: after the existing `if ext.to_lowercase() != "md" { continue; }` guard, add a second arm that accepts `ext == "lhs"` and assigns `kind = "lhs_prose"` (so the walker collects `.lhs` files into `candidates` alongside `.md` files).
- [X] T021 [US2] In `ingest_docs()` in `agentix-indexer/src/ingest/docs.rs`, add a branch for `.lhs` files (detected by `doc_kind == "lhs_prose"`): (1) call `lhs::parse_lhs(&source)`; (2) for each `Prose` block from `ParsedLhs::prose_blocks()`, split the block content into paragraphs (split on runs of blank lines); (3) emit one `DocRecord` per paragraph with `doc_kind = "lhs_prose"`, `title = file_stem`, `content = paragraph_text`, skipping paragraphs shorter than `MIN_CHUNK_CHARS`. Paragraphs exceeding `MAX_CHUNK_CHARS` are truncated at a char boundary (reuse `truncate_to_char_boundary()`).
- [X] T022 [P] [US2] In `collect_docs()` in `agentix-indexer/src/ingest/docs.rs`, add filename special-casing for changelogs (mirroring the `cabal.project` pattern in `collect_files()`): recognize `CHANGELOG`, `ChangeLog`, `CHANGES`, `NEWS`, `HISTORY`, `CHANGELOG.txt`, `CHANGES.txt`, `NEWS.txt`, `CHANGELOG.rst`, `CHANGES.rst` as valid doc files with `doc_kind = "changelog"`. In `ingest_docs()`, handle `doc_kind == "changelog"` via paragraph chunking (same as prose blocks — no Markdown heading splitting).
- [X] T023 [P] [US2] Unit test in `agentix-indexer/src/ingest/docs.rs` test block: given Bird-style `.lhs` source with a prose section containing multiple paragraphs, assert that the produced `DocRecord` list has one entry per paragraph with `doc_kind = "lhs_prose"` and `content` not containing `"> "`.
- [X] T024 [US2] Integration test (requires live PostgreSQL): index `agentix-indexer/agentix-indexer/tests/fixtures/bird_style.lhs` through `ingest_docs()`, query `documents WHERE doc_kind = 'lhs_prose'`, assert at least one row. Also assert no rows exist in `code_chunks` with `content` matching the prose text (no cross-contamination).

**Checkpoint**: US1 and US2 are both functional. `search_code language=haskell` returns code from `.lhs` files; `search_docs` returns prose from the same files.

---

## Phase 5: User Story 3 — Prose-to-Code Navigation in LaTeX-Style .lhs Files (Priority: P3)

**Goal**: Each prose chunk from a LaTeX-style `.lhs` file carries a reference to the line range of the adjacent code block, enabling navigation from a doc search result to the corresponding code.

**Independent Test**: Index `agentix-indexer/agentix-indexer/tests/fixtures/latex_style.lhs`; query `documents` for the prose chunk; verify the `content` field begins with a `[Adjacent code: lines N-M]` header where N and M correspond to the actual code block line numbers in the fixture file.

### Implementation for User Story 3

- [X] T025 [US3] In the `.lhs` prose extraction loop in `ingest_docs()` in `agentix-indexer/src/ingest/docs.rs`, find the next `Code` block following each `Prose` block in `ParsedLhs.blocks`. If one exists, prepend a reference line to the prose chunk's `content`: `format!("[Adjacent code: lines {}-{}]\n\n{}", code_block.start_line, code_block.end_line, paragraph_text)`. If no following code block exists (prose is at end of file), emit the content without the header.
- [X] T026 [P] [US3] Unit test in `agentix-indexer/src/ingest/docs.rs` test block: given a `ParsedLhs` with alternating `Prose` / `Code` / `Prose` blocks (LaTeX-style fixture), assert that the first prose `DocRecord` content begins with `[Adjacent code: lines N-M]` for the correct line range of the Code block.
- [X] T027 [US3] Integration test: index `agentix-indexer/agentix-indexer/tests/fixtures/latex_style.lhs` through `ingest_docs()`, query `documents WHERE doc_kind = 'lhs_prose'`, verify at least one row whose `content` starts with `[Adjacent code: lines `.

**Checkpoint**: All three user stories are fully functional and independently testable.

---

## Phase 6: Polish & Cross-Cutting Concerns

- [X] T028 [P] Run `cargo fmt --all --check`; fix all formatting issues in `agentix-indexer/src/ingest/lhs.rs`, `agentix-indexer/src/ingest/code.rs`, `agentix-indexer/src/ingest/docs.rs` (no `rustfmt` overrides).
- [X] T029 [P] Run `cargo clippy -- -D warnings` on the workspace; fix all warnings in modified files. Pay special attention to `clippy::unwrap_used` and `clippy::expect_used` — replace with `?` or explicit `match`. Add `// SAFETY:` comment if any `unsafe` block was introduced.
- [X] T030 Run `cargo test --workspace`; confirm all new unit tests (T010, T011, T016, T017, T023, T026) and integration tests (T018, T019, T024, T027) pass. Fix any failures before proceeding.
- [X] T031 [P] Verify `nix build .#agentix-indexer` succeeds; the Nix build is the canonical reproducibility check per the constitution.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately; T001, T002, T003 fully parallel
- **Foundational (Phase 2)**: Depends on Phase 1 completion — **blocks all user story phases**
  - T004 → T005 [P], T006 [P] → T007 → T008; T009 [P], T010 [P], T011 [P]
- **US1 (Phase 3)**: Depends on Phase 2 — T012 → T013 → T014 → T015; T016 [P], T017 [P], T018 [P], T019 [P]
- **US2 (Phase 4)**: Depends on Phase 2 (uses `lhs::parse_lhs()`); independent of US1
- **US3 (Phase 5)**: Depends on Phase 4 (extends prose extraction); independent of US1
- **Polish (Phase 6)**: Depends on all user story phases complete

### User Story Dependencies

- **US1 (P1)**: Depends on Foundational (Phase 2) only. No dependency on US2 or US3.
- **US2 (P2)**: Depends on Foundational (Phase 2) only. No dependency on US1 or US3.
- **US3 (P3)**: Depends on US2 (Phase 4) — extends the prose extraction logic added in US2.

### Within Each Phase

- Foundational: T004 (types) → T005, T006 can run in parallel → T007 (parse_lhs top-level, needs both) → T008 (LineMap); T009, T010, T011 (tests) run in parallel after their respective implementations
- US1: T012, T013 (constants + detect_language) can run in parallel → T014 (make_lhs_chunks) → T015 (wire into ingest_code); tests T016–T019 run in parallel after T015
- US2: T020 (collect_docs), T022 (changelogs) can run in parallel → T021 (prose extraction, depends on T020) → T023, T024 (tests)
- US3: T025 (adjacency) → T026, T027 (tests) in parallel

### Parallel Opportunities

```bash
# Phase 1 — all in parallel:
T001: Create bird_style.lhs
T002: Create latex_style.lhs
T003: Create ffi_binding.hsc

# Phase 2 — partial parallel:
T004                     # types first
T005 & T006              # Bird and LaTeX parsers in parallel
T007                     # parse_lhs top-level (needs T005, T006)
T008                     # LineMap (independent of T005-T007, depends only on T004)
T009 & T010 & T011       # unit tests for Bird, LaTeX, after their impls

# US1 & US2 phases — can start simultaneously after Phase 2:
#   Developer A: Phase 3 (code pipeline)
#   Developer B: Phase 4 (docs pipeline)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only — Phases 1–3)

1. Create fixtures (Phase 1)
2. Build `lhs.rs` parser (Phase 2) — the critical foundation
3. Wire into code pipeline (Phase 3) — `search_code` now finds `.lhs` symbols
4. **STOP and VALIDATE**: Re-index `iohk-monitoring` package; verify `ObserveOutcome` and `WithThreadAndTime` appear in `search_code language=haskell` results
5. Ship the MVP — the original bug is fixed

### Incremental Delivery

1. MVP (Phases 1–3) → `.lhs` code symbols searchable
2. Add US2 (Phase 4) → `.lhs` prose searchable via `search_docs`
3. Add US3 (Phase 5) → LaTeX-style prose chunks carry code adjacency metadata
4. Polish (Phase 6) → CI-clean, Nix build verified

### Notes

- `agentix-indexer/src/ingest/symbols.rs` requires **no changes** — `extract_haskell_symbols()` runs on already-stripped Haskell source
- The `documents` table and `code_chunks` table require **no schema migrations**
- `doc_kind = "lhs_prose"` and `doc_kind = "changelog"` are new string values in an existing `text` column — no enum or constraint to update
- The `[Adjacent code: lines N-M]` adjacency header (US3) is stored in `content`, not a separate column — no schema change, no MCP tool API change
- `.hsc` and `.chs` get no special pre-processing — tree-sitter-haskell's existing fallback to `chunk_lines()` handles directive-heavy files automatically
