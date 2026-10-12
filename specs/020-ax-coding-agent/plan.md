# Implementation Plan: Local coding agent with orchestration

**Branch**: `020-ax-coding-agent` | **Date**: 2026-10-11 | **Spec**: [spec.md](spec.md)
**Base**: `68d7770`; implementation started 2026-10-11. Progress and validation are recorded in tasks.md and quickstart.md.

## Summary

Extend the existing Rust agentix stack into a persistent terminal coding agent, with transactional local/Hugging Face model selection and supervised local workers. Reuse upstream Codex components through narrow adapters before writing equivalent primitives. Absorb the useful feature 019 queue design into this feature rather than requiring an unavailable service or creating a competing queue.

Preserve today's daemon-to-llama Unix-socket transport. The daemon alone exposes TCP; the llama binary owns inference residency and private UDS control, with all inference mechanics remaining in agentix-infer/agentix-llama libraries. The unused `agentix-daemon/src/gateway/infer_handler.rs` is not the current request path.

## Technical Context

**Language/Version**: Rust 2021 for agentix; fenix toolchain from the flake. The pinned dev shell's Rust 1.94.1 is verified for the selected edition-2024 upstream libraries and extracted patch core; older compilers are untested.
**Primary Dependencies**: existing tokio, axum 0.8, reqwest 0.12, ratatui 0.29, crossterm 0.28, serde, sqlx, rmcp and llama-cpp-2; exact resolution in Cargo.lock during implementation. Upstream PTY/file-search crates at commit `092d3acd6bec3e3a14bdc7e7a2810ab628ab759d` are the initial library candidates.
**Storage**: versioned append-only session JSONL under `~/.ax/sessions`, atomic metadata snapshots and exclusive writer lock; daemon model blobs/manifests in existing Ollama-compatible layout; PostgreSQL 17 for relational orchestration. No SQLite.
**Testing**: Rust unit/contract tests, deterministic backend fixtures, actual small Nix-pinned GGUF, real PostgreSQL, bubblewrap integration, scripted terminal/headless acceptance.
**Target Platform**: Linux/NixOS first; documented Nix-on-Linux alternative.
**Performance Goals**: SC-003/004; bounded two-worker reference scenario SC-007. No claim about model tokens/second.
**Constraints**: no C++ in agentix-harness/router/infer through new dependencies; no credentials in agent jails; no network listener in reusable libraries; user-selected model/settings inheritance; no automatic Git publication.
**Scale/Scope**: one operator, up to four configured local workers (default two when enabled), one active turn per session and one valid lease per task. Cross-host worker execution is deferred.

## Constitution Check

| Principle | Pre-design obligation | Post-design disposition |
|---|---|---|
| I Library-first | Preserve crate layering | Mechanics in harness/infer/orchestrator; HTTP wiring only in daemon; private worker UDS stays in existing binary. New orchestrator crate has the distinct durable queue purpose established in 019. |
| II Local-first | No implicit cloud route | Local-only model identifiers; typed EscalationRequest via router is the sole cloud path for every client. Legacy cloud routes reject before provider I/O; API/daemon 0.2.0 documents the break. Default ax cloud disabled. |
| III Reproducible | Nix and lockfile | Pin upstream Git revision and Cargo.lock; update source filters and actual MSRV documentation after compatibility gate. No floating main or registry package assumed official. |
| IV Isolation | Parent/children jailed, no secrets | Launcher-controlled mounts, sanitized environment, child supervisors and restricted Git config; existing shared-network exception requires explicit per-profile rationale and security review. |
| V Layered contracts | TUI uses contract types only | agentix-api owns wire types; ax uses harness/client adapters, never daemon/router. Version management API and orchestration MCP explicitly. |
| VI Testing | Real database and small model | Mandatory PostgreSQL, actual fixture inference and real bwrap tests. Mock inference is only deterministic protocol coverage, never replacement for model tests. |
| VII Formal state | No flat ad-hoc loop | Exhaustive checked transition functions for turns, model operations, queue tasks and leases; illegal transitions return typed errors. |
| VIII Quality | All required gates | fmt, clippy, workspace tests, every Nix deliverable and green CI before merge. Planning stage does not claim they ran. |

