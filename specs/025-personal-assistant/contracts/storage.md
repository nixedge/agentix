# Storage contract v1

Engine-free Rust API in agentix-assistant-core:

- apply(CommandRequest) → CommandReceipt, durable before success.
- get(UUID) → optional Record (tombstones are visible to repository/worker code).
- records() → complete stable snapshot of records including tombstones; initial personal-scale API, later pagination must preserve explicit snapshot semantics.
- pending_projections(limit) → jobs; invalid/unbounded limits rejected.
- ack_projection(job) → bool; removes only that exact pending revision and flushes before success.

Failures distinguish invalid input, absent record/reference, revision conflict, request-ID conflict, unsupported schema and storage I/O. An I/O error after mutation can have an uncertain outcome; the client retries the SAME request ID, never silently generates another. Successful replay returns the stored original receipt. No model, search service or network prerequisite.

Sled adapter lifecycle: `SledRepository::close(self)` flushes and awaits final database release on a blocking worker; remaining clones/in-flight operations cause an error. Use this when immediately reopening a path. Last-handle Drop also offloads blocking destruction on Tokio, but does not await directory-lock release.

Projection contract: apply original record (including tombstones) idempotently and without revision regression; search returns IDs/revisions/scores. Equal revisions with different original envelopes fail. Source records remain authoritative. `LanceProjection` implements this contract with an explicit lexical-only profile `personal-lexical-v1`; it does not call an embedder or generation model. Schema/profile/zero-dimension metadata must match even for empty tables.

`LanceProjection::open(path)` exclusively owns the dedicated local index directory for its lifetime; clones share that owner. `open_fresh(path)` also refuses an existing personal table, protecting a live index from accidental rebuild. Cancellation cannot release ownership/gating while an already-started apply is committing. A failure after row commit but before indexing is retried before acknowledgement.

Core orchestration:

- `project_pending(repository, projection, limit)` performs one bounded pass; loads the current original, applies it, then acknowledges the originally observed job. It stops on the first error; unacknowledged work remains for a subsequent pass. No autonomous retry timer or daemon is introduced here.
- `search_records(repository, projection, request)` validates a 1..100 result limit and nonblank text up to 4096 UTF-8 bytes. It rehydrates source IDs, suppresses deleted/missing/mismatched revisions and enforces the requested kind. It reports discarded candidates and whether any projection work remains. Invalid/incomplete retrieval must not be presented as an exhaustive answer. Pagination/refilling stale candidates is not implemented.
- `rebuild_projection(repository, fresh_destination)` applies a stable snapshot including tombstones without consuming pending jobs. Open a fresh directory explicitly; after rebuilding, run the worker to catch edits made after the snapshot. The caller later selects the new directory; no live deletion or automatic index swap occurs.

Search freshness is checked at each original read, not locked across an entire query and later tool execution. Later edits remain possible; mutation tools must still supply expected revisions. Projection outage propagates an error from retrieval but never reverses a successful original save.
