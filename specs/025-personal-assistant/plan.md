# Implementation Plan: Agentix personal assistant

Branch `025-personal-assistant`, created 2026-10-29 from the 023-lancedb-backend snapshot, refreshed onto merged master (`c489476`) after verifying identical source trees, then onto master `cd83a25` after Android024 merged. Worktree verified at `/home/sam/home/agentix/025-personal-assistant`. The merged storage API is available here; do not edit other worktrees.

## Module boundaries

- `agentix-assistant-core`: typed records, commands, results, errors and repository/projection/inference contracts. Pure Rust domain/application library with no storage engines or listeners.
- `agentix-assistant-sled`: the authoritative repository implementation, isolated serialization/key layout, transactional command handling, indexes, replay receipts and projection outbox. Depends on core, never the daemon/search adapter.
- `agentix-assistant-lance`: personal-memory projection schema and lexical retrieval implementation; semantic retrieval awaits T011 evaluation. Depends on core plus the existing pinned LanceDB stack, never on Sled at runtime. Separate namespace/path from the code index and independent profile. Reviewed/reused suitable engine patterns from merged023; no additional remote server or code-index schema fork.
- Future application coordinator: composes repository, projection and inference traits; existing agentix-agent hosts HTTP/tools and background scheduling. Existing agentix-router remains responsible for provider selection; do not add routing policy in storage or Android.

## Increment A design

Core exposes `RecordBody` variants Note, Task, List and ListItem. A record envelope contains id, revision (starts at one), schema_version, created_at/updated_at and a deletion flag. A typed Create/Replace/Delete command is enclosed by a caller UUID request_id. Replace/Delete require expected_revision. Record bodies remain structured documents; no database-specific types escape the adapter.

Sled adapter uses one owned Db behind a shared mutex; all operations run through spawn_blocking. A single keyspace uses prefixes for schema, records, request receipts, list-item membership and projection jobs. One atomic batch commits all related changes while the repository serializes access; no other API exposes the raw Db. This deliberately favours a small understandable concurrency boundary over many optimistic retry closures. Separate Sled processes cannot share the directory; remote callers use the later owning service.

Final Sled handle release also performs blocking work. Last-handle Drop offloads engine destruction when called from Tokio; explicit `SledRepository::close(self)` awaits flush and destruction on a blocking worker for deterministic shutdown/reopen. Close rejects remaining clones or in-flight operations rather than pretending the directory lock was released.

Creation/replacement validates the target and references inside the same serialized operation. Deleting a list with live members fails. Replacing an item moves its membership index atomically. A receipt stores the typed command and original resulting record. Identical retry returns the original receipt even if the entity later changed, but still flushes before acknowledgement. Reusing the request ID with another command fails. An explicit Db flush occurs before returning a success; if flush fails, the same request ID must be retried to resolve the outcome.

Projection jobs use entity ID as key and coalesce to the latest revision. `ack_projection` compares identity/revision before removing the entry. Deletes preserve tombstones and enqueue their revision. No embeddings are produced in A. Pending jobs and exact record listing are model-independent. Version headers prevent an incompatible schema being silently opened; foreign or nonempty unversioned databases are rejected. Do not open the legacy AgentDb as a new personal store.

## Increment B design

Review of merged `agentix-store` confirms its collection allowlist, record keys, schema metadata and embedding columns are specific to repository/GitHub indexing. Personal documents will use a dedicated `agentix-assistant-lance` adapter with the same pinned LanceDB 0.40.0 and Arrow 58 versions and established query/merge/FTS patterns. Reusing the code-index Store directly would misrepresent personal records as code documents and couple their profiles. No code-index schema or server API changes are needed.

The first retrieval profile is explicitly lexical-only, `personal-lexical-v1`, with no vectors/model dependency. Table schema and profile metadata are checked at open, including empty indexes; unknown tables/profiles are rejected. Semantic profile selection/evaluation remains T011 and cannot be claimed from lexical fixtures. A lifetime filesystem lock permits one local owner; clones share an async gate for revision comparison and application. Path/lock operations run on blocking workers. No remote personal-index writer is introduced.

One table retains each source ID, revision, kind, deletion state, searchable text and serialized original envelope. Tombstones retain revision barriers and are excluded from search. Older applications are ignored; equal revisions must carry identical envelopes. A single row merge commits each application. FTS indexing must be present before a job can be acknowledged; retries repair an interrupted index update.

Core orchestration remains engine-free: a bounded worker pass loads authoritative records, applies the latest original, then acknowledges only the job observed. A failed/cancelled pass leaves unapplied/unacknowledged work durable. New edits remain queued. Search rehydrates candidates and verifies identity/revision/kind/deletion, reports discarded candidates and outstanding projection work, and never returns stale text. Rebuild applies a stable authoritative snapshot to a fresh destination without acknowledging source jobs; the normal worker subsequently catches concurrent edits. No deletion of a live index is implicit.

Real Sled/LanceDB tests cover lexical queries/filter/limits, out-of-order replay and tombstones, restart after projection-before-ack interruption, pending-index lag, changed/deleted search candidates, fresh rebuild, single ownership and incompatible schema/profile rejection. Scripted projection failure tests only the outage boundary; actual retrieval tests use LanceDB.

## Runtime and model decisions

The initial conversational baseline is Qwen2.5-7B-Instruct Q8_0 through agentix-llama, not the Jina embedding checkpoint. Treat its suitability as an evaluation question. Jina code embeddings remain for code indexing; personal retrieval profile is selected independently. Backend interface boundaries allow a later agentix-qwen provider. RTX3090/24GB is the initial host; the owner also has a 48GB unified-memory Mac mini available, not deployed by this feature. The combined 12–16GB VRAM allowance for the baseline and code embedder is an estimate, not a measured SLO. No model download/deployment is required for A.

## Constitution check

Library-first: new crates have distinct domain and engine roles and no listening ports. Local-first: no new model/provider calls. Layering: APIs and adapters are independent; existing inference router remains authoritative. State machine: command lifecycle Received → Validated → AtomicallyApplied → Flushed → Acknowledged; failures after application are uncertain outcomes recovered by request replay, not blind mutation retries. Projection Pending → Applied → Acknowledged, with later revisions retained. No new agent loop in A. Tests use real engines. Nix gates run before claiming a deliverable. No jail/mount or credential changes.

The existing constitution names agentix-store as owner of the code-search index; personal projections must integrate consistently with that boundary in B. A introduces only Sled authority and a core projection contract, so no amendment is required. Any new listener, rather than reuse of agentix-agent, requires a separately reviewed scope change.

## Validation

Core validation tests; real temporary Sled DB tests for idempotency and request mismatch, concurrent revision updates, item reference/deletion rules, membership moves, outbox stale acknowledgement, schema guards and close/reopen recovery. Reopen tests establish persisted behaviour, not power-loss certification. Nix package/check `assistant-core` runs both crates' tests without an LLM/GPU. Strict targeted Clippy and formatting; full workspace tests remain the merge gate and must be reported distinctly if not run.

## Complexity tracking

Two crates in A are justified by an engine-free public domain API and a swappable Sled adapter. Do not add an empty Lance crate, generic plugin framework, event-sourcing framework, database abstraction over arbitrary SQL, or a second daemon. Keep commands exhaustively typed and module responsibilities narrow.
