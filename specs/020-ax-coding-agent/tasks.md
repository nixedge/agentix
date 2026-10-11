# Tasks: Local coding agent with orchestration

**Input**: spec.md, plan.md, research.md, data-model.md and contracts/.
**Status**: Implementation in progress from 2026-10-11. Checkboxes reflect completed tasks only; partial work and validation are recorded in quickstart.md.
**Tests**: Mandatory under the constitution. Use Nix for every Cargo invocation. A failed approach/test must be reported before choosing a different approach. No automatic commits, pushes or merges.

Task numbers are sequential. Unless a dependency is explicitly stated, each task depends on prior tasks in its phase and on that phase's prerequisites. No `[P]` markers are used: shared contracts, Nix files and ownership changes make speculative parallel implementation unsafe. Test-design tasks may define expected failures; do not confuse intentionally unimplemented assertions with a failed approved implementation approach.

## Phase 1 — Upstream reuse and reproducible foundation

- [x] T001 Audit upstream Codex release `092d3acd6bec3e3a14bdc7e7a2810ab628ab759d` PTY/file-search/apply-patch/execpolicy/MCP candidates, exact transitive dependencies, editions/MSRV, workspace patches and licenses; record adopt/reject reasons and compare third-party codex-protocol 0.63.0 in `specs/020-ax-coding-agent/research.md` (FR-031, FR-032).
- [x] T002 Add the approved pinned upstream dependencies, compatible toolchain and reproducible source hashes in `Cargo.toml`, `Cargo.lock`, `flake.lock`, `perSystem/devShells.nix` and `perSystem/packages.nix`; update MSRV statements in `AGENTS.md` and `README.md` after verifying the actual toolchain (FR-031, FR-032, FR-034).
- [x] T003 Exercise minimal upstream adapters and preserved behavior cases in `agentix-harness/src/upstream.rs`, `agentix-harness/tests/upstream.rs` and `vendor/codex-apply-patch/{src,tests}`; demonstrate PTY I/O/cancel, file-search roots, patch conflicts and policy behavior before equivalent implementation; record incompatibility instead of silently replacing libraries (FR-031, FR-032, SC-008).
- [x] T004 Pin a redistributable single-file GGUF under 50 MB with exact source/hash/license in `perSystem/packages.nix`, expose it to tests, and define deterministic protocol fixtures in `agentix-harness/tests/fixtures/` plus reference-host metadata in `specs/020-ax-coding-agent/quickstart.md` (FR-034, SC-001, SC-003, SC-008).

**Gate**: candidate reuse decision, dependency closure and actual fixture are established before feature implementation. No full Codex runtime/SQLite dependency is accepted incidentally.

## Phase 2 — Shared contracts and local backend prerequisites

