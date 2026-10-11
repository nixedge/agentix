# Orchestration contract v1

This adapts feature 019's design into the daemon-backed ax architecture. No pre-existing implementation is assumed. Queue mechanics live in agentix-orchestrator; wire DTOs in agentix-api. A protocol version handshake rejects unsupported major versions. Existing 019 MCP shapes are design references, not a compatibility guarantee for an already deployed server; document v1 differences explicitly.

## Identity and transport

The host launcher/supervisor authenticates to the daemon using an administrator-provisioned TLS client certificate. The daemon pins the issuing CA and maps the certificate's public-key fingerprint to a configured host ID, operator identity, allowed repository identities, worker ceiling and allowed operations. Server identity is verified too. No unattended enrollment, caller-supplied host identity or trusted-LAN bypass is accepted. Missing/expired/revoked identity disables orchestration with a setup error; local single-agent inference remains usable. Revocation is checked on every control request and terminates active control streams.

The private key and supervisor state live outside every jail grant, Git common directory and all-worktrees mount. The launcher rejects any requested mount containing these paths, including aliases. Keys are never copied into child environment variables, arguments, logs or state. Certificate provisioning is an operator setup step; it is not performed by an agent. This identity authenticates host control, not a model-provider account.

The launcher registers a launch using its authenticated identity and a canonical repository/base plus configured limits. The daemon derives launch_scope_id and binds it to that host/operator and bounds. The daemon returns random 256-bit, short-lived role capabilities for parent and assigned workers, stored only in host supervisor memory (hashes/expiry/revocation stored daemon-side). Capabilities bind launch/run, parent session or worker/assignment generation, role and allowed operations. An administrator cannot grant a child permissions by changing task text. Default expiry is 15 minutes; only the authenticated supervisor can renew, and worker capabilities are revoked on lease revocation/completion. Host-only supervisor operations require the certificate and cannot accept parent/worker capabilities.

Each jailed process accesses an HTTP-over-Unix-socket relay owned by the host launcher. Only that session's socket is mounted at a fixed path; the containing host directory and other sessions' sockets are absent. Parent and workers get different sockets and no bearer credentials. The relay binds role/run/worker identity from its own socket configuration, ignores identity/authorization headers from the caller, allowlists methods/routes/body sizes and forwards to the daemon over mutually authenticated TLS with the host-held role capability. It is not a generic HTTP proxy or host-command service. Daemon authorization verifies both certificate and capability on every forwarded request. Direct HTTP from a jail without these credentials is rejected, even on loopback/trusted LAN. The only TCP listener remains agentix-daemon; relays are private local IPC in the launcher binary.

Parent capability: submit/inspect/follow-up/cancel/reprioritize/review/unblock/detach only within its run. Worker capability: inspect its assignment, heartbeat, submit result and request intervention only for its worker/task/generation. Supervisor authority (certificate, not a child capability): registration, assignment claims/preparation, stopped acknowledgment, artifact sealing and revocation only for its launch. Workers cannot register launches, create capabilities, approve results, launch descendants or acknowledge their own termination. Parent calls cannot impersonate supervisor calls.

On parent reattach or supervisor restart, require a new authenticated registration for the same operator and run, revoke old capabilities, inspect retained assignments and issue new session relays. PID numbers alone do not prove ownership: retain process start identity and pidfd while live. If termination cannot be established, block the assignment rather than retry. All identity changes are audited without secret material. Tests must cover direct HTTP bypass, forged headers, cross-socket/cross-run access, replay after revocation, certificate revocation and spoofed stopped acknowledgments.

## Operation surface

Daemon paths under `/v1/agentix/orchestration`; every route uses the role rules above:

| Operation | Required payload | Result |
|---|---|---|
| POST `/launches` | authenticated supervisor; repository/base, parent_session_id, worker_limit | registered launch_scope_id and bounded parent role capability |
| POST `/runs` | idempotency_key, launch_scope_id, parent_session_id, model/settings | run_id and validated immutable base/limits |
| POST `/runs/{id}/tasks` | idempotency_key, task graph, criteria, priorities, optional explicit overrides | task IDs; reject cycles/cross-run dependencies atomically |
| GET `/runs/{id}` | none | bounded authorized task/worker/intervention snapshot with cursor |
| POST `/claims` | authenticated supervisor; registered idle worker and model/settings | reserved assignment, generation, expiry, input_manifest_hash and host-held worker capability, or exhausted/paused/capacity_wait |
| GET `/assignments/current` | worker identity fixed by relay | already reserved assignment and preparation status; no claiming or scope changes |
| POST `/heartbeats` | assignment generation | acknowledged, lease expiry or revoked |
| POST `/results` | result_id, generation, outcome, summary, evidence references | accepted into Revoking for result sealing; never immediately Completed |
| POST `/interventions` | assignment generation, reason | Revoking, then Blocked after trusted process-stop confirmation |
| POST `/tasks/{id}/messages` | message_id, bounded text | queued acknowledgment; sender role derived from capability |
| POST `/tasks/{id}/cancel` | reason | revoking/cancelled status; no assertion that the process has already stopped |
| POST `/tasks/{id}/review` | exact sealed artifact hash, approve/reject, notes | Completed on approval of a successful reviewed result, otherwise Blocked |
| POST `/tasks/{id}/unblock` | resolution notes, optional corrected input manifest | pending only with prior writer stopped and validated inputs |
| POST `/tasks/{id}/priority` | priority | updated priority for future claims |
| POST `/assignments/{id}/prepared` | supervisor; generation, input manifest/tree hashes | enables model execution only after inputs were materialized/verified |
| POST `/assignments/{id}/stopped` | supervisor; generation, observed termination, terminal intent | permits sealing, retry, cancellation or blocking |
| POST `/assignments/{id}/artifact` | supervisor; frozen artifact/input hashes, bounded manifest | Reviewing after successful result and verified process stop |
| POST `/runs/{id}/detach` | explicit operator decision | retained supervisor/run ownership record |

