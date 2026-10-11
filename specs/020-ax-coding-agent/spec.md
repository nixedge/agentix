# Feature Specification: Local coding agent with orchestration

**Feature Branch**: `020-ax-coding-agent`
**Created**: 2026-10-11
**Status**: Draft for review; specification, plan, tasks and analysis only
**Input**: Make `ax` and `ax-jail` work as close to Codex as practical using local models served by agentix-daemon. `/model` selects an existing model or searches Hugging Face and downloads/loads a selection. Include orchestration. Prefer reusing upstream Codex libraries to reimplementation.

## Scope and clarification

The target is a Linux terminal coding workflow: inspect a repository, plan changes, edit, execute commands/tests, review diffs, retain context, delegate bounded work and resume after interruption. Model reasoning quality is not promised to equal Codex. The feature includes the required local tool-calling/backend work; UI-only changes cannot satisfy it.

Clarified on 2026-10-11: orchestration is included, using feature 019's design as the starting point; upstream Codex library reuse must be evaluated before implementing equivalent components. Review remediation: all cloud routes must enforce structured escalation; supervisor control uses authenticated role separation; dependencies consume sealed reviewed changes; dirty-parent delegation requires an explicit committed base. No implementation is authorized in this planning stage.

Included: interactive and headless operation, project instructions/skills, stdio MCP, sessions, context management, local model management, bounded multi-agent work and sandbox lifecycle. Deferred: images/voice, OpenAI account integration, hosted Codex services, plugin marketplace, remote MCP OAuth, distributed workers, unrestricted model formats and training. Cloud escalation remains explicitly enabled and typed; it never replaces the primary local path.

## User Scenarios & Testing

### US1 — Complete a local coding task (P1)

A developer runs `ax-jail`, asks for a change, follows streaming progress, reviews tool actions and receives an accurate change/test summary.

**Independent test**: In a disposable repository with an existing compatible local model, read a file, apply a patch, run its test and ask a follow-up referring to the change.

1. **Given** a loaded compatible model and a writable project, **when** a coding request is submitted, **then** the agent can read/search, propose/apply edits, run tests and summarize observed results using that local model.
2. **Given** a requested tool action awaiting approval, **when** the developer denies it, **then** no side effect occurs and the model receives the denial as a tool result.
3. **Given** a running command or generation, **when** the developer interrupts, **then** work stops, the terminal remains usable and completed edits are retained without automatic rollback.
4. **Given** malformed or incomplete model tool arguments, **when** they arrive, **then** no tool executes and a bounded recoverable error is reported.

### US2 — Select an existing local model (P1)

**Independent test**: Open `/model` with one loaded model, one installed cold model and one incompatible model.

1. **Given** those models, **when** the picker opens, **then** each shows installed/resident/busy/capability status without equating a stored file with a loaded model.
2. **Given** a cold compatible model, **when** selected, **then** loading progress appears and the session changes only after readiness is confirmed.
3. **Given** an active turn or failed load, **when** another model is selected, **then** the current turn's model stays fixed; a failed switch preserves the previous selection and history.

### US3 — Find and load a Hugging Face model (P1)

**Independent test**: Through a controlled Hub fixture, search, choose a repository and exact GGUF file, confirm size/revision, download, verify, load and use it; repeat with a failed download.

1. **Given** search results, **when** a repository is selected, **then** supported files, quantization when available, size, revision and license/gating information are shown before download.
2. **Given** an explicit confirmed file, **when** downloading, **then** progress and cancellation are available; only verified data can become an installed model.
3. **Given** a gated model, missing file, rate limit, insufficient disk/RAM/VRAM, corrupt content or unsupported format, **when** the operation fails, **then** the reason and recovery action appear without changing the active model or exposing credentials.

### US4 — Continue work with context and integrations (P2)

**Independent test**: Save a two-turn session, restart, resume in the same repository, invoke a local skill and a configured stdio MCP tool, then compact the session.