- [x] T005 Define versioned agent/model/orchestration DTOs and strict decoding in `agentix-api/src/{lib.rs,agent.rs,models.rs,orchestration.rs}` with compatibility tests in `agentix-api/tests/contracts.rs`; bump agentix-api and agentix-daemon from 0.1.0 to 0.2.0 in their manifests, update dependent constraints/Cargo.lock, and document the legacy cloud-routing migration (FR-006, FR-020, FR-033).
- [x] T006 Implement checked turn transitions, typed errors and exhaustive transition tests in `agentix-harness/src/{agent.rs,event.rs,session.rs}` and `agentix-harness/tests/transitions.rs`, including approval, cancellation, budgets, stagnation and one bounded finalization (FR-002, FR-004, FR-005, FR-006).
- [x] T007 Extend `agentix-infer/src/{lib.rs,engine.rs,pool.rs}` with typed tool exchange, cancellation, readiness/residency and capacity interfaces; test guards, duplicate loads, load errors and in-use eviction protection in `agentix-infer/tests/lifecycle.rs` (FR-007, FR-008, FR-013).
- [x] T008 Adapt the relevant 015 concepts into `agentix-llama/src/{lib.rs,tool_template.rs}` with supported-template validation, tool-call IDs/arguments, strict parsing and bounded repair; test unsupported/malformed templates and calls in `agentix-llama/tests/tool_protocol.rs` (FR-006, FR-007, FR-013).
- [x] T009 Implement typed streaming/tool completion, worker cancellation and private UDS readiness/tokenization controls in `agentix-llama/src/main.rs` and `agentix-llama/tests/worker_api.rs`; use actual fixture generation and deterministic protocol tests without ignored substitutes (FR-005, FR-007, FR-013, SC-001, SC-004).
- [x] T010 Wire actual daemon proxy paths and local-only model IDs in `agentix-daemon/src/gateway/mod.rs` and `agentix-router/src/lib.rs`; test aliases resembling cloud names and legacy routes in `agentix-daemon/tests/local_routing.rs` (FR-007, FR-033).
- [x] T011 Enforce typed escalation for every cloud entrypoint in `agentix-api/src/agent.rs`, `agentix-router/src/lib.rs`, `agentix-daemon/src/gateway/{mod.rs,escalation.rs}` and `agentix-harness/src/tools/ask_cloud.rs`; require router-issued non-deserializable ValidatedEscalation at every provider executor, with private construction and compile-fail bypass tests. Test malformed/disabled requests and every legacy cloud route for rejection with zero provider traffic, preserving known local IDs/aliases (FR-023, FR-033).

## Phase 3 — US6 sandbox baseline and US1 coding MVP

- [x] T012 [US6] Separate launch profile construction into `agentix-jails/src/ax_jail/profile.rs`; remove implicit SSH-agent exposure, sanitize direnv/env/proxy variables, disable executable Git configuration/hooks/includes and enforce mount grants in `agentix-jails/src/ax_jail/main.rs` (FR-021, FR-022).
- [x] T013 [US6] Add real bubblewrap sentinel, credentials, Git-config, single/all-worktree, path-alias and Nix socket tests in `agentix-jails/tests/ax_profile.rs`; verify absolute host paths and document shared-network exception/security review criteria in `ARCHITECTURE.md` (FR-021, FR-022, SC-006).
- [x] T014 [US1] Implement a shared controller/config path in `agentix-ax/src/{controller.rs,config.rs,main.rs}` using the typed harness; keep direct unjailed tool execution disabled and preserve existing positional/stdin flags (FR-001, FR-007, FR-021).
- [x] T015 [US1] Implement typed SSE consumption and complete-call validation in `agentix-harness/src/client.rs` with split-frame, duplicate-ID, malformed-argument and truncated-stream cases in `agentix-harness/tests/streaming.rs` (FR-002, FR-006).
- [x] T016 [US1] Implement scoped approval requests/decisions, timeouts and headless denial in `agentix-harness/src/approval.rs` and `agentix-harness/tests/approvals.rs`; bind decisions to arguments/cwd/turn and enforce no-shell/no-cloud (FR-004, FR-006).
- [x] T017 [US1] Wrap approved upstream search/patch primitives in `agentix-ax/src/tools/{file_ops.rs,search.rs,patch.rs}`; retain user-approved upstream filename-search symlink behavior; add read/edit cwd/root checks, symlink-race resistance, expected-content conflicts and bounded binary/content handling (FR-003, FR-032, FR-035).
- [x] T018 [US1] Wrap upstream process/PTY primitives in `agentix-ax/src/tools/shell.rs` with bounded streaming, real exit codes, timeout, process-group termination/reaping and input handling; test child/grandchild cleanup in `agentix-ax/tests/processes.rs` (FR-003, FR-005, FR-035, SC-004).
- [x] T019 [US1] Connect streamed text/tool/approval events to `agentix-ax/src/tui/{mod.rs,ui.rs}`; implement terminal cleanup, Unicode editing, paste, resize and live interruption with terminal tests in `agentix-ax/tests/terminal.rs` (FR-001, FR-002, FR-004, FR-005).
- [x] T020 [US1] Implement headless JSONL events and exit codes in `agentix-ax/src/main.rs` and `agentix-ax/tests/headless.rs`, including approval-required and interrupted-output behavior (FR-001, FR-020).
- [x] T021 [US1] Run coding acceptance cases in `agentix-ax/tests/coding_flow.rs`: read/search/edit/test/follow-up, denied edit, external edit conflict, broken stream, cancellation and truthful final summary; verify actual local daemon/worker transport separately with the pinned model (FR-024, SC-001, SC-004).