Design is intended to satisfy all eight principles; implementation feasibility, security sign-off and runtime gates are outstanding. No constitutional exemption is requested. Historical architecture documentation needs correction to describe the existing UDS binaries accurately.

## Phase 0 — Research and dependency selection

[research.md](research.md) records baseline source evidence, feature 008/015/019 reuse, upstream provenance, and alternatives. First implementation gate: build a minimal integration of the upstream candidates with the Nix toolchain, inspect transitive dependencies and preserved tests, and record the actual result in research.md. Adopt PTY and file-search libraries if they meet the gates. Assess apply-patch and execpolicy before reimplementing either; do not import codex-core wholesale just to access a small primitive.

Use Cargo Git dependencies pinned to the release commit, including any necessary workspace patch overrides at the agentix root. Nix must pin/fetch the same source and include Git dependency hashes in crane dependency artifacts. Do not assume upstream workspace patches propagate through a Git dependency. If adaptation/extraction is necessary, record the exact upstream file, hash, notices and local diff and seek direction after a failed approach, per working agreements.

## Phase 1 — Design

### Ownership and dependency flow

- **agentix-api**: local catalog/model-operation and orchestration DTOs, streaming events, version negotiation and typed escalation payload. Existing local OpenAI-compatible shapes remain available; legacy direct-cloud routing is removed in API/daemon 0.2.0 with migration guidance.
- **agentix-harness**: session/turn state, context assembly, budgets, approvals, validated tool dispatch, gateway/MCP transport adapters and upstream primitive adapters. No daemon/router dependency. Platform tool implementations may remain in ax, invoked through harness traits.
- **agentix-ax**: interactive/headless presentation, config and command parsing, local session persistence adapter and file/command adapters. A shared controller drives both frontends.
- **agentix-infer**: pure-Rust model identity/store validation, download/operation state primitives, residency interfaces and capability metadata. Extend the store without pulling C++ into its dependency tree.
- **agentix-llama**: tool-template rendering/parsing, cancellation and context checks in the library; private UDS handlers for residency/load/tokenization in main.rs. Blocking FFI remains on dedicated workers/spawn_blocking.
- **agentix-daemon**: credential-bearing Hub HTTP adapter/download orchestration, public catalog and operation endpoints, typed cloud routing, orchestration HTTP adapter and configuration. It calls model-store library code and controls the llama worker over UDS. The model directory must be explicitly shared between their service identities.
- **agentix-orchestrator** (new, from 019 design): PostgreSQL queue, graph validation, checked transitions, fenced leases, resource reservations and execution history; no listener. Daemon holds database credentials and assembles it.
- **agentix-orchestrator-mcp** (new, adapted from 019): thin MCP stdio and `orchestrator-ctl` clients of the daemon contracts. No credentials in jailed clients. Session-scoped UDS relays supply host-held role capabilities. No additional TCP port.
- **agentix-jails**: trusted host launcher/supervisor and per-worker bubblewrap setup. Only this host-side code can create host sibling worktrees or launch workers; no generic host-shell endpoint is exposed to models. The same launcher binary owns private session UDS relays and authenticated daemon control, with keys/state outside all mounts.

### Coding loop and upstream adapters

Follow-up regression: configure MiniJinja with Hugging Face's `trim_blocks` and
`lstrip_blocks` settings. Preserve whitespace inside message/argument values;
test the exact ChatML generation boundary and replay of a completed tool turn
before a second user question. Keep malformed tool-batch rejection strict.

Typed `SessionController` accepts user turns and user control commands and emits ordered events. The harness owns the formal machine, not the TUI. An upstream process adapter supplies spawn/I/O/signal primitives; agentix still owns authorization, timeout, output bounds and process-group lifetime. An upstream file-search adapter supplies candidate matching with unchanged symlink-following behavior explicitly accepted by the user. Its starting root is not a containment boundary; jail mounts control accessibility. Read/edit path policy remains separate. Patch parsing/application and execution-policy reuse must cross the same compatibility gate, with agentix approval applied before every side effect.

Tool calls are accumulated and validated only after completion. IDs must be unique per turn; every advertised tool call receives success/error/denied/cancelled results before another generation. Budget exhaustion resolves unexecuted calls explicitly, then permits at most one bounded tool-free finalization. Stagnation enters an intervention state. Plan mode omits mutating tools and shell execution; pure file/search/diff tools remain. Shell commands are not classified as read-only by a string heuristic.

