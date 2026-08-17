# agentix Architecture

> Invariants and non-negotiable rules live in `.specify/memory/constitution.md`.
> This document records how those invariants are realized today, plus the decisions
> behind them. Each section opens with the principle it realizes.

---

## Crate Inventory

*Realizes Principles I and V.*

### Current

```
agentix/                        (workspace root — agentix package)
├── src/jail/                   claude-jail binary (bubblewrap sandbox for claude-code)
├── src/ax_jail/                ax-jail binary (bubblewrap sandbox for agentix-ax)
├── src/gh_proxy/               gh-jail-client + gh-jail-server binaries
├── agentix-api/                OpenAI-compatible request/response types (no deps)
├── agentix-router/             backend-selection routing (RouteTarget enum)
│                               depends on: agentix-api
│                               INVARIANT: MUST NOT depend on any C++ crate
│                               (verified: cargo metadata shows zero C++ transitive deps)
├── agentix-infer/              pure-Rust inference traits and types; no C++ deps
│                               features: whisper (stub, reserved)
│                               depends on: (none — pure Rust library crate)
├── agentix-llama/              llama-cpp-2 GGUF inference backend (C++)
│                               features: cuda
│                               depends on: agentix-infer, llama-cpp-2
├── agentix-daemon/             Axum HTTP server; routes to agentix-llama / cloud backends
│                               depends on: agentix-api, agentix-router, agentix-infer, agentix-llama
├── agentix-search/             PostgreSQL search library (BM25 + vector + reranking)
│                               depends on: sqlx, fastembed, reqwest (no C++ inference)
├── agentix-indexer/            repo ingestion pipeline (tree-sitter + embed)
│                               depends on: agentix-search, tree-sitter-*
├── agentix-mcp-server/         MCP stdio server exposing search tools
│                               depends on: agentix-search, agentix-indexer, rmcp
├── agentix-harness/            agent loop library (no daemon dependency)
└── agentix-ax/                 TUI agent; runs inside bubblewrap jail
```

The workspace is decomposed so that changes to search, indexing, MCP server, or
routing logic never trigger C++ recompilation of the inference backend. The key
isolation boundary is `agentix-infer` (pure Rust traits) vs `agentix-llama`
(llama-cpp-2, C++). Crates that don't need inference depend only on `agentix-infer`;
only `agentix-daemon` and `agentix-llama` itself pull in the C++ build.

### Dependency rules (from constitution)

```
agentix-daemon      →  agentix-api, agentix-router, agentix-infer, agentix-llama
agentix-ax          →  agentix-api, agentix-harness
agentix-router      →  agentix-api                    (NO C++ deps — invariant)
agentix-llama       →  agentix-infer, llama-cpp-2
agentix-mcp-server  →  agentix-search, agentix-indexer
agentix-indexer     →  agentix-search
```

Library crates (`agentix-api`, `agentix-router`, `agentix-harness`, `agentix-infer`,
`agentix-search`) MUST NOT depend on `agentix-daemon`. No circular dependencies.

**C++ isolation invariant**: `agentix-router` MUST NOT depend on any C++ crate.
Verified by inspecting `cargo metadata` — zero C++ transitive deps on that path.

---

## HTTP Gateway (agentix-daemon)

*Realizes Principle V.*

Axum-based server. Exposed routes:

| Route | Method | Purpose |
|-------|--------|---------|
| `/health` | GET | liveness probe |
| `/v1/models` | GET | list local + cloud models |
| `/v1/chat/completions` | POST | OpenAI-compat chat (routed) |
| `/v1/embeddings` | POST | embeddings (local Ollama only, today) |
| `/v1/messages` | POST | Anthropic-native chat (for SDK clients) |

All credentials (API keys) live in the daemon's config. No key is exposed to
jailed agents. The jail calls the daemon over HTTP and the daemon holds the
trust boundary.

---

## Routing Policy

*Realizes Principle V.*

**Current implementation**: model-name-prefix matching, hardcoded in
`agentix-daemon/src/inference/mod.rs` (`InferenceEngine::route`).

```
local/<anything>          → Local (Ollama)
<prefix>/<model>          → OpenRouter  (provider/model syntax)
claude*                   → Anthropic
gpt*, o1*, o3*, o4*       → OpenAI
qwen*, mistral*, llama*,  → Local (Ollama)
  laguna*, gemma*, phi*,
  deepseek*, kimi*, ...
<unrecognized>            → config.default_route (runtime fallback)
```

**Decision — code not data**: routing rules are compiled in for now. A policy
config schema was considered but deferred: the prefix rules are stable, the
set of model families changes infrequently, and a config schema adds a parsing
and validation surface for limited current benefit. Revisit when per-user or
per-request policy is needed.

**Target**: when `agentix-router` is extracted, this logic moves there. The
typed `EscalationRequest` (required by Principle II) will be defined in
`agentix-api` and consumed by `agentix-router`.

---

## Agent Loop

*Realizes Principles II and VII.*

Implemented in `agentix-harness/src/agent.rs` as `AgentLoop::run_impl`.

### Current shape

The loop is currently a flat `loop {}` with ad-hoc checks, not a formal typed
state machine. The implicit states are:

| Implicit state | Trigger |
|----------------|---------|
| Budget check | top of each iteration |
| Model call | after budget check passes |
| Tool dispatch | model returned tool calls |
| Stagnation check | after all tool results pushed |
| Intervention inject | stagnation detected |
| Final answer | model returned no tool calls |
| Budget exhausted | budget hit mid-dispatch |