**MVP checkpoint**: one existing configured compatible local model can perform the coding workflow inside a tested jail. Full model picker, persistence and delegation follow.

## Phase 4 — US2 existing model selection

- [ ] T022 [US2] Implement model catalog snapshots and readiness jobs in `agentix-daemon/src/gateway/models.rs` and `agentix-infer/src/model_operation.rs`, keeping installed, resident and coding-compatible states distinct (FR-008, FR-013).
- [ ] T023 [US2] Add Local model picker, status and turn-boundary selection transaction in `agentix-ax/src/tui/model_picker.rs` and `agentix-ax/src/controller.rs`; reject stale selection generations and recheck context capacity (FR-008, FR-009, FR-019).
- [ ] T024 [US2] Test loaded/cold/busy/incompatible, concurrent switch, failed/OOM load and disconnect recovery in `agentix-daemon/tests/model_lifecycle.rs` and `agentix-ax/tests/model_picker.rs` (FR-009, FR-013, FR-014, SC-002).

Phase 4 checkpoint: installed catalog snapshots and interactive `/model` selection
are implemented, including filtering, warm/readiness checks, tokenized history plus
4096-token coding reserve, turn-boundary commit, and stale-response isolation.
Client/PTY tests and the real daemon/worker catalog test pass. T022–T024 remain open
for durable readiness jobs, exact rendered-context budgeting, OOM/disconnect job
recovery and complete lifecycle coverage; this is not full Phase 4 acceptance.

## Phase 5 — US3 Hugging Face discovery and installation

- [ ] T025 [US3] Implement daemon Hub catalog adapter in `agentix-daemon/src/hub.rs`, recording actual primary API endpoint/query/redirect fixtures in `agentix-daemon/tests/fixtures/hub/`; support search/file pagination, immutable revision and explicit-file lookup (FR-010, FR-011).
- [ ] T026 [US3] Add exact artifact verification, partial-file handling, expected hash/size checks, locks, atomic manifests and existing-blob reuse in `agentix-infer/src/store/{hf.rs,blob.rs,manifest.rs}` with integrity and race tests in `agentix-infer/tests/download.rs` (FR-011, FR-012).
- [ ] T027 [US3] Implement daemon download/job persistence, byte/stage SSE, idempotency, cancellation and restart interruption in `agentix-daemon/src/gateway/models.rs`; keep Hub credentials in daemon config and nonblocking jobs bounded (FR-012, FR-014, FR-035).
- [ ] T028 [US3] Add Hugging Face search/file/confirmation/progress flows to `agentix-ax/src/tui/model_picker.rs` with exact quantization/revision/size and license/gated explanations; activate only after warm/context checks (FR-010, FR-011, FR-012, FR-013, FR-019).
- [ ] T029 [US3] Apply management access policy to new and legacy mutating endpoints in `agentix-daemon/src/{config.rs,gateway/mod.rs,gateway/ollama_manage.rs}`; test forbidden sources, redirect token stripping and secret-free errors in `agentix-daemon/tests/model_security.rs` (FR-014, FR-021, FR-035).
- [ ] T030 [US3] Exercise catalog/download/install/warm end-to-end in `agentix-daemon/tests/hub_flow.rs` and `agentix-ax/tests/model_picker.rs`: offline, 403/404/429, split GGUF, corrupt hash, unknown metadata, disk full, cancel and restart; retain previous active model in every failure (FR-014, SC-002).

## Phase 6 — US4 sessions, context and integrations