### Model selection and Hub lifecycle

2026-10-12 checkpoint extension: support Qwen3-Coder-30B-A3B-Instruct's native
function/parameter tool syntax alongside Qwen JSON. Select the parser from a
rendered historical-call probe, use advertised parameter schemas for typed values,
preserve string whitespace, and reject incomplete/duplicate/unknown calls before
dispatch. Pin the upstream template as a regression fixture. The change lives in
agentix-llama and requires a worker rebuild/deployment; ax's wire protocol stays
unchanged. Validate rendering, historical replay, typed parsing and repair prompts,
then document Q4_K_M setup and actual live validation status.
Add a concise built-in ax coding prompt covering Git status/staged/unstaged review,
read-before-edit and evidence-backed validation; full instruction/skill discovery
remains T033. Verify the system message survives follow-ups without duplication.

A model record has a stable opaque ID, aliases, blob digest, exact source revision, resident state and validated capability profile. Unknown capability is not coding-ready. Initially support one documented Qwen-style tool-template family with regression fixtures; identify support by validated metadata/template behavior rather than model name alone. Other templates remain inspectable/installable but unavailable in coding mode until an adapter exists.

Daemon catalog/search adapters resolve Hub repositories and file metadata. The user chooses an exact single-file GGUF at an immutable commit. Catalog tags are hints, not proof of compatibility. Downloads write private partials, verify expected size/SHA-256, and atomically publish blob and manifest under per-artifact locks. No arbitrary URL or server-local path is accepted by new public endpoints. Redirects are HTTPS-only, bounded and validated against configured Hub/CDN origins; do not forward authorization across origins.

A job separates `download`, `verify`, `install`, `warm` and `ready`. Cancellation does not undo an installed valid blob but prevents session activation. The llama worker deduplicates loads, reserves capacity and cannot evict leased/in-use models. Sessions store the selected ID, not a permanent VRAM reservation; each turn reacquires readiness. A failed candidate load keeps the previous selection, though that model may need rewarming. Missing expected integrity metadata yields an explicit unsupported-verification error rather than an unchecked installation.

### Sessions, instructions, skills and MCP

Use a versioned append-only event journal plus atomic session metadata, with fsync at tool-start/result and turn-terminal boundaries. Recovery truncates only an incomplete trailing record after preserving a recovery copy; other corruption requires user-directed recovery. One writer lock per session; fork creates a new ID and immutable parent reference. No tool invocation is replayed automatically.

Instruction order: built-in policies, global ax AGENTS.md, repository ancestors from root to cwd, nested instructions for touched paths, then user turn. Applicable user instructions may override project guidance but cannot change sandbox grants. Show source path/hash and reload changes at turn boundaries. Skills are discovered in configured read-only global roots and repository `.agents/skills`, with explicit `$name` activation and lazy SKILL.md loading; references stay bounded by granted roots. Tools and remote content are untrusted data.

Reuse upstream MCP client only if its dependency/protocol gate passes; otherwise use the existing workspace rmcp client facilities after investigation, not the current handwritten protocol as a new default. Configured server executables and env keys are approved at launch; names are namespaced. Cancellation shuts down owned servers and records unresolved side effects.

Context accounting uses the selected model's tokenizer through the private worker and daemon endpoint; reserve output/tool budgets. Compaction runs locally, preserves pinned instructions/decisions and complete exchanges, journals the source span, and fails safely if required content cannot fit. Model switches re-evaluate context limits.

### Orchestration and isolation

Feature 019 is planning-only at base; its required queue implementation is part of this task list. Retain its priorities, DAG submission, intervention and history concepts, adding run IDs, lease tokens/generations, cancellation and stable scopes. TUI communicates via agentix-api contracts. StdIO MCP and ax access queue operations through role-scoped host UDS relays forwarding to the daemon, not direct PostgreSQL. The identity/transport protocol is specified in contracts/orchestration.md: administrator-provisioned mTLS supervisor identity, daemon-minted host-held per-role capabilities, revocation/renewal, and no credentials inside parent or child jails. Every daemon control route authenticates the role, including direct HTTP attempts; trusted LAN is not an authorization bypass.

