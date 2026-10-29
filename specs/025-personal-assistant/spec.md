# Feature Specification: Agentix personal assistant core

**Feature Branch**: `025-personal-assistant`
**Created**: 2026-10-29
**Status**: Durable records and lexical retrieval implemented; semantic evaluation and service/operational increments remain open
**Input**: Build a maintainable, modular personal assistant using Sled for authoritative records and LanceDB for retrieval. Android transcription is working and stays unchanged until real backend interaction works. Keep inference in agentix-llama; a later agentix-qwen adapter must not require storage/client changes.

## Product boundary

A single-owner private assistant captures instructions, saves information, manages commitments, retrieves relevant memory, and reports actual outcomes. Android, desktop and future MCP clients share an application service. The model proposes typed operations; deterministic validation and durable persistence decide whether an operation succeeds. Search results are candidates, never authoritative task state.

Initial record types are notes, tasks, named lists and list items. Later increments add append-only conversation messages, sourced facts, reminder delivery, attachments and external integrations. No live legacy database or deployed index is modified during development. No Android default-assistant switch until backend acceptance.

## User scenarios and acceptance

### US1 — Durable capture and editing (P1)

Given a valid command with a request ID, when a note/task/list/item is created or revised, then the record, request receipt, lookup indexes and projection job commit atomically and a successful response follows a storage flush. Given an uncertain response, replaying the identical request returns the original result without another mutation; reusing the ID for different content fails. Given a stale expected revision, an update fails rather than overwriting newer work. Restarting storage preserves acknowledged records and pending jobs.

### US2 — Exact commitments (P1)

Given tasks and named lists, exact lookup/listing does not require a model, embeddings or LanceDB. List items reference a live list. Deleting a nonempty list fails with a useful conflict rather than leaving orphan items. Completing/editing a task uses its revision. Dates are stored as instants; interpretation of relative dates belongs to the later application layer with an explicit owner timezone.

### US3 — Searchable memory (P1)

Given a committed original, a worker creates a LanceDB projection asynchronously. Projection failure does not fail or undo the authoritative save; jobs survive restart. Replaying jobs is safe. Concurrent edits cannot allow an older projection to overwrite a newer revision. Search results are checked against the authoritative revision and deletion status. Deleted content cannot reappear after replay or rebuild. A fresh index can be reconstructed from authoritative records, including tombstones, without calling the generation model.

### US4 — Natural language and conversation (P2)

Given tell(text) with a conversation ID, request ID and timezone context, the service supplies relevant recent messages and retrieved memories to an inference interface. Tools validate typed parameters. Saved conversation history must actually reach inference. Unclear intent or date asks a question. A response distinguishes saved data, pending work, and completed effects. Tool-call retries cannot duplicate mutations. A fake/scripted inference fixture validates control flow; local-model acceptance separately checks Qwen2.5-7B-Instruct before choosing any replacement.

### US5 — Reliable reminders and external actions (P2)

Due-time lookup is exact and independent of semantic search. Scheduling intent and delivery state are durable. Restart/retry does not silently drop reminders. Delivery is at-least-once unless a recipient supports an idempotency key; never claim universal exactly-once side effects. Editing/cancelling a commitment invalidates obsolete pending delivery. Existing Google/ntfy integrations remain isolated adapters.

### US6 — Recovery and ownership (P1 before production use)

An explicit versioned export preserves authoritative records, receipts and pending work. Restore validates input into a fresh store and is tested by rebuilding search. Consistent exports coordinate with writers; copying a live database directory is not the backup contract. Original audio/attachments are separately backed-up files with references and checksums. Legacy todos/sessions require explicit reviewed migration, never implicit in-place rewriting.

## Functional requirements

- FR-001: Sled owns authoritative records; LanceDB contains replaceable search projections. Each original has one authority, a stable UUID, schema version, revision and timestamps.
- FR-002: Domain types and service contracts have no dependency on Sled, LanceDB, HTTP, an LLM SDK or daemon internals. Storage and inference adapters are replaceable.
- FR-003: Commands validate nonblank bounded content, compatible record kinds and references. Unknown/unsupported persisted schema versions fail clearly.
- FR-004: Successful command acknowledgement requires atomic record/index/receipt/outbox storage and a successful explicit flush. Flush failure returns an uncertain storage error; replaying the request can recover its outcome.
- FR-005: Request IDs are caller-generated; receipts bind an ID to the complete typed command and original result. Revision checks reject lost updates. No semantic deduplication based on similar text.
- FR-006: Deletes retain versioned tombstones. Record type and identity cannot change through an edit. List deletion with live items is rejected; moving an item must validate its new parent.
- FR-007: Projection jobs coalesce by record identity and carry a revision. Acknowledgement removes only the job actually processed; a later revision stays pending. Workers never perform network/model calls inside a Sled transaction.
- FR-008: Retrieval has typed filter/limit interfaces. Search hits carry source identity/revision; execution and final presentation rehydrate current originals. Explicitly report unavailable/incomplete search when appropriate.
- FR-009: Embedding model/revision/dimension and index schema are validated. Personal-memory embeddings are independent of code embeddings and conversation inference. Changing generation model does not require re-embedding.
- FR-010: Initial generation baseline is configured Qwen2.5-7B-Instruct via agentix-llama. Generation context/concurrency is bounded; models are configured rather than hardcoded in domain code. No new cloud escalation path.
- FR-011: Runtime blocking storage operations run on blocking workers; async library boundaries do not block Tokio executor threads.
- FR-012: Keep existing code indexing, legacy home-agent stores, deployed services and Android behaviour unchanged until explicit integration/migration tasks.
- FR-013: Tests cover actual Sled persistence, concurrency, replay, deletion/reference validation, schema guards and reopen recovery. Projection integration uses actual LanceDB; scripted adapters test only orchestration failure paths.
- FR-014: Nix provides canonical packages/checks. Document completed tests separately from unperformed power-loss, live-model, phone, delivery and production-load acceptance.
- FR-015: No transcripts, secrets or production databases in Git/build outputs/logs. Development fixtures are synthetic. Filesystem paths and errors must not falsely imply a successful action.
- FR-016: Owner bearer authentication is optional for WireGuard-only deployments. A configured token is enforced before inference/effects, and invalid or missing explicitly configured secrets fail startup. Android omits Authorization when its optional token is empty. Authentication changes preserve pending-request identity and replay semantics; network-only mode relies on the deployment's existing ingress restrictions.

## Success criteria and increments

A. Durable foundation: typed command API, real Sled adapter, concurrent/replay/reopen tests, Nix check. No natural-language execution or service route claimed.
B. Search projection: separate LanceDB adapter after worktree 023 is merged; outbox coordinator, real lexical retrieval, revision filtering and rebuild integration. Semantic embeddings follow an explicit profile, not an inherited code-model assumption.
C. Assistant service: typed tools and conversational context, tell endpoint, local inference evaluation, then Android client connection over WireGuard. No new public listener is introduced by a library.
D. Reminders, migration and recovery: scheduled delivery, explicit legacy import/export/restore and operational acceptance. Backup/restore gates irreplaceable production data.

Each increment must be usable/testable independently. Increment A and lexical retrieval tasks T007–T010 are implemented and validated on merged master. Semantic evaluation T011 and increments C/D remain open; their integrations are not represented by success stubs. See quickstart.md for completed checks and acceptance boundaries.