- [ ] T031 [US4] Implement versioned journal, atomic metadata and writer locks in `agentix-ax/src/session_store.rs` with tool-start/result durability and bounded retention from data-model.md (FR-015, FR-035).
- [ ] T032 [US4] Add new/list/resume/fork and recovery UI/CLI in `agentix-ax/src/{main.rs,commands.rs,controller.rs}`; test corruption, incomplete tails, schema mismatch and two writers in `agentix-ax/tests/sessions.rs` (FR-015, FR-019, SC-005).
- [ ] T033 [US4] Implement instruction provenance/precedence and lazy explicit skill loading in `agentix-harness/src/context.rs`, `agentix-ax/src/config.rs` and `agentix-harness/tests/instructions.rs`; enforce bounded sources and no hook auto-execution (FR-016, FR-035).
- [ ] T034 [US4] Implement tokenizer-based budgets, local compaction and source-span journaling in `agentix-harness/src/context.rs`; test preserved instructions/decisions/tool pairs and smaller-model switches in `agentix-harness/tests/context.rs` (FR-017, FR-035).
- [ ] T035 [US4] Adapt the selected compliant MCP client in `agentix-ax/src/mcp.rs`, preserving configured stdio servers with namespace/schema checks, approvals, initialization, timeout/cancel and shutdown; add protocol tests in `agentix-ax/tests/mcp.rs` (FR-004, FR-018, FR-032).
- [ ] T036 [US4] Finish help/status/diff/plan/permissions/skills/mcp/quit command dispatch and settings visibility in `agentix-ax/src/commands.rs` and TUI files; test unknown commands, busy-turn rules and read-only plan mode in `agentix-ax/tests/commands.rs` (FR-019, FR-024).
- [ ] T037 [US4] Exercise restart/fork/skill/MCP/compaction and no-tool-replay acceptance in `agentix-ax/tests/session_flow.rs`; measure UI p95 under 100 scripted turns and 10 MiB tool output in `agentix-ax/tests/responsiveness.rs` with recorded host details (FR-015, FR-016, FR-017, FR-018, SC-003, SC-005).

## Phase 7 — US5 durable orchestration foundation (absorbs 019)

- [ ] T038 [US5] Create `agentix-orchestrator/{Cargo.toml,src/lib.rs,src/db.rs}` and register the pure-Rust library in workspace manifests; implement idempotent versioned tables from data-model.md, including launches, hashed role capabilities, artifacts and input manifests, in `scripts/schema.sql` (FR-027, FR-033).
- [ ] T039 [US5] Implement checked run/task/worker/lease transitions in `agentix-orchestrator/src/model.rs`, including Preparing, Revoking and Reviewing; implement dependency graph validation in `src/graph.rs` and unit tests for illegal edges/cycles/cross-run references, prepared-input gates and reviewed-artifact completion (FR-025, FR-027, FR-029).
- [ ] T040 [US5] Implement transactional DAG submission, priority/eligibility, atomic claims and capacity reservations in `agentix-orchestrator/src/queue.rs`; derive scope from authenticated launch registration, require approved predecessor artifacts and bind claims to immutable input versions (FR-025, FR-026, FR-027, FR-029).
- [ ] T041 [US5] Implement host-held fenced role capabilities/generations, heartbeats, result idempotency, revocation and bounded retries in `agentix-orchestrator/src/lease.rs`; require authenticated supervisor process-stop acknowledgment before retry or sealing and revoke capabilities on lease termination (FR-027, FR-028, FR-030).
- [ ] T042 [US5] Implement blocked interventions, follow-up delivery, cancellation, detach/reattach and durable execution/evidence history in `agentix-orchestrator/src/queue.rs` (FR-025, FR-029, FR-030).
- [ ] T043 [US5] Add mandatory real PostgreSQL tests in `agentix-orchestrator/tests/{claims.rs,leases.rs,recovery.rs}` for two workers/20 tasks, duplicate submissions/results, stale generations/input versions, supervisor loss, Preparing/Reviewing transitions and cancellation dependencies; fail setup clearly rather than silently skipping DB coverage (FR-027, FR-028, FR-029, FR-030, SC-007).