1. **Given** a prior session, **when** resumed, **then** messages, completed tool results, selected model and work context are restored; interrupted side effects are never automatically replayed.
2. **Given** applicable project instructions and a selected skill, **when** starting a turn, **then** their sources and precedence are visible and lower-trust tool output cannot grant permissions.
3. **Given** context near the model limit, **when** continuing, **then** bounded compaction preserves user decisions, outstanding tasks and complete tool-call/result pairs or reports that a turn cannot fit.

### US5 — Delegate and supervise local agents (P1)

**Independent test**: Run a dependency graph containing two independent tasks and a dependent integration task against two jailed workers; kill a worker and cancel another task.

1. **Given** explicit delegation authorization and a worker limit, **when** tasks are submitted, **then** eligible workers claim them once per active lease, dependencies gate execution and progress appears in `/agents`.
2. **Given** concurrent editing tasks, **when** launched, **then** each receives an isolated worktree, bounded permissions and the parent's model/settings unless the user explicitly overrides them.
3. **Given** a worker crash, expired lease or cancellation, **when** recovery occurs, **then** stale results are rejected, the old writer is stopped before retry and retained work is visible for inspection.
4. **Given** stopped workers with sealed results, **when** the parent approves their exact artifact hashes, **then** dependent tasks receive those changes in verified inputs; parent-worktree integration remains a separate explicit action.
5. **Given** two predecessors with a shared ancestor, **when** their dependent is prepared, **then** each ancestor delta is applied once and any content conflict blocks execution before a model starts.
6. **Given** staged, unstaged or nonignored untracked parent changes, **when** delegation is launched without an explicit committed base, **then** launch stops without stashing, copying, committing or deleting those changes; an explicit base records their exclusion.
7. **Given** a worker attempts a supervisor-only request directly or through its relay, **when** it forges role/identity or a termination acknowledgment, **then** authorization rejects it and no retry becomes eligible.

### US6 — Launch predictably with isolation (P1)

**Independent test**: Exercise ordinary and bare-plus-worktree repositories, project dev shells and child workers in real bubblewrap namespaces using harmless sentinel files.

1. **Given** a normal launch, **when** the agent or its tools run, **then** only authorized roots are writable, host credential directories and SSH agents are unavailable, and Nix builds can use the configured daemon socket.
2. **Given** a single-worktree launch, **when** broader access or delegation is requested, **then** access remains bounded by the launch configuration; the UI cannot expand mounts.
3. **Given** terminal exit, cancellation or failure, **when** cleanup runs, **then** owned processes are reaped and session/worktree data remains available.

## Requirements

### Functional Requirements

