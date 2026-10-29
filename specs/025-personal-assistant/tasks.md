# Tasks: personal assistant

## A — Durable modular foundation (complete)
- [x] T001 Inspect current stores, allocate025 after all branches/worktrees/specs, verify host path, and complete spec/plan/contracts before code.
- [x] T002 Add engine-free agentix-assistant-core with typed documents, commands, receipts, errors and repository/projection contracts.
- [x] T003 Add agentix-assistant-sled with guarded schema, atomic command/index/receipt/outbox batches and explicit durable acknowledgement.
- [x] T004 Add revision-safe CRUD and list membership/reference rules, coalescing projection jobs and conditional durable acknowledgements.
- [x] T005 Add actual storage tests for retries/mismatch, concurrent edits, integrity, stale ack, tombstones, validation/schema and reopen recovery.
- [x] T006 Add canonical Nix package/check, update lockfile, run tests/strict Clippy/format and document actual evidence.

## B — Rebuildable personal retrieval (on merged023 base)
- [x] T007 Refresh025 onto merged LanceDB base and review engine utility reuse; no edits to active023 worktree.
- [x] T008 Implement separate personal projection adapter/profile, lexical retrieval, safe single-owner concurrency and version guards.
- [x] T009 Implement outbox worker with durable retry/ack and revision-safe projection application; indexing outage never rolls back saves.
- [x] T010 Implement search rehydration, tombstone suppression and fresh-index rebuild; test with real Sled and LanceDB across restart and worker interruption.
- [ ] T011 Evaluate personal-note embeddings independently of code embeddings and configure a pinned profile; test model/dimension/version mismatch and embedding outage.

## C — Conversational backend and phone
- [x] T012 Implement deterministic tool handlers over the core; connect recent messages and retrieved context to inference through interfaces, preserving router policy.
- [x] T013 Implement request/conversation lifecycle and append-only message persistence; validate clarification and correction turns with scripted and actual local-model tests.
- [x] T014 Expose a versioned tell API in existing agentix-agent with bounded requests/timeouts and owner-access configuration; test actual storage effects/retries.
- [ ] T015 Evaluate Qwen2.5-7B-Instruct via agentix-llama on grocery/list/note/task/correction cases; record latency, correctness and memory. Backend swap remains an adapter concern.
- [ ] T016 Connect Android's existing tell seam over WireGuard only after server acceptance; preserve the user's default assistant until real backend usefulness is verified.

## D — Operational assistant
- [ ] T017 Add due-time index and durable reminder/delivery state with revision invalidation, restart recovery and documented at-least-once external-effect semantics.
- [ ] T018 Add versioned quiescent export and fresh-store restore, checksums/file manifests, tested search rebuild and deletion retention policy; gate irreplaceable production use on this.
- [ ] T019 Define/review explicit legacy todo/session migration and test on copied fixtures; never mutate production as incidental setup.
- [ ] T020 Add sourced facts/attachments and separate Google/calendar/mail delivery adapters as individually tested increments.
- [ ] T021 Run full workspace/Nix gates and deploy/live/phone acceptance only for the completed integration scope; publish no claims from mocks alone.

## C execution detail — approved before semantic T011
- [x] T022 Define versioned tell/intent/turn/message contracts and additive journal schema before implementing C.
- [x] T023 Implement real-Sled append-only conversation and durable immutable plan/response lifecycle with retry/reopen/concurrent admission coverage.
- [x] T024 Implement bounded application state machine, exact tool resolution, context/history delivery and receipt-derived replies; test storage uncertainty and cancellation recovery.
- [x] T025 Add opt-in protected HTTP route and local gateway adapter to agentix-agent, with real storage and HTTP contract tests and canonical Nix checks.
- [x] T026 Run explicit synthetic acceptance against the installed Qwen baseline; record correctness and latency separately from scripted tests.
- [x] T027 After server acceptance, wire HTTPS Android tell client, explicit enablement/private owner token, durable retry envelope and contract tests; build APK and record physical acceptance limits.
- [x] T028 Replace generic model replacement with owner-approved typed item-text/task-completion tools; validate target/revision and preserve unrelated fields in Rust, with actual-Sled regression tests. Actual-model evaluation is recorded under T026.

T026 now covers13 operation/replay cases across three fresh stores using the approved two-step adapter. Capability and ambiguous-time cases pass; Android implementation and APK checks are complete. T016 remains open for actual server deployment/phone acceptance; T015 runtime memory and T021 broader gates remain unestablished.
- [x] T029 Add typed unsupported capability answers and deterministic no-write replies; actual-store tests plus live follow-up/address/confirmation acceptance.
- [x] T030 After T029 acceptance, implement Android immutable pending-envelope lifecycle, private opt-in configuration and bounded HTTPS client; test retry/recreation/identity/endpoint guards and build canonical APK.
- [x] T031 Implement owner-approved two-step local inference: bounded operation selection, per-operation field schema, deterministic capability rejection, mismatch/no-repeat-replay tests and full live acceptance.