## Phase 8 — US5 daemon adapters and trusted host supervision

- [ ] T044 [US5] Implement the fixed mTLS supervisor enrollment, certificate fingerprint allowlists, revocation and 15-minute host-held role capabilities from contracts/orchestration.md in `agentix-daemon/src/{config.rs,gateway/orchestration.rs}`; enforce parent/worker/supervisor operations and scopes, with forged, expired, revoked and missing-identity tests (FR-021, FR-025, FR-028).
- [ ] T045 [US5] Implement daemon orchestration HTTP routes, including authenticated launch registration, prepared-input acknowledgment, artifact sealing and parent review, and database assembly in `agentix-daemon/src/{main.rs,config.rs,gateway/orchestration.rs}`; database credentials stay daemon-side and local model snapshots/capacity are authoritative (FR-025, FR-026, FR-027, FR-029, FR-033).
- [ ] T046 [US5] Create `agentix-orchestrator-mcp/src/{lib.rs,main.rs,bin/orchestrator_ctl.rs}` with compliant MCP stdio and operator CLI adapters over daemon DTOs through session-specific UDS relays; implement all documented operations including artifact review without raw credentials in clients or temporary production stubs (FR-018, FR-025, FR-029, FR-033).
- [ ] T047 [US5] Implement opt-in bounded host supervisor in `agentix-jails/src/ax_jail/supervisor.rs` and main.rs: require clean HEAD or explicit `--base <full-commit-id>`, record excluded dirty paths without modifying them, reject delegated submodule edits, create uniquely named persistent sibling worktrees, verify paths and launch only the fixed packaged worker command (FR-021, FR-022, FR-025, FR-028).
- [ ] T048 [US5] Add per-child mount/session profiles, host-owned HTTP-over-UDS relays with fixed role/route/body limits, no-grandchild default, model/settings inheritance and explicit overrides in `agentix-jails/src/ax_jail/{profile.rs,supervisor.rs,relay.rs}` and `agentix-ax/src/main.rs` worker mode. Mount only each client's socket; reject mount aliases overlapping host keys/state and never expose role capabilities to children (FR-021, FR-026, FR-028).
- [ ] T049 [US5] Connect runtime heartbeats, independent supervisor revocation, process-group kill/reap, interrupted-write reporting and authenticated stopped acknowledgments in `agentix-jails/src/ax_jail/supervisor.rs` and `agentix-ax/src/controller.rs`; successful results stop and seal before Reviewing, with parent approval required for Completed (FR-005, FR-027, FR-028, FR-029, FR-030).
- [ ] T050 [US5] Test real parent/child jail isolation, direct HTTP and forged-header attacks, cross-socket access, revoked certificates/capabilities, protected-key/state mount aliases, stale-writer retry, supervisor crash and preserved worktrees in `agentix-jails/tests/ax_workers.rs`; require zero unauthorized writes/credential exposure (FR-021, FR-022, FR-028, SC-006, SC-007).

## Phase 9 — US5 delegation UX and result integration