`ax-jail --workers N` authorizes a bounded local supervisor for this launch. It resolves the host repository root and freezes a clean HEAD by default; a dirty index/worktree or nonignored untracked file blocks delegation until the user commits or explicitly supplies `--base <full-commit-id>`. Explicit base selection excludes local changes without modifying them; ignored files are excluded and submodule repositories are unsupported for v1 delegation. It allocates persistent sibling worktrees with unique non-numbered names (`ax-<run>-<task>`), checks actual paths, and launches only the packaged ax worker command with allowlisted arguments and sanitized environment. The daemon cannot specify an arbitrary executable or cwd. A launch-bound scope restricts run/task/worktree assignments; wildcard worker scopes are not accepted from model arguments. Single-agent launches do not gain worker launch permissions implicitly.

Each editing child sees its own worktree, required Git object store with configuration/hook protections, its own session state and explicit read-only inputs. Children cannot spawn grandchildren unless another bounded grant explicitly permits it; v1 defaults to one delegation level. Parent model and quality configuration are captured at submission; explicit user overrides are stored separately. Daemon concurrency reservations prevent too many resident/generating models. Read-only research jobs may share read-only source mounts.

A task claim binds a random role capability plus monotonically increasing generation to the assignment. The capability stays in the host relay, which binds worker identity to its socket and supplies the credential on the authenticated daemon hop. Every heartbeat, result and intervention checks run/scope/worker/capability/generation and expiry transactionally. Timeout marks the assignment revoking; only after the host supervisor confirms old process-group termination may a replacement edit worker start. If the supervisor cannot confirm termination, block for operator action. A database fence alone cannot stop filesystem writes.

Successful results enter Revoking until the worker is stopped, then Reviewing with a sealed task-local delta and exact input/artifact hashes. Parent approval of that frozen artifact makes the task Completed and authorizes downstream consumption, not writes to the parent. The supervisor prepares each dependent worktree from the frozen run base plus the transitive predecessor deltas, once each in deterministic topological order. Exact preimage checks reject conflicts; only a verified input-manifest/tree hash enables model execution. A conflicting or missing input blocks preparation for explicit reviewed resolution. This also defines diamond graphs, retry input reuse and input-version invalidation in contracts/orchestration.md.

Integration into the parent's current worktree remains a separate explicit action after reviewing the sealed diff and test evidence. Apply selected changes through the ordinary approved patch/Git tool path with current-content checks; conflicts preserve both trees. No automatic push, merge, reset, deletion or commit to master. Artifact capture and materialization execute in restricted helper sandboxes; no worker-supplied command runs on the host.

Default parent exit cancels children; explicit detach leaves the trusted supervisor running and is recorded in the run. Reattach inspects actual worker state. Follow-up messages go to the named child at a turn boundary; settings changes affect future assignments, not in-flight turns.

### Permission and network boundary

Default workspace-write mode allows reads and approved patch edits within launch roots; commands and side-effectful MCP require a concrete approval unless the launch policy permits them. Headless policy `never` denies actions requiring approval. Session approval is scoped to tool/arguments/cwd and never grants mounts. No-shell and no-cloud remain hard bounds.

The launcher must remove automatic SSH_AUTH_SOCK mounting and unrestricted direnv variable forwarding. Keep only documented dev-shell/toolchain variables and sanitized Git identity, with host hooks, includes, helpers and worktree-local executable Git settings disabled or exposed read-only. No ~/.codex, ~/.claude, Hub tokens or database credentials are mounted.

Shared network remains a documented v1 exception for configured remote daemon access and development commands that need registries. It is not claimed to enforce daemon-only egress; review this explicitly in ARCHITECTURE.md. Model traffic and Hub credentials still go through the daemon. Model-management endpoints remain administrator-restricted to loopback/trusted-LAN deployments. Orchestration control additionally requires the mTLS/role-capability checks on every request, including loopback; its session relay sockets are narrow operation capabilities. This is not a general multi-tenant service. Provisioning, revocation, protected key/artifact paths and relay setup are mandatory Nix service/launcher configuration deliverables.

## Project Structure

Documentation: spec.md, plan.md, research.md, data-model.md, quickstart.md, checklists/requirements.md and contracts/{agent.md,models.md,orchestration.md}; tasks.md is the subsequent stage.