**Gap**: this does not satisfy Principle VII. Invalid transitions are not
structurally rejected — the ordering is enforced only by code flow.

### Target shape

Refactor to a typed state enum with explicit transitions. Stagnation detection,
budget enforcement, and escalation MUST be modeled as transitions, not inline
conditionals. The specific enum variants are a design decision for the
refactor spec; the requirement for a formal machine is constitutionally mandated.

### Stagnation detection (`agentix-harness/src/stagnation.rs`)

Sliding window over tool result hashes (default: window=4, min\_matches=3).
Content-addressed: same result text → same hash → stagnation detected.
Stateless across runs; reset on each `AgentLoop::run` call.

### Escalation policy (`agentix-harness/src/policy.rs`)

`EscalationPolicy` holds `max_tool_calls`, `stagnation_window`,
`stagnation_min_matches`. Defaults: 20 tool calls, window 4, threshold 3.

---

## Jail Profiles

*Realizes Principle IV.*

Each jailed agent gets exactly:

| Resource | What | Notes |
|----------|------|-------|
| Network | Full network access | Required: agent calls daemon over HTTP |
| Tools | Predefined fixed set | Bound at jail construction; not extensible at runtime |
| `/nix/store` | Read-only bind mount | Agent can use any store path; cannot write |
| Nix daemon | Proxy socket (`nix-container-daemon`) | Store queries + builds allowed; GC operations blocked |
| Home | tmpfs with selective bind mounts | `.gitconfig`, etc. as needed |
| `.claude` | Bind-mounted from host | **Transitional** — see note below |

**Transitional exception — `.claude` mount**: `~/.claude` (containing OAuth
credentials) is currently bind-mounted because `claude-code` is the jailed
agent and requires it for authentication. This violates Principle IV's
"no secrets in jails" rule and is accepted only until `agentix-ax` replaces
`claude-code` as the jailed agent. At that point the mount is removed, the
daemon becomes the sole credential holder, and this exception is closed. Exit
criterion: `agentix-ax` running as the jailed agent end-to-end.

**The network exception** (Principle IV): open network access is required by
`claude-code`, which needs real HTTPS to reach Anthropic's API — there is no
way to jail it without it. This is the constraint that makes the full netns
unshare impractical today. The current security posture is therefore weaker
than the target: a live OAuth token sits in a jail with open egress.

Post-migration (once `agentix-ax` is the only jailed agent), the picture
changes: no credentials in the jail, all provider tokens daemon-side. At that
point a Unix socket + netns unshare becomes viable — `agentix-ax` is our code
and we control its transport. Whether to take that option is a separate
decision, but the migration is what unlocks it. The residual risk after
migration is prompt-injection exfiltration of data (repo contents, search
results), not credentials — a deliberate, documented trade-off rather than an
unstated one.

**Nix daemon proxy**: `nix-container-daemon` speaks the Nix worker protocol and
proxies to the host daemon. GC operations are blocked so a jailed agent cannot
collect store paths the host needs. Container/agent profiles are registered as
GC roots on the host. This is what makes `nix develop` work inside the jail
without trust-escalating the agent.

---

## Search and Fusion

*No direct constitution principle — supports Principle VI (testability).*

**Storage**: PostgreSQL 17 with `pg_search` (ParadeDB BM25) and `pgvector`
(HNSW index). One service, one backup story.

**Fusion**: Reciprocal Rank Fusion (RRF) computed in SQL by the `hybrid_search`
PostgreSQL function. The Rust layer issues one query and receives pre-ranked
results with an `rrf_score` column. No application-side merging.

**Decision — SQL fusion over application-side**: RRF in a SQL function keeps
the Rust code simple (one query, typed result rows) and makes the ranking
formula visible and testable with plain SQL. The alternative — fetch BM25 and
vector results separately and merge in Rust — was rejected because it requires
two round trips and duplicates logic that the database can express more cleanly.

**Embedding path**: ingest generates embeddings via Ollama during indexing.
Chat inference and embedding inference are separate paths today (embedding
always goes to Ollama; chat goes through the router). This is a known
inconsistency to revisit when `agentix-infer` lands.

---

## Decisions Log

| Decision | Chosen | Rejected | Reason |
|----------|--------|----------|--------|
| Search fusion | RRF in SQL (`hybrid_search` fn) | Application-side merge | One round trip; ranking logic in SQL is auditable |
| Routing policy representation | Compiled-in prefix rules | Config schema | Stable rules; config adds parsing surface for no current benefit |
| Jail ↔ daemon channel | HTTP over network | Unix socket + netns unshare | `claude-code` requires real HTTPS to Anthropic; open network is the constraint it imposes. Post-migration to `agentix-ax`, UDS + netns unshare becomes viable and should be reconsidered — it eliminates the open-egress exfiltration surface entirely |
| Nix store in jail | Read-only bind + daemon proxy | Full writable store | Proxy blocks GC; agent can build without corrupting host store |
| Model storage layout | Ollama-compatible content-addressed blobs | Custom layout | Existing Ollama models usable without re-download |
| C++ isolation — infer split | `agentix-infer` (pure Rust traits) + `agentix-llama` (C++) | Single crate with feature flags | Feature-gated crate still pulls C++ into the dep graph for all consumers; separate crates mean search/indexer/router never trigger llama-cpp-2 recompilation |
| C++ isolation — search/indexer | tree-sitter kept in `agentix-indexer`; fastembed kept in `agentix-search` | Move all C++ to agentix-llama | tree-sitter and fastembed are bounded C++ builds; removing them would require rewriting parsers and reranking in pure Rust for no practical gain |
