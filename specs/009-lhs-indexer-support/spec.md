# Feature Specification: Literate Haskell (.lhs) Indexer Support

**Feature Branch**: `009-lhs-indexer-support`
**Created**: 2026-08-13
**Status**: Draft
**Input**: User description: ".lhs (Literate Haskell Source) indexer support for agentic-nix"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Search .lhs Source Files by Code Symbol (Priority: P1)

A developer searches for a Haskell function or type defined in a `.lhs` file (e.g., `ObserveOutcome.lhs` or `WithThreadAndTime.lhs` from iohk-monitoring). Today the search returns no results because the indexer only discovers `.hs` files. After this feature, searching for symbols defined in `.lhs` files returns accurate matches with correct file paths and extracted code snippets.

**Why this priority**: The original bug report. Developers cannot find code that exists in the repository. Every empty search result for a `.lhs`-defined symbol is a complete miss with no fallback.

**Independent Test**: Can be fully tested by running `search_code` for a known symbol defined in a `.lhs` file (e.g., `ObserveOutcome`) and verifying at least one result points to the correct `.lhs` file with a valid code snippet (Literate prefix stripped).

**Acceptance Scenarios**:

1. **Given** a `.lhs` file containing Bird-style code (`> myFunc = ...`), **When** a user searches for `myFunc` via `search_code`, **Then** a result is returned pointing to that `.lhs` file with the `> ` prefix stripped from the snippet
2. **Given** a `.lhs` file containing LaTeX-style code (`\begin{code}...\end{code}`), **When** a user searches for a symbol defined inside the code block, **Then** a result is returned pointing to that `.lhs` file with only the code block content in the snippet
3. **Given** the `language=haskell` filter is applied, **When** searching, **Then** results from both `.hs` and `.lhs` files are returned
4. **Given** an existing `.hs` file is also indexed, **When** searching after `.lhs` support is added, **Then** the `.hs` results are unaffected (no regressions)

---

### User Story 2 - Search Prose Documentation in .lhs Files (Priority: P2)

A developer searches for explanatory prose written in a `.lhs` file — for example, the rationale or design notes written around the code. Because `.lhs` is intentionally a literate format, the non-code portions are documentation, not noise. After this feature, the prose sections are indexed as documentation and surfaced via `search_docs`.

**Why this priority**: Literate Haskell's primary value proposition is colocated prose and code. Indexing only the code half discards the documentation half, which is often the most informative part for understanding intent.

**Independent Test**: Can be fully tested by running `search_docs` for a phrase that appears only in the prose section of a `.lhs` file and verifying it is returned, while `search_code` for the same prose phrase returns no match.

**Acceptance Scenarios**:

1. **Given** a Bird-style `.lhs` file where lines without `> ` are prose, **When** `search_docs` is called with a phrase from a prose section, **Then** a result is returned with the prose content and `.lhs` file path
2. **Given** a LaTeX-style `.lhs` file where text outside `\begin{code}...\end{code}` is prose, **When** `search_docs` is called with a phrase from that prose, **Then** a result is returned from the `.lhs` file
3. **Given** a prose section contains LaTeX markup (e.g., `\section{}`, `\emph{}`), **When** the prose is indexed, **Then** LaTeX commands are included as-is (no LaTeX rendering required — plain text extraction from the `.lhs` source is sufficient)
4. **Given** `search_code` is called with prose-only text from a `.lhs` file, **Then** no code result is returned for that prose (prose is not misclassified as code)

---

### User Story 3 - Prose-to-Code Navigation in LaTeX-Style .lhs Files (Priority: P3)

A developer working with LaTeX-style `.lhs` files (common in academic Haskell and IOHk codebases) retrieves a prose chunk via `search_docs` and wants to navigate to the adjacent code block that the prose describes. The documentation narrative — LaTeX prose → code block → more prose — is load-bearing for understanding the module. Each prose section should be discoverable as a doc chunk associated with its surrounding code context.

**Why this priority**: LaTeX-style `.lhs` typically contains richer, longer prose sections than Bird-style. The structure (prose → code → prose) encodes design rationale. Losing the association between a prose block and its adjacent code means losing the "why" for that code.

**Independent Test**: Can be fully tested by searching for a concept described in the prose of a LaTeX-style `.lhs` file and verifying that the result includes enough context to identify the adjacent code block (file path + approximate line range), enabling direct navigation to the corresponding code.

**Acceptance Scenarios**:

1. **Given** a LaTeX-style `.lhs` file, **When** the indexer processes it, **Then** each prose block is stored as a doc chunk that references the `.lhs` file and an approximate location (line range or surrounding code symbol)
2. **Given** a search via `search_docs` returns a prose chunk from a LaTeX-style `.lhs` file, **Then** the result includes enough context to identify the adjacent code block
3. **Given** a LaTeX-style `.lhs` file with multiple alternating prose/code sections, **When** indexed, **Then** all prose sections are indexed — not just the first or last