- **FR-001**: Support interactive `ax` and headless `ax exec`, retaining the existing positional task invocation as a documented alias.
- **FR-002**: Preserve multi-turn conversation and correlated assistant/tool messages in one session; expose streaming text and tool progress.
- **FR-003**: Supply bounded file read/list/search, conflict-aware patch editing, command execution and diff inspection within authorized jail mounts. Per user clarification during implementation, upstream filename search follows symlinks unchanged; its starting root is not an additional filesystem boundary. Reads and writes still enforce their path/approval policy.
- **FR-004**: Enforce explicit approval policies for edits, execution, MCP and escalation; denial or no headless approval input must never imply consent.
- **FR-005**: Interrupt generation and tool work, terminate owned process groups and restore terminal state; report uncertain side effects after crashes.
- **FR-006**: Validate tool names, arguments and complete stream boundaries before execution, with per-turn budgets, timeouts and stagnation transitions.
- **FR-007**: Make all primary agent requests use the selected local model through the daemon; reject silent cloud fallback and incompatible coding models.
- **FR-008**: `/model` must list installed and resident models, capability status, current selection, and loading/busy/failure states.
- **FR-009**: Model switching must be transactional at turn boundaries; preserve session history and the previous selection on failure.
- **FR-010**: Search Hugging Face and browse exact supported files with pagination; permit explicit repository/file selection when search is unavailable.
- **FR-011**: Confirm immutable revision, exact filename, download bytes and known license/gating information before fetching; never silently choose a quantization.
- **FR-012**: Download through daemon-owned services with progress, cancellation, digest verification, atomic installation and content-addressed reuse; incomplete data must not be advertised as installed.
- **FR-013**: Warm a downloaded/installed model and verify coding capability before selection; serialize duplicate loads and avoid evicting in-use models.
- **FR-014**: Explain offline, authorization, rate-limit, unsupported-model, missing-file and resource failures without losing the usable session or leaking secrets.
- **FR-015**: Persist versioned sessions and support new/list/resume/fork; detect corruption and concurrent writers, and do not replay interrupted tool actions.
- **FR-016**: Load applicable AGENTS.md instructions and explicitly selected SKILL.md resources with deterministic precedence and provenance; do not auto-execute hooks.
- **FR-017**: Provide context usage and compaction while retaining authoritative instructions, user decisions, task state and complete tool exchange boundaries.
- **FR-018**: Support configured MCP stdio servers using a compliant client, namespaced tool names, initialization, timeouts, cancellation and shutdown.
- **FR-019**: Provide `/help`, `/model`, `/status`, `/new`, `/resume`, `/fork`, `/compact`, `/diff`, `/plan`, `/permissions`, `/skills`, `/mcp`, `/agents` and `/quit`; unknown commands do not become model prompts.
- **FR-020**: Headless mode must expose versioned JSONL events, a terminal outcome and stable exit codes for success, failure, cancellation and approval-required.
- **FR-021**: Execute agent tools inside bubblewrap; enforce authorized mount roots and secret-free environment/configuration in parent and child agents.
- **FR-022**: Support canonical sibling worktrees, opt-in broader mounts, project Nix environments and restricted Git configuration; never create ephemeral jail-only worktrees.
- **FR-023**: Keep cloud escalation off by default in ax; when enabled, require a typed request containing knowledge, question and local insufficiency, routed by agentix-router with permission enforcement. Every client/cloud entry point must use that path; legacy direct-cloud requests must fail before provider I/O with documented migration guidance.
- **FR-024**: Report actual edits, command statuses and remaining work; never infer test success from issuing a command or from a worker's unsupported claim.
- **FR-025**: Offer explicit delegation with task descriptions, acceptance criteria, dependencies, priority, worker count and visible ownership/status.
- **FR-026**: Inherit the parent's selected local model and quality settings for children unless the user explicitly chooses an override; enforce daemon resource/concurrency limits.
- **FR-027**: Use durable atomic task claims, fenced leases, bounded retries, heartbeats, human intervention and validated state transitions. Only authenticated supervisor control can register launches or confirm stopped workers; worker/parent roles cannot impersonate it, including direct daemon calls.
- **FR-028**: Isolate concurrent editing workers in persistent host worktrees and prevent stale/duplicate writers during retry. Freeze a committed run base; reject dirty-parent delegation unless the user explicitly selects a commit excluding local changes, without modifying the parent.
- **FR-029**: Support task/worker inspection, follow-up, cancellation and sealed result review. Before dependent execution, materialize approved predecessor deltas exactly once into verified inputs and block on conflict. Keep parent integration separately explicit, without automatic publication or cleanup.
- **FR-030**: Recover parent/worker interruptions with durable execution history and blocked-task reasons; session exit defaults to cancelling owned children, with explicit detach available.
- **FR-031**: Reuse suitable upstream Codex libraries before implementing equivalent primitives; record provenance, exact revision, license, coupling and the reason for every rejected reuse candidate.
- **FR-032**: Isolate upstream dependencies behind agentix-owned adapters; verify dependency/toolchain compatibility and behavior before adoption; do not equate third-party crate names with upstream provenance.
- **FR-033**: Keep wire contracts in agentix-api, agent mechanics in libraries, only daemon TCP serving, and TUI independent of daemon/router internals; preserve existing public APIs or document versioned breaks.
- **FR-034**: Provide reproducible Nix packages and automated unit, protocol, real-database, small-model, jail and end-to-end acceptance checks.
- **FR-035**: Bound transcript/tool-output/download/task resources and redact secrets from errors, events and logs; display truncation explicitly.
- **FR-036**: Record supported/deferred Codex workflow parity and backend/model limitations in user documentation, including non-NixOS launch instructions.

