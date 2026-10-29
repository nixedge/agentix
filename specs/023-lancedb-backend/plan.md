# Implementation Plan: LanceDB search storage replacement

**Branch**: `023-lancedb-backend` | **Date**: 2026-10-28 | **Spec**: [spec.md](spec.md)

## Design

Add a library crate `agentix-store` with no network listener and no inference dependency. It owns persistent Lance tables, schema/model validation, stable keys, project merging, record/state CRUD, search primitives, index maintenance and a nonblocking filesystem writer lock. Search and indexer depend on this one crate; no duplicate backend implementations or selectable PostgreSQL runtime remain. Existing public search row types and MCP tools stay stable.

Each searchable collection has a Lance table with common typed search/filter columns, raw fixed-size-list embeddings and a JSON payload retaining source-specific fields. Preserve nullable vectors for migrated rows; vector queries exclude nulls. A separate metadata table stores repository and sync-state JSON under stable composite keys. Schema metadata pins version/model/dimension; configurable local path defaults to XDG data directory. Connection handles refresh committed versions for independent readers. Writers use a shared in-process mutex and filesystem lock across processes; writes fail explicitly when another process holds the lock.

Use pinned LanceDB0.40.0/Lance13/Arrow58 from benchmark022, including the previously approved remote feature workaround; storage URIs remain local. IVF_HNSW_FLAT uses cosine, 64 partitions at adequate row counts, m20/construction ef400, query ef100/20 probes initially. Smaller tables use exact search. FTS/scalar indexes support predicates before candidate selection. Hybrid retrieval uses 60 candidates per retriever (or more for larger requested output), RRF k60 and deterministic tie-breaking. Explicit optimize/reindex commands maintain indexes after incremental writes; new fragments remain searchable before maintenance.

Refactor code/docs writers around whole-file preparation and atomic replacement, retaining shared existing chunkers and local embedding calls. Reuse GitHub download/pagination/content handling but replace SQL persistence and watermarks, propagate stream failures. Packages/git/repository listing/pruning use the shared store. Replace db.rs SQL with Lance query adapters preserving row types and formatter/reranker callers. Route all CLI/MCP entry points through one environment configuration. PROJECTS scope must apply consistently to CLI and MCP code/docs/list/file paths; GitHub follows the existing unscoped contract.

Provide a read-only psql export script and a streaming Rust JSONL importer for all legacy collections/state. No SQL runtime dependency: export uses an operator-supplied PostgreSQL client, import consumes files. Require a fresh destination and explicit model assertion, validate counts/types/vectors and retain a manifest. No live export or deployment is implicit in implementation authorization.

## Constitution check

Library-first and local inference preserved. Library code never binds a listener; the dedicated storage server binary is an explicit exception for the user's server/laptop topology. No jail filesystem mounts are expanded; launchers must permit the API with optional bearer authentication endpoint through their existing network policy. The shared storage responsibility does not pull reranking/inference into the indexer. Nix/locked builds and real backend integration remain required. User explicitly chose replacement, superseding PostgreSQL-specific storage convention and integration requirement for this stack; update those narrow constitution references to LanceDB without weakening other invariants. Home-agent SQL/sled and unrelated runtime components remain outside scope. No production deployment or database deletion.

## Validation

Meaningful actual-backend tests cover code/docs/GitHub rows, project/repo/language/kind/state filters, deterministic fusion, insert/update/delete and shorter/empty/deleted file replacement, project retagging, sync error/watermark persistence, process reopen and incompatible model/dimension rejection. A fixture embedding HTTP server may exercise ingestion protocol without downloading a production model; deterministic stored vectors test storage/search independently. Migration tests cover all entities and nullable embeddings. Use Nix for builds/tools; test default workspace as feasible and record existing/unrelated failures before changing approaches. Canonical Nix builds for ingest and MCP are completion gates; full workspace and CI remain merge gates.

## Structure

- agentix-store/: shared Rust storage library and real Lance integration tests.
- agentix-search/src/db.rs: existing result API backed by LanceDB.
- agentix-indexer/: all ingesters, registry, pruning, import/stats/index maintenance.
- agentix-mcp-server/: unified local/remote Store initialization and package/query paths.
- perSystem/, flake/nixosModules/, justfile, README.md, .env.example: operational configuration.
- scripts/export-postgres.sql and specs/023-lancedb-backend/: migration contract, reproduction and evidence.

## Remote-store amendment

