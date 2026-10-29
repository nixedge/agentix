# Tasks

- [x] T001 Allocate 023 from max existing/abandoned number, create sibling worktree from master and verify host path; inspect all existing search/ingestion/storage callers.
- [x] T002 Specify full replacement including docs/GitHub, design shared storage/migration and record constitution implications before implementation.
- [x] T003 Implement shared Lance storage library: schema/model/dimension guards, stable record/state keys, process writer lock, CRUD, project merging and file replacement.
- [x] T004 Implement actual keyword/vector/RRF search, predicate handling, metadata enumeration and HNSW/FTS/scalar maintenance with fixture integration tests.
- [x] T005 Replace search adapters and MCP/CLI initialization, package visibility and project scoping; retain reranking/output contract.
- [x] T006 Refactor code/docs ingestion with atomic whole-file replacement, incremental retagging and deleted/stale chunk handling; migrate packages/git/repo registry/pruning.
- [x] T007 Replace GitHub issue/PR/comment persistence and sync state; preserve metadata changes, propagate failures and test stream watermark behavior.
- [x] T008 Implement read-only legacy export and streaming fresh-destination importer for every collection/state; test round-trip and invalid input guards.
- [x] T009 Update Nix packages/dev shell/service configuration, constitution storage references, just/environment/primary docs and migration guide.
- [x] T010 Run formatting, clippy, unit/real Lance integration and ingestion/MCP smoke checks; canonical Nix package builds and appropriate workspace checks.
- [x] T011 Review scope/parity and working diff, record evidence/limitations, update indexed memory and leave live deployment untouched.

Dependencies: T003 before T004/T005/T006/T007/T008; T009 before canonical T010; all before T011. No commits/push/deployment implied.

Remote deployment amendment (supersedes local-only readiness):
- [x] T012 Define versioned storage request/response contract, authenticated HTTP client and configuration/handshake guards.
- [x] T013 Add shared-state storage server with concurrent readers, queued writes, bounded requests and atomic remote metadata updates.
- [x] T014 Add actual HTTP concurrency/auth/error tests and remote subprocess MCP/ingestion coverage.
- [x] T015 Add Nix storage-server package, NixOS service/state/credentials/network options and laptop/server deployment documentation; update architecture/constitution.
- [x] T016 Run formatting/clippy/tests, canonical Nix builds and module evaluation; update validation and indexed memory with remote deployment status.

- [x] T017 Make authentication optional for trusted LAN deployments, update NixOS/docs and verify unauthenticated/authenticated HTTP, MCP and server startup plus canonical packages.
- [x] T018 Exercise the user's deployed LAN store with real repo/package/GitHub ingestion, all MCP tools, concurrent clients, incremental replacement and maintenance; fix GitHub User-Agent and empty-success sync status, rebuild clients and record evidence/remaining relevance and file-reconstruction limits.
