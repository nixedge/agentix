# Feature Specification: Replace search storage with LanceDB

**Feature Branch**: `023-lancedb-backend`
**Created**: 2026-10-28
**Status**: Implemented; user-deployed LAN store validated
**Input**: Following benchmark 022, replace PostgreSQL outright for code, documentation and GitHub indexing/search. The user explicitly rejected keeping docs/GitHub PostgreSQL-only and chose full replacement over an optional backend.

## User Scenarios & Testing

### User Story 1 — Search every indexed source without PostgreSQL (P1)

Given a local index containing code, documents, issues and PRs, MCP and CLI search use LanceDB for keyword/vector/hybrid retrieval and retain formatting/reranking and existing tool parameters. Language, symbol kind, repository LIKE, document kind, issue/PR state/type and project filters constrain candidate retrieval. list_repos and get_file use the same index and honor project restrictions. Empty indexes return empty results; malformed vectors/model or schema mismatches fail explicitly.

Independent test: real local Lance tables with deterministic fixture vectors exercise all search modes, filters, file order, repository counts and reopen visibility without PostgreSQL or a live model.

### User Story 2 — Ingest and maintain every supported source (P1)

Local code/docs, literate Haskell, Rust/Haskell/Python package fetches, git sources, GitHub issues/PRs/comments, repository metadata and sync watermarks persist locally. Unchanged files skip embedding; new project tags merge without erasing existing scopes. Replacing a changed file removes obsolete chunks and preserves prior file data until replacement is ready. Deleted files and prune operations remove their indexed data; dry-run does not mutate. GitHub state/title/metadata changes persist even when text is unchanged. Watermarks advance only after successful stream writes; stream failures return nonzero.

Independent test: ingest a fixture, repeat unchanged, shorten/edit/delete and retag files, reopen, query and prune; exercise GitHub fixtures and watermarks without external writes.

### User Story 3 — Move an existing index safely (P2)

A read-only PostgreSQL JSONL export and offline import preserve all six searchable entity collections, repo metadata, sync state, nullable embeddings, project arrays and timestamps. Import requires an explicit destination and model identity, validates dimensions/finite vectors, refuses a populated destination, records counts and fails on unsupported records. Production data is never modified or switched automatically. Nix/runtime instructions use the new persistent local index.

## Requirements

- FR-001: LanceDB is the sole search/index storage backend; remove PostgreSQL runtime connections and SQL dependencies from search/indexer/MCP. The separate home-agent sled storage is unchanged.
- FR-002: Include code_chunks, documents, GitHub issues/PRs and both comment streams, repository registry and sync state. No deferred PostgreSQL-only source types.
- FR-003: Keep MCP names/parameters and result formatting; retain current reranker and local embedding configuration. GitHub search retains current issue/PR scope; comments remain persisted for parity with existing ingestion.
- FR-004: Use cosine IVF_HNSW_FLAT with raw vectors; record/configure search effort and explicit index maintenance. Newly written rows remain visible before index maintenance; small tables can use exhaustive search.
- FR-005: Persist schema version, embedding model and dimensions, refusing incompatible reuse. Reject invalid/nonfinite/zero vectors before mutating existing data.
- FR-006: Stable composite keys, atomic per-file replacement, idempotent upserts and merged project tags; no SQL-expression interpolation without correct escaping. Reject invalid limits.
- FR-007: Support multiple readers/process reopen and serialize writers across processes with an explicit busy error. Never silently ignore storage errors or advance watermarks after failed persistence.
- FR-008: Bound migration batching; no whole-corpus embedding regeneration. Export is read-only and import is explicit/offline; do not alter live deployment.
- FR-009: Update Nix packages/shell/service configuration, environment examples, just commands and primary documentation. No PostgreSQL service is required for a fresh installation.
- FR-010: Use Rust, pinned Cargo dependencies and Nix. Run relevant unit/integration, formatting/clippy and canonical package builds; record unrun workspace gates honestly.

## Success Criteria

