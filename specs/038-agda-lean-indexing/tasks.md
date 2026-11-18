# Tasks

T001–T002 establish discovery. T005 precedes T006, and T008 extends the same design to Go/Elm. T003/T007 cover storage after extraction; final gates T004/T009 depend on all implementation tasks.

- [x] T001 [US1] Enable Agda/Lean discovery and language labels in agentix-indexer/src/ingest/code.rs.
- [x] T002 [US2] Exclude .lake; add discovery and original-line fallback tests in agentix-indexer/src/ingest/code.rs.
- [x] T003 [US1] Test filtered local and remote ingestion, incremental replacement and deletion in agentix-indexer/tests/local_ingestion.rs.
- [x] T004 Document supported formats and refresh in README.md and quickstart.md; run Nix checks/build and record verification.md.
- [x] T005 [US1] Research and pin compatible Agda/Lean tree-sitter grammar dependencies, updating Cargo.lock.
- [x] T006 [US1] Implement and test grammar-based declaration extraction in agentix-indexer/src/ingest/symbols.rs; enable symbol chunking and preserve literate Agda coordinates.
- [x] T007 [US1] Update storage integration assertions and documentation for symbol chunks; repeat affected validation after T005–T006.
- [x] T008 [US1] Pin compatible Go/Elm grammars; add Elm discovery and elm-stuff exclusion; implement Go/Elm symbol extraction and fixture tests.
- [x] T009 [US1] Extend local/remote ingestion and documentation to all four languages; run final validation after T008.
