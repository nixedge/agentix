# Data model and invariants

## Local session journal

`Session`: schema_version=1, UUID, canonical project/common-Git root, parent_session_id, selected_model_id, effective settings, created/updated times, last committed sequence. Store under `~/.ax/sessions/<id>/` with metadata.json, events.jsonl and a writer lock. Directory 0700/files 0600. No auth tokens or raw environment dumps.

`TranscriptItem`: session_id, monotonically increasing sequence, turn_id, optional invocation_id, kind, bounded payload, timestamp. Kinds include user, assistant_delta/final, tool_started/result, approval_request/decision, context_compacted, model_changed, worker_event, terminal_outcome. Tool-start is durable before execution; tool-result after completion. A crash between them means outcome_unknown, not permission to replay.

`Turn`: immutable model/settings snapshot, input sequence, usage/output/tool budgets, cancellation state and terminal outcome. One active turn per session. Events from other turns cannot complete it.

`Approval`: request_id, session/turn/invocation IDs, tool, canonical cwd, argument digest, allowed roots, pending/allow/deny status and lifetime. Persisted decisions are audit records; resume never revives a pending or expired grant.

## Turn machine

States: Idle, Preparing, Compacting, Generating, ValidatingCalls, AwaitingApproval, ExecutingTool, EvaluatingProgress, Finalizing, Cancelling, Completed, Failed, Cancelled.

| From | Allowed next states |
|---|---|
| Idle | Preparing |
| Preparing | Compacting, Generating, Failed, Cancelling |
| Compacting | Generating, Failed, Cancelling |
| Generating | ValidatingCalls, Completed, Failed, Cancelling |
| ValidatingCalls | AwaitingApproval, ExecutingTool, Finalizing, Failed, Cancelling |
| AwaitingApproval | ExecutingTool, EvaluatingProgress, Cancelling |
| ExecutingTool | AwaitingApproval, ExecutingTool, EvaluatingProgress, Failed, Cancelling |
| EvaluatingProgress | Generating, Finalizing, AwaitingApproval, Failed, Cancelling |
| Finalizing | Completed, Failed, Cancelling |
| Cancelling | Cancelled, Failed |
| Completed / Failed / Cancelled | Idle |

Each edge additionally validates required payloads/counters; an enum alone is insufficient. Denials and skipped-budget calls produce matching tool results. Stagnation enters an intervention represented by AwaitingApproval with a distinct intervention payload. Cloud escalation is a typed, approved invocation. Finalizing disables all tools and has a single bounded model attempt.

## Model records and operations

`ModelRecord`: opaque id, display_name, aliases, sha256, byte_size, source {repo_id, revision_commit, filename}, format, architecture, context_limit, tool_profile, capabilities, installed status and resident snapshot {unloaded, loading, ready, busy, failed}. Do not persist transient resident state as truth after restart.

`ModelOperation`: UUID, schema_version, idempotency_key, immutable artifact identity, kind (install_and_warm / warm), phase, downloaded/total bytes, current sequence, cancel_requested, error_code, model_id. Progress is monotonic within a phase; stage changes are explicit.

Transitions: queued → resolving → downloading → verifying → installing → warming → ready. Already-installed paths skip download/verify only after trusted artifact lookup; warm-only starts at warming. Any nonterminal phase can cancel/fail; installing is atomic, so a cancelled operation can leave an installed blob but cannot activate a session. Restart marks nonterminal jobs interrupted; partials are not resumable in v1 and are cleaned under the artifact lock before an explicit new attempt.

A per-artifact lock deduplicates installation. Capacity reservations and in-use model guards prevent eviction during turns. Operation readiness is not a permanent residency guarantee. The client commits selection only at a turn boundary after readiness and context checks.

## Orchestration (PostgreSQL)

Implement the 019 concepts in idempotent `scripts/schema.sql` migrations with a schema-version gate. Tables: orch_runs, orch_tasks, orch_task_dependencies, orch_workers, orch_execution_records, orch_intervention_requests, orch_messages, orch_worktrees, orch_launches, orch_role_capabilities, orch_artifacts and orch_input_manifests.

