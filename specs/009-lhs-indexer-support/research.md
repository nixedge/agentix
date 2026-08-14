# Research: Literate Haskell (.lhs) Indexer Support

**Feature**: 009-lhs-indexer-support
**Date**: 2026-08-13

---

## 1. Root Cause: Confirmed

**Decision**: The bug is confirmed and the fix is well-scoped.

**Rationale**: `CODE_EXTENSIONS` in `agentix-indexer/src/ingest/code.rs:7-10` is the single gate for file discovery. `"lhs"` is not in the list. Additionally, `detect_language()` at line 283-320 has no mapping for `.lhs`, and `SYMBOL_LANGUAGES` at line 321-329 routes `"haskell"` through tree-sitter-haskell — which cannot parse raw `.lhs` source because it contains non-Haskell markup. A pre-processing layer is required.

**Alternatives considered**: Relying on tree-sitter-haskell's error recovery to skip Bird markup. Rejected — the `> ` prefix lines are syntactically invalid Haskell; the parser emits ERROR nodes for every code line, making symbol extraction useless.

---

## 2. Pre-Processing Architecture

**Decision**: Extract a new `agentix-indexer/src/ingest/lhs.rs` module containing the `.lhs` pre-processor. The code pipeline (`code.rs`) and doc pipeline (`docs.rs`) each consume its output separately.

**Rationale**: The `.lhs` format must fan out to two different storage tables:
- Stripped Haskell code → `code_chunks` (via `ingest_code`)
- Prose text → `documents` (via `ingest_docs`)

Embedding this in either `code.rs` or `docs.rs` alone would require one to call into the other, creating circular logic. A shared `lhs.rs` module keeps the splitting logic in one place and lets each pipeline consume what it needs.

**Alternatives considered**: Handling `.lhs` entirely inside `code.rs` by storing prose as `symbol_kind = "lhs_prose"` in `code_chunks`. Rejected — prose has different chunking semantics, different search relevance tuning, and belongs in the `documents` table where `search_docs` can reach it.

---

## 3. Style Detection

**Decision**: Per-file style detection. LaTeX-style wins if any `\begin{code}` marker is present in the file; Bird-style otherwise.

**Rationale**: GHC uses the same heuristic (presence of `\begin{code}` triggers LaTeX mode for the whole file). Mixed-style files are undefined behavior in GHC and vanishingly rare in practice.

**Implementation**: Single linear scan of the file checking for `\begin{code}` before splitting into blocks.

---

## 4. Bird-Style Extraction

**Decision**: A line beginning with `> ` (greater-than followed by exactly one space) is code; all other lines are prose. Strip the two-character prefix from code lines.

**Rationale**: This is GHC's exact rule. Lines starting with `>` without a following space are prose in GHC — do not treat them as code.

**Edge case**: A line containing only `>` (no space) → prose.

---

## 5. LaTeX-Style Extraction

**Decision**: Content inside `\begin{code}...\end{code}` pairs is code. Everything else (including the delimiter lines themselves) is prose. Unclosed `\begin{code}` → treat rest of file as code; log a warning.

**Rationale**: GHC only supports `\begin{code}` / `\end{code}` as delimiters (not `\begin{haskell}` or other variants). Some packages also use `\begin{spec}...\end{spec}` for non-compiled specification prose that looks like Haskell — this is treated as prose since GHC ignores it.

---

## 6. Prose Chunking (not Markdown)

**Decision**: Prose from `.lhs` files is chunked by paragraph (runs of non-blank lines separated by one or more blank lines), with a maximum chunk size of 4,000 characters matching `MAX_CHUNK_CHARS` in `docs.rs`.

**Rationale**: `.lhs` prose is not Markdown, so `chunk_markdown()` (which splits on `##` headings) is not appropriate. LaTeX section commands (`\section{}`) could be used as split points for LaTeX-style files, but many `.lhs` files don't use explicit section commands. Paragraph splitting works for both styles and produces chunks of appropriate semantic granularity.

**doc_kind**: `"lhs_prose"` — new value inserted into `classify_doc()`-equivalent logic in the `.lhs` prose writer.

---

## 7. Line Number Tracking

**Decision**: Store original (pre-strip) line numbers in `code_chunks`. When a Bird-style code block is extracted, its `start_line` and `end_line` refer to the original `.lhs` file, not a re-numbered stripped version.

**Rationale**: Line numbers are shown to developers navigating results. If they open the `.lhs` file and jump to line 47, they need to land on the correct line in the original file. Renumbering would make navigation incorrect.

**Implementation**: Track a `file_line_offset: usize` as lines are emitted, increment for every source line, record the offset at the start of each code block.

---

## 8. Symbol Extraction for .lhs

**Decision**: After stripping, feed the concatenated code content to `extract_haskell_symbols()` (tree-sitter-haskell). The extracted symbols' `start_line` / `end_line` values from tree-sitter will be relative to the stripped code; map them back to original file lines using the offset table.

**Rationale**: Symbol extraction is what makes `search_code` return meaningful `symbol_kind` values (function, type, impl). Without this, `.lhs` code would fall back to `chunk_lines()`, producing 120-line window chunks with no symbol metadata.

**Alternative considered**: Skip symbol extraction for `.lhs`, always use `chunk_lines()`. Acceptable as a v1 simplification if offset mapping proves complex; document as a known limitation.

---

## 9. Additional Missing File Types (scope of this feature)

Research into what other Haskell-ecosystem file types are missing from `CODE_EXTENSIONS` and would be valuable when fetching packages from Hackage.

