# agentix Constitution

## Core Principles

### I. Library-First Architecture
Inference logic, agent-loop mechanics, routing policy, and API types MUST live in standalone
Rust library crates with no network surface of their own. The daemon assembles these libraries
and binds the inference API port. The dedicated `agentix-store-server` binary may bind
the shared-index API with optional bearer authentication for remote MCP clients; its library never binds a listener.
Libraries MUST be independently testable with
`cargo test` and carry no compile-time dependency on daemon internals. New crates require a
clear, non-overlapping purpose before being added to the workspace.

### II. Local-First Intelligence
The primary inference path MUST always route through a locally-hosted model. Cloud models
(Anthropic, OpenAI, OpenRouter) MUST be treated as a fallback tool, not a default path.
Escalation from local to cloud MUST be structural: it MUST flow through the routing library
via a typed escalation request type carrying at minimum what the model already knows, a
precise question, and why local sources are insufficient. A cloud call that bypasses this
type MUST NOT compile. Lazy offloading to cloud is a defect, not a feature.

### III. Reproducible Environments
Nix flakes are the single source of truth for the build environment, service orchestration,
and toolchain versions. All model services (Ollama) MUST be launchable via
`nix run .#dev` or the NixOS module. `just` recipes MUST assume the Nix dev shell. No
dependency on host-installed tools beyond Nix itself. On non-NixOS hosts, system-service
alternatives MUST be documented in `README.md`. Dependencies in `Cargo.toml` are pinned via
`Cargo.lock`; lock file MUST be committed and kept up to date.

### IV. Isolation by Default
Every AI agent execution MUST run inside a bubblewrap sandbox (`claude-jail`, `ax-jail`).
Sandboxes MUST unshare all namespaces except network, mount `/nix/store` read-only, and
expose only the minimum required tooling. Credentials, API keys, and secrets MUST NOT be
bind-mounted into jails. Exception: subscription-based model wrappers that authenticate
via OAuth with no API-key alternative (e.g. `claude-code`) MAY mount the credential
directory read-write; this exception MUST be documented per jail profile in
`ARCHITECTURE.md`. Expanding a jail's permissions requires explicit justification in
code review.

The network exception MUST be constrained: where possible, jailed agents MUST communicate
exclusively with the daemon (which holds all credentials and enforces policy). Unrestricted
external egress from within a jail MUST be explicitly justified per jail profile in
`ARCHITECTURE.md` and reviewed as a security decision. The intent is that the daemon is
the sole trust boundary; eliminating credentials from the jail filesystem is the goal for
new jail profiles (`ax-jail` achieves this), not a universal invariant where the binary
being jailed makes it structurally impossible.

### V. Layered API and Routing
The API contract (OpenAI-compatible types, request/response schemas) MUST live in a dedicated
library crate, separate from the HTTP server. Backend-selection policy (local inference,
OpenRouter, Anthropic, other frontier providers) MUST live in a dedicated routing library
crate — not inside the daemon. The daemon is responsible for inference HTTP serving; it wires
the API and routing libraries together and binds the port. The TUI MUST communicate via API
contract types only and MUST NOT depend on the daemon or routing library directly. Breaking
changes to the API contract types require a documented version bump. The MCP server MUST
expose tools using MCP spec-compliant transports only — no custom protocols.

### VI. Comprehensive Testing
- **Unit tests** cover each crate's logic independently (`cargo test -p <crate>`)
- **Integration tests** run against actual local LanceDB storage — no mock databases for
  integration coverage. Inference integration tests MUST use a small, quantized fixture
  model pinned in the Nix flake; they MUST NOT depend on whatever model happens to be on
  disk, and MUST NOT require a multi-gigabyte download to run the test suite.
- **Async runtime**: embedding and inference calls MUST NOT block the async runtime; offload
  to a blocking thread pool. This is an architectural invariant, not a per-feature SLO.
- Latency budgets (e.g. p95 thresholds under specific load) are per-feature spec material,
  not constitution-level rules.
- Tests MUST run locally before pushing. CI validates but does not substitute for local
  verification.