- **Launch / RoleCapability**: authenticated host certificate fingerprint/operator, repository/worker bounds, launch UUID, hashed capability, role, run/session/worker/generation binding, expiry and revocation. Raw secrets stay host-side; jailed clients use their own relay socket. Supervisor-only operations require host authentication and never accept worker roles.
- **Run**: UUID, parent_session_id, launch_scope_id, repository identity, immutable base commit, base_choice (clean_head / explicit_commit), recorded excluded dirty paths, worker limit, model/settings snapshot, status and detach flag.
- **Task**: UUID, run_id, description, acceptance_criteria, priority, target_files, preferred_model hint, explicit_model_override, state, retry_count/max_retries, assigned_worker_id, lease_generation, role_capability_hash, lease_expires_at, input_manifest_hash, input_version, approved_artifact_hash and cancellation/terminal-intent reason.
- **Dependency**: source_task_id waits for target_task_id Completed with an approved sealed artifact; same-run only, no cycles/self-reference. Insert graph transactionally. Input manifests list the transitive closure in deterministic topological order with each ancestor once.
- **Worker**: UUID, run_id, launch_scope_id, actual model/settings, host assignment, process identity, heartbeat and state. Scope comes from launch registration, never from task text.
- **Worktree**: UUID, run/task IDs, host absolute path, branch, base commit, supervisor identity, lifecycle state, prepared input/tree hashes and retained-result metadata. Unverified inputs cannot enable model execution.
- **Execution**: task/worker/generation IDs, start/end, outcome, model snapshot, bounded summary, test evidence references and diff/worktree references. Retain history after worker exit.
- **Artifact / InputManifest**: content-addressed immutable task-local delta, task/generation, exact input hash, before/after path hashes, file modes, bounded payload size, supervisor seal, parent approval and retained storage reference. InputManifest records run base, ordered approved ancestor artifacts, resulting tree hash and version; changes revoke stale prepared assignments.
- **Message / Intervention**: stable ID, task, sender role, text, creation/delivery/resolution state, optional resolution notes. Follow-ups are delivered at turn boundaries and acknowledged once.

Task states: Pending, Preparing, InProgress, Revoking, Reviewing, Blocked, Completed, Failed, Cancelled. Pending → Preparing atomically reserves an assignment/capacity after dependencies have approved sealed artifacts. Preparing → InProgress requires authenticated supervisor acknowledgment of the exact materialized input/tree hashes. Preparation conflict → Blocked before any model starts and only after the supervisor confirms the preparation helper has stopped. Preparing timeout/cancel → Revoking as for active work; no replacement may race its helper process.

Every active result, failure, intervention, expiry or cancellation enters Revoking after fenced identity validation. Only trusted supervisor termination confirmation permits Pending (bounded retry), Failed, Cancelled or Blocked. Successful stopped results additionally require artifact sealing, then Revoking → Reviewing; parent approval of that exact hash permits Reviewing → Completed. Review rejection → Blocked; cancellation before approval → Cancelled. Unblock may return Blocked → Pending only with stopped writers and valid dependencies/inputs. Completed/Failed/Cancelled are terminal; an explicit new task is required after those terminal states. Failed/cancelled dependencies block dependents with a reason. Retries of nonterminal failed attempts get a new generation and the same immutable input unless an explicit input-version update was reviewed.

`SELECT FOR UPDATE SKIP LOCKED` provides atomic eligible claims; all updates include authenticated role/ownership/generation/capability predicates. Submission idempotency keys and result IDs deduplicate transport retries. A stale result cannot overwrite current history/state. Capabilities are hashed at rest and redacted in logs; raw values exist only in the supervisor relay and daemon issuance response. There is no unconditional exactly-once external side-effect claim.

## Resource bounds

Defaults: 20 calls/turn, one generation/session, two workers when enabled (configured maximum four), 1 MiB persisted text per tool result with a 64 KiB displayed tail, 10 MiB per individual event rejected, 1000 tasks/run. Over-limit content is truncated with byte counts before persistence; optional larger artifacts require explicit local retention configuration. Session journals rotate at 64 MiB segments without discarding history. Artifacts have a default 64 MiB total and 10,000-path cap per task; oversized transfers block explicitly instead of silently dropping files. Model downloads respect administrator disk/residency budgets and one active writer per artifact. Limits are configuration values with upper bounds validated at startup.