### 9a. `.hsc` — hsc2hs FFI Interface Files

**Prevalence**: High. Any Haskell package wrapping a C library uses `.hsc` (unix, network, openssl, zlib, posix, sqlite bindings, etc.). The crypton package (351 files, 3010 chunks indexed) very likely has `.hsc` files not currently indexed.

**Content**: Haskell source with interspersed hsc2hs directives:
- `#include "header.h"` — C header inclusion
- `#type CType` — generate `type CType = ...` from C typedef
- `#field struct_name, field_name` — generate field accessor
- `#enum EnumName, mkEnumName, VALUE1, VALUE2` — generate enum
- `#peek struct_name, field_name` / `#poke struct_name, field_name` — raw memory accessors

**Value for library understanding**: Essential for understanding how a Haskell library wraps C types. Without `.hsc` indexing, the actual C-to-Haskell type mappings are invisible.

**Implementation cost**: Minimal. Add `"hsc"` to `CODE_EXTENSIONS` and `"hsc" => "haskell"` to `detect_language()`. tree-sitter-haskell will parse the Haskell parts successfully and produce ERROR nodes for the directive lines. Because `SYMBOL_LANGUAGES` includes `"haskell"`, symbol extraction will run; the fallback to `chunk_lines()` triggers automatically if symbols are sparse (which they will be in `.hsc` files heavy on directives). No pre-processing required.

**Decision**: Add `.hsc` support as part of this feature. One-line additions to `CODE_EXTENSIONS` and `detect_language()`.

### 9b. `.chs` — c2hs Interface Files

**Prevalence**: Medium. Less common than hsc2hs. Used by gtk2hs, cairo, glib, and some older C binding packages.

**Content**: Haskell source with `{# ... #}` hook directives (import, fun, call, get, set, sizeof, alignof, enum, pointer, class).

**Value**: Similar to `.hsc` — reveals C-to-Haskell type mappings and function wrappers. Less critical than `.hsc` because c2hs is less prevalent in current Cardano/IOHk tooling.

**Decision**: Add `.chs` support as part of this feature. Same one-line additions. tree-sitter-haskell will parse the Haskell portions; `{# ... #}` directives will be ERROR nodes, triggering the line-based fallback for heavily directive files.

### 9c. `.hs-boot` / `.lhs-boot` — Mutual Recursion Boot Files

**Prevalence**: Low. Only needed for mutually recursive module pairs. Present in some large packages (GHC itself, some compiler infrastructure).

**Content**: Haskell type signatures and declarations without implementations. Useful for understanding the interface boundary in circular-dependency modules.

**Decision**: Add `"hs-boot"` and `"lhs-boot"` to `CODE_EXTENSIONS`. `detect_language()` maps them to `"haskell"`. For `.lhs-boot`, apply the same Bird/LaTeX stripping as `.lhs`. Note: `Path::extension()` returns `"boot"` for `Foo.hs-boot` — a special-case filename check is needed (similar to `cabal.project`).

### 9d. Changelogs Without `.md` Extension

**Prevalence**: High in older Hackage packages. Many use `CHANGELOG`, `ChangeLog`, `CHANGES`, `NEWS` without any extension, or with `.rst` extension.

**Content**: API migration notes, breaking change history, version timeline. Extremely useful when trying to understand how a library's API evolved.

**Current gap**: `collect_docs()` in `docs.rs` only matches `.md` extension. A `CHANGELOG` file (no extension) is silently skipped.

**Decision**: Add filename-based special-casing in `collect_docs()` for `CHANGELOG`, `ChangeLog`, `CHANGES`, `NEWS`, `HISTORY` (with or without `.txt`/`.rst` extension). These are plain text or reStructuredText — chunk by paragraph, `doc_kind = "changelog"`. This is independent of `.lhs` but is a natural companion fix given the same Hackage fetch use case.

### 9e. `.hsig` / `.lhsig` — Backpack Signature Files

**Prevalence**: Low. Only in Backpack-based packages (text, bytestring compatibility shims, some IOHk signature packages).

**Content**: Abstract module signatures declaring type and function interfaces without implementations.

**Decision**: Defer to a separate issue. These are rare enough that the cost of researching correct Backpack semantics outweighs the benefit for current use cases.

### 9f. `package.yaml` (hpack)

**Status**: Already covered. `.yaml` is in `CODE_EXTENSIONS`, mapped to `language=yaml`. `package.yaml` is discovered and indexed. The content is useful for understanding build structure; no further action needed.

### 9g. `cabal.project` / `cabal.project.freeze` / `cabal.project.local`

**Status**: Already covered via special-case filename matching in `collect_files()` at `code.rs:265-267`. No action needed.

### 9h. Template Haskell

**Status**: Not a separate file type. Template Haskell splices (`$()`, `[| |]`, `[d| |]`) are embedded in regular `.hs` and `.lhs` files. They are already indexed (or will be with `.lhs` support) as part of the containing file. No separate handling needed.

---

## Summary: Recommended Additions

| File Type | Action | Cost | Priority |
|-----------|--------|------|----------|
| `.lhs` | Pre-process + dual indexing | High | P0 (this feature) |
| `.hsc` | Add to extensions + language map | Trivial | P1 (same PR) |
| `.chs` | Add to extensions + language map | Trivial | P1 (same PR) |
| `.hs-boot` | Special-case filename + extension | Low | P2 (same PR) |
| Changelogs | Filename special-case in docs.rs | Low | P2 (same PR) |
| `.hsig` / `.lhsig` | Research separately | — | Defer |
| Template Haskell | No action needed | — | N/A |