- [ ] T051 [US5] Add explicit delegation tools/settings and `/agents` view in `agentix-ax/src/{commands.rs,controller.rs,tui/agents.rs}`; display task/DAG status including Preparing/Reviewing, input/artifact hashes, worker/model, approvals and resource capacity (FR-019, FR-025, FR-026, FR-029).
- [ ] T052 [US5] Add follow-up, inspect, cancel, unblock, detach and reattach actions in `agentix-ax/src/tui/agents.rs` using the daemon contracts; surface unknown process states and denied scope changes (FR-029, FR-030).
- [ ] T053 [US5] Implement bounded sandboxed artifact capture/materialization in `agentix-jails/src/ax_jail/artifacts.rs`: seal stopped-worker task-local deltas, enforce manifest/path/type/size/hash checks, and apply each approved ancestor delta exactly once in deterministic topological order with exact preimages. Bind prepared inputs and retries to immutable hashes; block conflicting or missing artifacts before execution. Add frozen-artifact inspection/approval in `agentix-ax/src/controller.rs`; keep integration into the parent separate through normal approved tools in `agentix-ax/src/tools/patch.rs`, preserving trees on conflict without auto-push/merge/delete (FR-024, FR-028, FR-029).
- [ ] T054 [US5] Exercise two-worker chain/diamond DAGs with actual predecessor file changes, exactly-once ancestor deltas and dependent compilation, inherited/overridden models, cancellation/retry, parent detach/reattach and conflicting integration in `agentix-ax/tests/orchestration_flow.rs` against real PostgreSQL and bwrap. Test dirty tracked/index/untracked rejection and explicit-base exclusion without changing parent contents/index, artifact tampering/limits, unreviewed or conflicting inputs and stale input versions (FR-025, FR-026, FR-027, FR-028, FR-029, FR-030, SC-007).

## Phase 10 — Release validation and documentation

- [ ] T055 Audit per-event/output/session/download/task bounds, secret redaction and terminal escape handling across `agentix-harness/tests/limits.rs`, `agentix-daemon/tests/redaction.rs` and `agentix-ax/tests/terminal.rs` (FR-035, SC-003, SC-006).
- [ ] T056 Complete Nix packages/source filters/dependency artifacts for `ax`, `ax-jail`, `agentix-api`, `agentix-harness`, `agentix-infer`, `agentix-llama`, `agentix-daemon`, `agentix-orchestrator`, `orchestrator-mcp`, `orchestrator-ctl` in `perSystem/packages.nix`; wire daemon/model-store/PostgreSQL services, mTLS provisioning, private host credential/state directories and sandboxed artifact helpers in `perSystem/processes.nix` and the actual NixOS module (FR-033, FR-034).
- [ ] T057 Document scope/parity, upstream pins/notices, supported templates/formats, model/Hub operations, session migration, 0.2 cloud-route migration, supervisor identity provisioning/revocation, relay permissions, clean/explicit committed bases, frozen-artifact review/dependency inputs, network exception and non-NixOS launch in `README.md` and `ARCHITECTURE.md`; update `AGENTS.md` only for verified facts (FR-031, FR-032, FR-036).
- [ ] T058 Validate every scenario in `specs/020-ax-coding-agent/quickstart.md`, including one interactive model-selection/delegation demonstration, and record actual evidence/limits in that file (FR-024, FR-034, FR-036, SC-001, SC-002, SC-004, SC-005, SC-006, SC-007).
- [ ] T059 Run constitution gates through Nix: fmt, clippy with denied warnings, full workspace tests with actual PostgreSQL/small-model fixtures, Nix builds for all T056 deliverables and CI; record results in `specs/020-ax-coding-agent/quickstart.md` and do not claim completion on skipped required checks (FR-034, SC-008).

## Dependencies and delivery

Phase 1 blocks upstream adoption. Phase 2 blocks the coding/model paths. Phase 3 starts only after Phase 2 and establishes the sandbox before tools execute. Phase 4 → Phase 5 delivers the complete `/model` flow. Phase 6 completes the shared session context used by delegation. Phase 7 → Phase 8 → Phase 9 implements orchestration; no external 019 service is presumed. Phase 10 requires all journeys.

US1 is demonstrable with a configured existing model after Phase 3. US2 is independently tested against its catalog fixtures after Phase 4. US3 adds discovery/download without depending on orchestration. US4 has its own restart/context scenarios. US5 depends on the shared coding/session/jail foundations but has a separate DAG acceptance test. US6's single-agent checks precede US1; its worker checks follow supervisor implementation.

Future work may split large phases into reviewed commits, each meeting applicable build/test rules. Do not label tasks parallel merely because their functions differ when they edit the same files. Task decomposition does not authorize autonomous publication, model downgrades or additional agents in this planning session.