### VII. Formal Agent State Machine
The agent loop MUST be implemented as an explicit state machine with a typed state enum.
Invalid transitions MUST be structurally rejected — either at compile time via Rust's
typestate pattern, or at runtime via exhaustive `match` that returns `Err` for illegal
transitions. No implicit fallthrough between states. Stagnation detection, budget
enforcement, and escalation policy MUST all be modeled as state transitions, not ad-hoc
checks scattered through the loop. The specific states and transitions are an architecture
decision; the requirement for a formal machine is not.

### VIII. Code Quality Gates
All code MUST pass before merge:
1. `cargo fmt --check` — formatting enforced by rustfmt
2. `cargo clippy -- -D warnings` — no clippy warnings in CI; `clippy::unwrap_used` and
   `clippy::expect_used` MUST be enabled in `[workspace.lints]` so gate 2 mechanically
   enforces the no-panic-on-production-paths rule; `#[allow(...)]` with a safety comment
   is the only escape hatch
3. `cargo test --workspace` — all tests green
4. No `unsafe` blocks without a `// SAFETY:` comment explaining the invariant
5. Every deliverable MUST have a corresponding Nix package attribute; `nix build .#<pkg>`
   MUST succeed before merge — the Nix build is the canonical reproducibility check
6. CI MUST be green — never merge with failing checks

## Architecture

### Workspace Layout
The Cargo workspace follows strict layering — dependencies flow downward only. Circular
crate dependencies are forbidden. Library crates MUST NOT depend on the daemon. The TUI
MUST NOT depend on the daemon or the routing library. The daemon is the only crate that
assembles the full stack.

For the current crate inventory and dependency graph, see `ARCHITECTURE.md`.

### Storage Conventions
- **Search index**: LanceDB with FTS and unquantized cosine HNSW, owned by the shared agentix-store library. A storage server owns its local directory and exposes an API with optional bearer authentication to laptop MCP clients; local administrative use remains supported. Schema/model/dimension metadata MUST be validated on reuse and remote connection; migration from legacy PostgreSQL is explicit and read-only at source.
- **Model blobs**: content-addressed storage under `$AGENTIX_MODELS_DIR` in Ollama-compatible
  layout; existing Ollama model directories are usable without re-download

### MCP Protocol
The MCP server exposes tools using MCP spec-compliant transports. Tool schemas MUST be
stable across patch releases. Additive changes (new tools, new optional fields) are
non-breaking. Removing or renaming tools is a breaking change requiring a version bump.

## Development Workflow

### Commit Discipline
Conventional Commits format. Each commit addresses a single concern. `feat:` for new
capabilities, `fix:` for bug corrections, `feat!:` / `fix!:` for breaking changes.
Commits MUST build and pass tests at every point in history (`cargo test --workspace`).

### Code Review Culture
Reviews assume competence. Feedback MUST be specific and actionable. Security-sensitive
changes (sandbox permissions, API key handling, jail configuration) require heightened
scrutiny and explicit sign-off. Nitpicks MUST be marked as such.

### Dependency Management
All version constraints live in workspace `Cargo.toml` under `[workspace.dependencies]`
where practical; per-crate overrides require justification. Dependency bumps are deliberate
— each bump is a conscious decision with changelog review. Avoid adding dependencies that
duplicate functionality already available in the workspace or standard library. (`Cargo.lock`
discipline is covered by Principle III.)

## Governance

This constitution defines the quality and design principles for all spec-driven work on
agentix. It supersedes any prior project guidelines. All specs, plans, and implementations
MUST demonstrate compliance with these principles. Complexity that violates a principle
MUST be justified in the plan's Complexity Tracking table before implementation begins.

**Amendment procedure**: propose a diff to this file in a PR, state the version bump type
and rationale, propagate to dependent templates. Amendments require at least one review
before merge.

**Versioning rules** (semantic, applied to this constitution):
- **MAJOR**: a principle is removed, renamed, or its requirement weakened
- **MINOR**: a new principle is added, or existing guidance materially expanded
- **PATCH**: wording clarified, typos fixed, non-semantic refinements

**Version**: 1.1.0 | **Ratified**: 2026-08-10 | **Last Amended**: 2026-10-28