### Key Entities

Session, Turn, TranscriptItem, ToolInvocation, ApprovalDecision, ModelRecord, ModelOperation, UpstreamComponent, OrchestrationRun, Task, WorkerLease and WorktreeAssignment. Detailed states and ownership are specified in data-model.md and contracts/.

### Edge Cases

Concurrent model selection, a busy or evicted model, a split GGUF, stale search result, unavailable digest/size, download interruption/restart, disk exhaustion, and a model with no usable tool template. Session corruption, stale approvals, non-UTF8/binary files, symlink/path traversal, external edits before patch application, terminal escape sequences, oversized outputs and context smaller than retained instructions. Worker death during an edit, lease expiry while the old process lives, cyclic dependencies, cross-scope claims, conflicting child changes, cancelled dependencies and parent reconnection.

## Success Criteria

- **SC-001**: The deterministic coding acceptance suite completes all read/search/patch/test/follow-up scenarios in interactive and headless modes with correctly correlated tool results; a real pinned model separately completes a generation smoke test.
- **SC-002**: All loaded/cold/incompatible selection and Hub success/failure/cancellation cases pass; no failed operation changes the session's active model or publishes an incomplete blob.
- **SC-003**: For 100 scripted turns and a 10 MiB tool output, UI event processing p95 is at most 100 ms on the documented reference host; output storage/display stays within configured bounds. Generation speed and network transfer time are excluded.
- **SC-004**: Cancellation acknowledgment reaches the UI within 250 ms and owned shell groups exit within 2 seconds in controlled tests; inference cancellation is observed at the next token/batch boundary and no further tools run.
- **SC-005**: Resume/fork/crash tests preserve all committed transcript records, never replay a started side effect, and reject an incompatible schema without overwriting data.
- **SC-006**: Real jail tests show zero unauthorized sentinel-file writes or credential/socket exposure across parent and workers; documented network access is tested separately from filesystem isolation.
- **SC-007**: With two workers and 20 dependent tasks, real PostgreSQL/jail tests show no simultaneous valid claims, reject stale completions and forged supervisor requests, and stop old writers before retry. Chain/diamond dependents see approved predecessor changes exactly once; input conflicts prevent model start, and dirty-base tests leave parent files/index untouched.
- **SC-008**: Every deliverable builds with Nix and passes the constitution's format, lint, workspace-test and CI gates; upstream reuse candidates have recorded compatibility results before equivalent new implementations are accepted.

## Assumptions

- Linux/Nix is the initial target; one trusted operator controls a daemon deployment. Internet access is needed for uncached Hub discovery/download, not for existing local models.
- Only single-file GGUF completion models are initially downloadable through this flow. Split GGUF, safetensors conversion and arbitrary remote-code execution are rejected with explanations.
- A compatible chat/tool template and model context budget are necessary for coding mode; installing any GGUF does not establish that capability.
- Workers are local to the launcher host; the daemon and PostgreSQL may be on the configured trusted LAN. Multi-tenant authentication is not part of this feature, and management exposure must be administrator restricted.
- Feature 019's design is absorbed as a foundation, not assumed implemented. Its optional-test language and unfenced claims do not override this feature's required integration and recovery behavior.
- Upstream reuse is preferred, but no untested API compatibility or stable upstream Rust library guarantee is assumed. No SQLite storage is introduced.
