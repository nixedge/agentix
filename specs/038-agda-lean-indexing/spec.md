# Feature Specification: Agda, Lean, Go and Elm indexing

**Feature Branch**: `038-agda-lean-indexing`
**Created**: 2026-11-18
**Status**: Implemented and validated
**Input**: add agda and lean languages to agentic-nix indexing

## User Scenarios & Testing

### User Story 1 — Search formal source (P1)

As a code-search user, I can ingest Agda and Lean repositories and find their source with language filters.

**Independent test**: Ingest Unicode fixtures into actual local LanceDB and retrieve markers with language and project filters.

1. Given `.agda` or `.lagda` source, when code ingestion runs, then searchable chunks have language `agda`, original content, paths and valid original line ranges.
2. Given `.lean` source, when code ingestion runs, then searchable chunks have language `lean` and retain Unicode proof text.
3. Given an unchanged repository, when ingestion runs again, then no source is embedded again; changed and deleted files update existing search results.

### User Story 2 — Keep generated dependencies out (P2)

As a repository owner, I can index formal source without indexing Lean dependency and build caches.

**Independent test**: Discover a fixture repository containing supported source, `.lake` content and ignored source.

1. Given files under `.lake` or ignored by Git, when indexing runs, then those files are absent from search.
2. Given uppercase source extensions, when indexing runs, then language labels match lowercase extensions.

### User Story 3 — Search Go and Elm declarations (P1)

As a code-search user, I can retrieve Go and Elm declarations with useful symbol filters.

**Independent test**: Ingest Go/Elm fixtures and verify local and remote language/kind/project filtering and exact source ranges.

1. Given Go functions, receiver methods, types and bindings, when indexing runs, then declaration chunks retain the appropriate semantic kinds and receiver-qualified method names.
2. Given Elm function annotations and implementations, custom types and aliases, when indexing runs, then functions are grouped and all declaration kinds are searchable under language `elm`.
3. Given Elm generated files in `elm-stuff`, when indexing runs, then those files are excluded; unchanged, edited and deleted Go/Elm files retain existing incremental behavior.

## Requirements

- FR-001: Discover `.agda`, `.lagda` and `.lean` case-insensitively.
- FR-002: Store `agda` or `lean` labels and bounded text chunks with original line numbers and Unicode preserved.
- FR-003: Preserve existing ignore rules and exclude `.lake` directories.
- FR-004: Reuse incremental update, project tagging, deletion and local/remote storage behavior.
- FR-005: Document supported extensions, text chunking and index refresh.
- FR-006: Use tree-sitter for Agda and Lean declaration extraction, recording names and semantic kinds for ordinary definitions, theorems and data/record declarations; retain bounded fallback for source without recognized declarations.
- FR-007: Discover `.elm` case-insensitively with language `elm`, and use tree-sitter for both Elm and existing Go (`.go`, language `go`) sources. Extract Go functions, receiver methods, types and named constants/variables; extract Elm functions (group annotations with definitions), custom types and type aliases. Exclude Elm's `elm-stuff` cache.

## Edge Cases

Empty files produce no chunks. Unsupported extensions and compiled `.agdai`/`.olean` files remain undiscovered. Existing chunk limits apply. Plain `.lagda` code must be parsed with original coordinates preserved; prose may use text fallback. Compound literate formats such as `.lagda.md` are outside this feature. No theorem checking or compiler invocation is required. Comments must not create declarations, and module/namespace wrappers must not hide nested declarations.

## Success Criteria

- SC-001: All five supported extensions return source markers under the correct language filter in local and remote ingestion tests.
- SC-002: Every returned fixture chunk reproduces its original source line range exactly.
- SC-003: Unchanged ingestion makes zero additional embedding requests; deleted source leaves zero rows for its path.
- SC-004: Cache and ignored markers return zero results, and existing indexer tests remain green.
- SC-005: Fixture definitions, theorems and data/record declarations are returned as tree-sitter-derived symbols with exact original line ranges and searchable kinds.
- SC-006: Go and Elm fixtures pass declaration, metadata, filtered local/remote search, incremental and ignored-cache acceptance checks alongside Agda and Lean.

## Assumptions

Lean support targets Lean 4 `.lean` source. Language filters accept strings already, so API/schema changes are unnecessary. User clarified that tree-sitter extraction is required for both languages.