Keep the shared Store API and add an HTTP transport to it, so all search/indexer/MCP paths retain their behavior. AGENTIX_STORE_URL selects remote mode exclusively; the laptop does not open/create an index. A versioned serde request enum represents the existing storage operations; typed results are serialized as JSON. The storage server binary belongs to agentix-store and dispatches onto one local Store. It binds a listener only in its binary; library code builds a router/client but never binds. This is a narrow, explicit exception to the constitution's previous sole-listener rule, required by the user's server/laptop topology; inference daemon behavior is unchanged.

Bearer authentication is optional for trusted LAN deployments. When configured, apply it to all routes (runtime file preferred for NixOS). Disable redirects on clients to avoid credential redirection. Handshake pins protocol, embedding identity/dimensions. HTTP operations have bounded request size and a concurrency semaphore. Reads run concurrently, mutations serialize through existing Store locks. Remote read-modify-write metadata uses compare-and-swap under the server writer lock with bounded conflict retries; never retry an ambiguous failed mutation. A configured shared token represents trusted index access, not tenant isolation. Firewall opening remains opt-in.

Add a Nix package for the storage binary and services.agentix.storage options with a dedicated state directory/user, runtime token file, model/dimensions, host/port and optional firewall. Update laptop MCP environment and server ingestion recipes. Extend actual Lance/HTTP tests plus subprocess MCP coverage. Re-run focused tests/clippy and canonical server/MCP/indexer builds; previous validation does not establish remote readiness.

## LAN authentication adjustment

Represent the credential as Option<String> across configuration, client and router. With
None, the server skips only the bearer check; model/schema guards, concurrency limits and
body limits remain. With Some, existing authentication behavior remains. NixOS tokenFile
is nullable with default null; LoadCredential and its environment path are conditional.
The primary LAN example needs only the server address and client AGENTIX_STORE_URL.

The storage binary/check now use a dedicated Store dependency artifact built with all
targets. Prior verification showed the indexer-root artifacts caused storage-only builds
to recompile a different dependency feature graph; a separate cache avoids repeating that
work for each storage source change without altering runtime behavior.

## Large Git ingestion recovery

Reuse embedding backoff for HTTP 429/5xx and retain bounded diagnostic text from
failed responses. Exercise retry recovery, exhaustion and permanent errors with a
local HTTP fixture. Persist a separate `git_ingest` metadata record: mark incomplete
before writes and complete only after code, requested docs and repo registry succeed.
Only completed revisions with the requested docs coverage may skip ingestion; older
records without completion state safely walk/hash the checkout again. Prune both
registry and completion state. Test partial-data recovery, code-only to docs upgrade,
and completed-revision skipping with a disposable local Git repository, then rebuild
the clients and rerun/timestamp the user's large remote ledger ingestion.

## Inference input bound correction

Modify the active agentix-llama backend in this worktree. Set the embedding context
cap to 8,192, validate token length against both context and configured decode-batch
limit before touching native state, and map ContextExceeded to HTTP 400 on both
embedding endpoints. Keep source chunks intact; do not silently truncate or average
partial embeddings. Extend the real GGUF regression to reject a too-long input and
then successfully embed another request. Build the inference package and clients;
the operator must update agentix-llama before resuming the live ledger workload.

## MCP operational parity

Keep ingestion logic in agentix-indexer. Add a package completion helper and shared
maintenance policy using existing Store metadata/CAS/maintain operations, so remote
clients work with the already-deployed storage API. Auto-maintain a touched collection
on first tracking, after 10% row-count drift clamped to 64–512 rows, or after five
minutes with dirty work. Persist dirty generations and a 15-minute optimization lease;
preserve changes arriving during optimization and report failures without claiming
that already-committed ingestion rolled back.

Add typed MCP operation/job contracts in a separate operations module. start_ingestion
supports local code/docs, Git, registry packages, GitHub and explicit local import.
ingestion_status, cancel_ingestion and retry_ingestion use shared metadata; workers
heartbeat every five seconds and stale jobs become interrupted after 30 seconds.
Bound concurrent background ingestions per MCP with a semaphore. Cancellation drops
the ingest future; Git subprocesses must use kill_on_drop. Existing fetch_package
remains synchronous/compatible, gains force, and uses the same completion and
maintenance helpers. Maintenance runs through optimize_index (background job), and
index_status/list_sources expose counts, registry, sync, jobs and maintenance state.
prune_index exposes repo/local/old-versions targets with dry_run=true by default.
Reuse existing library functions rather than shelling out to ingest.

Validate with real Lance HTTP + two stdio MCP clients, deterministic embedding fixtures,
a disposable Git/local repo, job cancellation/interruption and maintenance leases.
Run full workspace tests, strict changed-crate Clippy, formatting, canonical client
builds, then read-only/live reversible MCP smoke against the deployed store. Update
tool instructions, README and quickstart with the primary MCP workflows.