- All supported fresh-install ingestion/search/maintenance paths work against an actual local LanceDB index without a PostgreSQL process.
- Integration checks cover retrieval/filter isolation, metadata changes, replacement/deletion, concurrent-reader visibility, reopen/model guards and migration round-trip.
- Source data remains untouched; the change is reviewable in this feature worktree with a migration guide and validation record.

## Scope and assumptions

This replaces storage in the repository; it does not deploy, delete the old database or switch the user's installed services. Existing benchmark data stays in worktree 022. Current embedding-model identity is configured by EMBED_MODEL; historical source model identity is not provable, so migration requires an explicit operator assertion. Cross-collection commits are not claimed to be transactional. Crash/restart and concurrency coverage must state its actual extent rather than treating independent connections as crash-recovery proof.

## Deployment clarification — remote shared store (2026-10-28)

The MCP processes stay on laptops. A server owns the Lance directory and exposes an authenticated, versioned storage API. Multiple independent MCP clients must query it concurrently without mounting the directory. Local/offline ingestion remains available, and remote ingestion/package-fetch mutations use the same API. Existing stdio MCP tools and query embedding/reranking behavior are retained.

- FR-011: Configure remote access through AGENTIX_STORE_URL, with an optional runtime bearer credential. Trusted LAN clients need no credential when server authentication is unset. Never silently fall back to a local index on connection/auth/model errors. Reject ambiguous local-path/URL configuration.
- FR-012: The storage service shares one Store and writer mutex across requests. Reads can overlap; writes queue within the service. Atomic metadata updates must not lose concurrent project tags or regress sync watermarks. External local writers retain the existing busy error.
- FR-013: Validate protocol/schema/model/dimensions at connection; enforce authentication before reading request bodies when a token is configured, bound body size/concurrent request execution, return structured errors, disable automatic mutation retries on ambiguous network failure.
- FR-014: NixOS provides a storage-service package, persistent state, runtime credential-file configuration, listen address/port, model/dimensions and opt-in firewall opening. Bind loopback by default; document private-network access or TLS reverse proxy. MCP stays a laptop process, not a remote MCP server.
- FR-015: Actual HTTP integration tests use separate clients against real Lance tables and exercise concurrent reads/writes, atomic metadata conflicts, authorization, invalid input/model mismatch and shared visibility. Existing MCP/ingestion regressions remain required.

The previous local-filesystem-only deployment assumption is superseded. Earlier readiness/validation describes that implementation only; remote deployment completion requires the tasks below. API credentials grant trusted full-index access; PROJECTS remains a tool visibility filter, not per-tenant authorization. No deployment or secret provisioning is implied.

## LAN authentication clarification

The user's deployment is a trusted local LAN and does not require bearer authentication.
FR-011/013/014 permit an optional credential: absent server token configuration allows
LAN clients without Authorization; a configured token retains mandatory matching bearer
authentication. Unset client credential variables send no Authorization header. Invalid or
ambiguous configured credentials still fail explicitly. The NixOS tokenFile option defaults
to null, and no credential unit directives or token environment are emitted in that case.

## Large Git ingestion recovery — 2026-10-29

The live cardano-ledger master ingestion encountered HTTP 502 after 2,486 persisted
chunks. Embedding generation must retry transient HTTP 429/5xx with bounded backoff,
preserve the final response status/body in errors, and reject permanent HTTP errors
without retrying. Storage mutations retain their existing no-automatic-retry behavior.
A Git revision is skippable only after an explicit successful completion record,
including whether documentation was indexed. Failed/partial runs remain resumable;
an interrupted forced rerun must invalidate its prior completion record. Pruning a
repository also removes its completion state.

The supplied llama.log confirms the ledger failures abort inside llama.cpp when
one input exceeds n_batch. The active inference backend must check actual token
counts before native decode/encode and return ContextExceeded/HTTP 400 without
aborting or truncating. Its embedding context cap increases from 4,096 to 8,192
tokens, bounded by the model context, to accommodate the indexer's existing
8,000-byte code chunks intact. The storage model identity/dimensions do not change.