---

### Edge Cases

- What happens when a `.lhs` file uses both Bird-style and LaTeX-style markup in the same file? (Assumption: treat the entire file as LaTeX-style if any `\begin{code}` is present; otherwise Bird-style)
- What happens when a Bird-style line starts with `>` but no space (e.g., `>foo = 1`)? GHC requires `> ` with a space; lines starting with `>` without a space are treated as prose — follow GHC semantics
- What happens when a `\begin{code}` block is never closed? Treat the rest of the file as code; log a warning
- What happens when a `.lhs` file contains no code blocks at all? Index it as a pure documentation file via `search_docs`; do not emit a code entry or error
- What happens if extracted Haskell from a `.lhs` file is not syntactically valid? Index what is extracted; do not fail the entire indexing run for one file

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The indexer MUST discover `.lhs` files using the same directory traversal and exclusion rules applied to `.hs` files
- **FR-002**: For Bird-style `.lhs` files, the indexer MUST extract code from lines beginning with `> ` (bird tick followed by a space), stripping that two-character prefix to produce valid Haskell source for code indexing
- **FR-003**: For LaTeX-style `.lhs` files, the indexer MUST extract code from within `\begin{code}...\end{code}` blocks, discarding the delimiter lines themselves
- **FR-004**: The indexer MUST detect which `.lhs` style is in use per file: LaTeX-style if the file contains any `\begin{code}` marker, Bird-style otherwise
- **FR-005**: Non-code content in `.lhs` files (prose, comments, LaTeX markup) MUST be indexed as documentation, queryable via `search_docs`
- **FR-006**: The `language=haskell` filter on `search_code` MUST match results from `.lhs` files alongside `.hs` files without requiring any additional filter
- **FR-009**: Extracted code chunks from `.lhs` files MUST be stored with accurate source location metadata (file path and original line numbers, accounting for the literal prefix)
- **FR-008**: Existing `.hs` file indexing behavior MUST be unchanged by this feature
- **FR-009**: A `.lhs` file containing no code blocks MUST still be indexed as a documentation file and MUST NOT cause an indexing error
- **FR-010**: For LaTeX-style `.lhs` files, prose chunks SHOULD be associated with their adjacent code block to enable navigation from documentation to code (line range or symbol reference)

### Key Entities

- **LhsFile**: A source file with a `.lhs` extension. Has a detected style (Bird or LaTeX), zero or more code blocks, and zero or more prose blocks.
- **CodeChunk**: An extracted block of valid Haskell source (Bird prefix stripped, or LaTeX delimiters removed). Carries file path and original start/end line numbers.
- **ProseChunk**: An extracted block of documentation text from a `.lhs` file. Carries file path, line range, and an optional reference to the adjacent code symbol or line range.
- **LhsStyle**: Discriminant — `Bird` (default) or `LaTeX` (selected when `\begin{code}` is detected in the file).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Searching for any symbol defined in a known `.lhs` file (e.g., `ObserveOutcome`, `WithThreadAndTime`) returns at least one correct result pointing to that `.lhs` file
- **SC-002**: Zero existing `.hs` search results are broken or altered after `.lhs` support is added (verified by running the existing search test suite against a mixed `.hs`/`.lhs` repository)
- **SC-003**: Prose text from `.lhs` files is retrievable via `search_docs` — at minimum 90% of distinct prose blocks in indexed `.lhs` files are discoverable by a representative keyword from that block
- **SC-004**: The `language=haskell` filter returns results from `.lhs` files without requiring a separate filter or extension parameter
- **SC-005**: Indexing a repository containing both `.hs` and `.lhs` files completes without errors attributable to `.lhs` parsing

## Assumptions

- The agentic-nix indexer is the system being changed and is under this project's control
- The indexer currently uses a glob or file-extension filter to discover Haskell source files; adding `.lhs` to that filter is the primary entry point for the change
- GHC's strict Bird-style rule (`> ` with exactly one space after `>`) is the authoritative definition; lines starting with `>` without a following space are treated as prose
- LaTeX rendering is out of scope — prose is indexed as raw `.lhs` source text, not rendered HTML or PDF
- The indexer's chunking and embedding pipeline can accept the stripped Haskell source without modification; only the pre-processing (unwrapping) step changes
- iohk-monitoring `.lhs` files (ObserveOutcome.lhs, WithThreadAndTime.lhs) are the representative test cases driving this feature
- Mixed-style files (both Bird and LaTeX markers in the same file) are rare but handled by preferring LaTeX-style when `\begin{code}` is present