Planned source additions/changes:

```text
agentix-api/src/{lib.rs,agent.rs,models.rs,orchestration.rs}
agentix-harness/src/{agent.rs,client.rs,event.rs,session.rs,context.rs,approval.rs,upstream.rs}
agentix-ax/src/{main.rs,config.rs,session_store.rs,controller.rs,commands.rs,mcp.rs}
agentix-ax/src/tui/{mod.rs,ui.rs,model_picker.rs,agents.rs}
agentix-ax/src/tools/{file_ops.rs,shell.rs,patch.rs,search.rs}
agentix-infer/src/{lib.rs,engine.rs,pool.rs,model_operation.rs,store/hf.rs,store/blob.rs,store/manifest.rs}
agentix-llama/src/{lib.rs,main.rs,tool_template.rs}
agentix-daemon/src/gateway/{mod.rs,models.rs,orchestration.rs,escalation.rs}
agentix-daemon/src/{config.rs,hub.rs}
agentix-router/src/lib.rs
agentix-orchestrator/src/{lib.rs,db.rs,model.rs,queue.rs,graph.rs,lease.rs}
agentix-orchestrator-mcp/src/{lib.rs,main.rs,bin/orchestrator_ctl.rs}
agentix-jails/src/ax_jail/{main.rs,supervisor.rs,profile.rs,relay.rs,artifacts.rs}
scripts/schema.sql
perSystem/{packages.nix,devShells.nix,processes.nix}
```

The cloud compatibility change requires agentix-api and agentix-daemon 0.2.0 and coordinated dependent/lockfile updates. New paths describe intended modules; no claim they exist yet. Update actual NixOS module and CI files discovered in implementation. All affected package source filters must include their new transitive workspace dependencies.

## Implementation phases

1. Upstream compatibility/provenance gate and baseline tests.
2. Shared contracts, state machines and model backend prerequisites.
3. US1 coding loop and US6 sandbox baseline.
4. US2 existing-model lifecycle, then US3 Hub operations.
5. US4 durable context, skills, MCP and full command surface.
6. US5 queue foundation, supervisor, UI and integration/recovery.
7. Full-system acceptance, packages, documentation and quality gates.

Tests are required despite the generic tasks template calling them optional. Feature 008's state-machine and 015's tool-calling code are references, not blind cherry-pick dependencies. Extend their ideas for cancellation, approval, typed streaming and strict errors. The original planning stage ran no runtime checks; implementation evidence is now recorded in quickstart.md.

## Complexity Tracking

No accepted constitutional violations. The new orchestrator library and stdio/CLI crate replace the same roles planned in 019; a second queue backend is rejected. A full Codex runtime embed is not selected because its coupled state/runtime/policy stack needs a much broader compatibility and storage redesign than leaf-library reuse.

### Terminal UX checkpoint (2026-10-12)

Keep the current Ratatui/event architecture. Use default terminal foreground and
background, bold user prompts, dim metadata and limited warning/error accents.
Place model/approval mode and project path below a lightly separated composer.
Shorten only displayed model identifiers; retain exact IDs in the picker. Wrap
transcript words with Unicode display widths. Keep bounded tool details behind
Ctrl+T, without hiding approvals or failures. Compare the final streamed response
with completion text before appending a fallback answer. Validate event reduction,
TestBackend layouts and the existing real-PTY resize/approval/follow-up tests.

Syntax refinement: use syntect with its pure-Rust fancy-regex feature set for
fenced code; no native regex dependency. Render a small Markdown presentation
subset and preserve original transcript data. Cache styled transcript rows by
log revision, width and detail visibility so spinner ticks do not repeat syntax
parsing. Syntax resources initialize once. Unknown languages fall back to text.

Extend this refinement with bounded in-memory input history, a labeled bordered
composer and assistant-group bullets. Preserve SSE error payloads as bounded anyhow
causes in the harness; the TUI displays the cause chain only in expanded details.

Store bounded versioned model preferences per normalized gateway using atomic
same-directory replacement and private file permissions. Load/save via spawn_blocking
in the TUI; reuse the existing ~/.ax jail mount. Save only after picker readiness or
successful completion, never on a failed selection. Combine footer spans and allocate
width to both model and path; keep approval mode visible when space permits.
