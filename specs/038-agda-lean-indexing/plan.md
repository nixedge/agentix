# Implementation Plan: Agda, Lean, Go and Elm indexing

Branch: `038-agda-lean-indexing` | Date: 2026-11-18 | Spec: [spec.md](spec.md)

## Technical context and research

Rust 2021, existing agentix-indexer CLI/library, ignore walker and shared LanceDB storage. CODE_EXTENSIONS controls discovery; detect_language controls persisted labels. SYMBOL_LANGUAGES selects tree-sitter extraction; make_chunks converts symbols to bounded chunks and falls back to overlapping line windows. Search filters accept arbitrary language/kind strings.

Pin tree-sitter-agda 1.3.3, tree-sitter-lean4 0.3.0, tree-sitter-go 0.25.0 and tree-sitter-elm 5.9.4, all compatible with tree-sitter 0.25. Published crates contain generated parsers and any required scanners. Agda/Go/Elm expose LANGUAGE; Lean exposes language(). Lean 0.3.0 includes layout-scanner compilation. See research.md for source details.

## Design and structure

1. In agentix-indexer/src/ingest/code.rs, add agda/lagda/lean/elm discovery and labels; retain existing Go discovery/label. Exclude .lake and elm-stuff directories. Enable symbol chunking for all four languages.
2. In symbols.rs, walk Agda module/privacy/mutual/recovery containers, extracting functions/types/records and grouping consecutive same-name signatures/equations, including mixfix names. Preserve indented constructor/field source recovered as siblings inside its type/record chunk. Walk Lean declarations and attributed wrappers for function/theorem/type/struct/class/instance/axiom/const kinds. Avoid extracting comments or proof expressions as declarations.
3. In symbols.rs, extract Go functions, receiver-qualified methods, type specs/aliases, constants and variables. Extract Elm values, type annotations, custom types, aliases and ports; group a function annotation with its definition without descending into function-local declarations.
4. For plain .lagda, mask prose and LaTeX delimiters before Agda parsing while retaining line positions; restore chunk content from original lines. Preserve existing bounded fallback where no declarations are recognized.
5. Add discovery/chunk unit coverage and grammar fixtures. Extend tests/local_ingestion.rs using actual LanceDB and deterministic embeddings for local and HTTP-backed ingestion; verify language/kind/project filters, original content/coordinates, ignored caches, unchanged re-ingestion, replacement and deletion for every language.
6. Update README.md and quickstart.md; record validation in verification.md.

## Data model and contracts

Existing code_chunks schema and MCP contracts remain unchanged. New language values: agda, lean, elm; Go retains go. Existing symbol_kind carries extracted declaration kinds. Names appear in source chunks as with other supported languages. No migration is needed. Normal repository ingestion discovers new formats on refresh; already-indexed Go needs --force to regenerate declaration chunks. Completed immutable Git imports also require --force. Compound Agda literate formats are excluded.

## Constitution check

Before and after design: existing library/CLI boundaries retained; no new workspace crates, listeners, credentials, cloud inference or sandbox changes. Grammar C/scanner dependencies remain isolated in the indexer. Nix remains the toolchain/build source. Dependencies are pinned in Cargo.lock, following existing per-crate grammar dependency placement. Tests use actual storage and deterministic fixture embeddings. Run formatting, indexer tests, Clippy and canonical nix build .#agentix-indexer. Workspace tests and CI remain merge gates. No exceptions.