The role capability is also the fenced assignment token for worker operations. Wire headers carry it only on the host-to-daemon hop. Worker JSON/MCP payloads never contain a token; the relay supplies identity and generation from the bound assignment and rejects a mismatched supplied generation.

## MCP and CLI

`orchestrator-mcp` is a compliant rmcp stdio client adapter using the session relay and agentix-api DTOs. Retain get_next_task, submit_result, request_human_intervention and heartbeat, adding generation/input-manifest fields to v1. get_next_task reads the supervisor-reserved assignment through `/assignments/current`; workers never create their own claims. Parent tools include submit_tasks, list_tasks, get_history, send_message, cancel_task, review_result and unblock_task. Transport role, not input text, determines availability and authority. Daemon checks remain authoritative.

`orchestrator-ctl` exposes add/add-batch/list/reprioritize/unblock/workers/history/interventions/cancel/review through the same relay; host operator use requires an authenticated launcher-created parent relay. Stable errors include scope_denied, identity_required, invalid_graph, stale_lease, dependency_blocked, input_conflict, dirty_base, capacity_wait, unknown_task and supervisor_unavailable. PostgreSQL credentials stay daemon-side.

## Committed base and dirty parent policy

Default launch freezes the exact parent HEAD commit only when both index and tracked worktree match HEAD and there are no nonignored untracked files. A dirty parent yields `dirty_base`, shows excluded paths without file contents and creates no run/worktree. Do not stash, clean, commit or copy the user's changes automatically.

The explicit launcher option `--base <full-commit-id>` authorizes using exactly that existing commit while leaving all parent modifications untouched. The launcher explains that staged, unstaged and untracked changes are excluded and records that choice in the run. Ignored files are always excluded. Alternatively the user can commit intended work and relaunch. Submodule-containing repositories are rejected for delegated editing in v1; users receive an explicit unsupported-repository error. Later changes to the parent never mutate a run's frozen base.

## Frozen results and dependency inputs

A successful worker result is a proposal. Stop/reap its entire sandbox and owned processes, then seal the result before review. The trusted supervisor captures a bounded artifact containing only the task's delta from its verified input tree, including explicitly reported new nonignored regular files, modifications, mode changes and deletions. Each path has expected before/after content hashes; manifests include task/generation, input hash, byte count and artifact SHA-256. Reject traversal, absolute paths, symlink/device/socket/submodule entries and oversized artifacts. Collection/materialization runs in a dedicated restricted helper sandbox, never by executing worker-provided host commands. Freeze artifacts in a host directory outside all jail writable mounts; live child worktree paths alone are not evidence.

The parent reviews the frozen diff and test evidence and approves the exact artifact hash. Only then is the task Completed. Approval permits using that artifact in downstream disposable worktrees; it does not apply it to the parent's tree. A no-change result has an explicit empty artifact. Rejection blocks the task for follow-up. No automatic parent merge, commit or publication occurs.

For each pending dependent task, compute the transitive predecessor closure and a deterministic topological ordering (task UUID breaks ties). Include each ancestor's approved task-local delta once, so a shared ancestor in a diamond graph is not applied twice. Materialize these deltas over the run's frozen base in a fresh task worktree using exact preimage checks, with no fuzzy/three-way automatic merge. Record the ordered artifact hashes and resulting tree hash as the immutable input manifest. Start the model only after the supervisor acknowledges successful materialization. The worker's final delta is against this materialized tree, not the original run base.

A missing/unapproved artifact or content/mode conflict blocks preparation before model execution; confirm the preparation helper has stopped before making the assignment eligible for any replacement. Retain the worktree for inspection and surface conflicting paths; never run the dependent task on an incomplete tree. The parent may provide an explicitly reviewed replacement input manifest, prepared/tested in an isolated integration worktree, or revise the graph; this creates a new input version and invalidates prior prepared assignments. Deterministic conservative conflict handling may require review even for overlapping changes that could be manually merged. Retrying an unchanged task reuses its exact sealed input; changing inputs creates a new assignment generation.

## Lease and retry semantics

Default heartbeat every 10 seconds, lease duration 60 seconds, maximum two retries after the initial attempt. Heartbeats originate in the runtime/relay, not model-generated text. Lease expiry immediately cancels local work; supervisor independently revokes and kills/reaps owned process groups. Revoking cannot transition to a new editing attempt until trusted stop confirmation. Supervisor unavailability leaves the task Blocked and ineligible. Repeated result_id with identical payload returns the original acknowledgment; differing payload conflicts. Results remain Reviewing until frozen-artifact approval; dependencies require Completed plus prepared verified inputs.

## Parent integration and recovery

Parent integration is a separate explicit action: apply the reviewed selected artifact(s) through ordinary parent approval/tool policy, checking the current parent content before edits. Changed parent files or conflicting results stop integration and preserve both trees. Worker success does not prove tests passed; inspect actual command evidence. Worktrees/artifacts persist until explicit cleanup.

Parent exit defaults to cancelling owned work. Detach is explicit; reattach reads durable run state and reports unknown process state. Cancellation/failure of prerequisites blocks dependents. Shared run budgets bound worker/task creation. Certificate/relay teardown revokes capabilities; teardown never deletes retained changes.
