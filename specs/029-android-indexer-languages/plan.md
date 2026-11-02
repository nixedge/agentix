# Implementation plan: Android and Kotlin indexing

## Technical context and research

Rust 2021, existing `agentix-indexer`, tree-sitter-kotlin-sg 0.4.0, shared LanceDB storage. `detect_language` and `extract_symbols` already handle Kotlin, but `CODE_EXTENSIONS` omits kt/kts. XML/Gradle/properties are also omitted. Search accepts arbitrary language strings, so no query API or schema changes are needed.

## Design

1. Extend discovery and language detection in `agentix-indexer/src/ingest/code.rs`; skip Android IDE/compiler/build caches with the existing directory filter.
2. Reuse Kotlin/Java symbol extraction and existing bounded line fallback for script/config files. No new dependencies or crate.
3. Add collection regression coverage for ignore rules and case-insensitive extensions. Extend `tests/local_ingestion.rs` with a realistic Android fixture using the existing embedding server and actual LanceDB; verify keyword language/kind/project filters, paths, line ranges, unchanged runs, and deleted Kotlin files.
4. Document supported Android types, index refresh, and code-search filters in README and feature quickstart.

## Constitution check

Before and after design: existing library/CLI boundaries retained; no listener, credentials, network expansion, cloud inference, schema or API changes. Tests use actual LanceDB and the existing deterministic embedding fixture. Nix storage dev shell runs formatting, indexer tests, and Clippy; `nix build .#agentix-indexer` is the canonical package gate. Workspace tests remain the repository merge gate; unrelated inference/CUDA packages may require separate resources. No exceptions.

## Contract and data model

Existing `code_chunks` fields and MCP parameters are unchanged. Added language values: xml, groovy, properties. Kotlin uses existing kotlin, including .gradle.kts. No migration needed. Local incremental ingestion discovers newly supported files; completed immutable Git imports require --force.
