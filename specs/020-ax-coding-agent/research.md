# Research: baseline and reuse decisions

Date: 2026-10-11. Agentix base `68d7770`. Upstream Codex comparison: installed CLI 0.162.1 and tag `rust-v0.162.1`, dereferenced commit `092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`. These are planning observations, not compiled integration results.

## Current agentix evidence

Agentic-nix hybrid search over `/home/sam/home/agentix/master` found the relevant code; retrieved evidence was verified against current files because the index predates HEAD.

| Area | Current evidence | Consequence |
|---|---|---|
| TUI | `agentix-ax/src/tui/mod.rs::submit_task` creates a fresh AgentLoop; input disabled while running; Ctrl-C exits | Persistent controller, streaming, cancellation and user command handling are actual work |
| Harness | `agentix-harness/src/agent.rs::run_impl` starts with one user message and a flat loop | Session input and formal checked transitions are needed; 008 is not merged |
| Wire client | `agentix-harness/src/client.rs::chat` buffers JSON; malformed arguments become `{}` | Typed stream parser must reject incomplete/malformed calls |
| Tools | file_ops.rs overwrites files directly; shell.rs waits up to 60 seconds and does not explicitly manage a process group | Conflict-aware patches, path policy, output bounds and cancellation needed |
| Gateway | `agentix-daemon/src/gateway/mod.rs` forwards to llama UDS; infer_handler.rs is not registered | New work targets actual worker main.rs and daemon proxy boundaries |
| Backend | `agentix-llama/src/lib.rs::apply_chat_template` accepts messages but no tools | Cannot get reliable structured tools merely by sending a tools array from ax |
| Store | `agentix-infer/src/store/hf.rs` resolves files and downloads synchronously via hf-hub; engine wraps pull in spawn_blocking | Existing blob reuse is useful; search, explicit revision selection, progress/cancellation and strict verification need extensions |
| Pool | engine/pool implement acquisition/load coordination | Expose supported residency/readiness and cancellation interfaces instead of a TUI-side shadow model list |
| Jail | ax_jail/main.rs shares network, forwards arbitrary direnv vars and auto-mounts SSH agent | Reuse worktree/Nix mounting concepts, fix secret and grant boundaries before worker orchestration |
| Build | `perSystem/packages.nix` has per-package source filters; ax includes ax/harness only | API/upstream/orchestrator adoption must update filters and dependency artifacts |

No model inference, live server mutation, remote publication or code implementation was performed during this research. Review remediation chooses daemon-wide structured cloud escalation with an API/daemon 0.2.0 migration, host-held mTLS/role capabilities with per-jail UDS relays, sealed reviewed predecessor artifacts, and a clean-HEAD/explicit-commit delegation policy. These are design decisions to validate during implementation, not tested security claims.

## Existing feature work

- 008 branch's `specs/006-harness-statemachine` describes an enum refactor but does not cover persistent sessions, approval and cancellation. Reuse the intent and reviewed source patterns; do not retain skipped tool-result behavior at budget exhaustion.
- 015's tool-calling plan introduces template rendering and XML tool-call parsing. Its initial streaming limitation and optional ignored fixture tests are insufficient here. Require strict typed tool streams and mandatory real-model smoke coverage.
- 019 at `c96657c` contains specification/plan/tasks for PostgreSQL queue and MCP/CLI, but not implemented workspace crates. Retain atomic claims, DAG priorities, intervention/history; add fenced leases, cancellation, worktree ownership and user-facing ax integration. Replace direct-DB jailed adapters with daemon-backed ones. Do not import its `todo!()` stubs, same-file parallelism claims or optional database-test skips.

## Codex workflow reference

The locally installed `codex --help` establishes exec, resume/fork, review, MCP, sandbox and model controls as useful comparison points. The official [CLI documentation](https://learn.chatgpt.com/docs/codex/cli) describes repository coding, saved sessions and tool integrations. The official [command documentation](https://learn.chatgpt.com/docs/developer-commands?surface=cli) describes `/model`, `/status` and `/plan`. This feature uses those workflows as product references, not a promise of full command/API compatibility.

## Upstream library reuse

User preference: reuse upstream code as libraries wherever practical. This is an explicit first-phase gate, not a later optimization.

Source pin: [OpenAI Codex release tree](https://github.com/openai/codex/tree/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d/codex-rs). The [workspace manifest](https://github.com/openai/codex/blob/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d/codex-rs/Cargo.toml) declares edition 2024 and Apache-2.0. Preserve upstream license/notices and audit dependency licenses during adoption. No legal opinion or compiled compatibility claim is made.

| Candidate | Observed boundary | Decision |
|---|---|---|
| codex-utils-pty | Public process/PTY spawn, handle, signal and terminal-size exports; depends on tokio/portable-pty with platform-specific code | First library candidate for bounded command I/O; agentix still enforces approvals and process-group lifecycle |
| codex-file-search | Public create_session/run, options/results and cancellation flag; ignore/nucleo-based dependencies | First library candidate for filename matching; content search remains bounded rg/tool functionality |
| codex-apply-patch | Public lib, but manifest now pulls codex-exec-server and path utilities | Assess dependency closure and parser/application separation before choosing full dependency or narrowly adapted source |
| codex-execpolicy | Library with Starlark command rules and path utilities | Assess for command-policy parsing; cannot replace bubblewrap or user grants |
| codex-ansi-escape | Ratatui-facing ANSI conversion | Evaluate ratatui version compatibility before adoption; security handling still strips unsafe terminal sequences |
| codex-utils-output-truncation | Pulls codex-protocol | Do not treat as an isolated helper without dependency audit |
| codex-protocol (upstream) | Pulls protocol, policy, network and path-related dependencies | Keep agentix wire DTOs authoritative; optional adapter only if a concrete compatibility consumer needs it |
| codex-core / TUI / app-server | Broad runtime, auth, state, providers and tools integration; core depends on codex-state, whose manifest includes SQLite | Not selected wholesale: conflicts with no-SQLite preference and duplicates daemon/state ownership. Revisit only as an explicit architecture change |

Direct source inspection verified public entry points in `utils/pty/src/lib.rs` and `file-search/src/lib.rs`, and dependency declarations in the above Cargo.toml files. Internal APIs, exact transitive versions, upstream `[patch]` requirements, toolchain compatibility and lifecycle behavior remain to be exercised by T001–T003. Do not claim the adapter is proven until those gates pass.

### crates.io provenance

The crate named [codex 0.3.0](https://docs.rs/codex/0.3.0/codex/) is Typst's Unicode-symbol library, unrelated to the coding agent.

[codex-protocol 0.63.0](https://docs.rs/crate/codex-protocol/0.63.0/source/Cargo.toml) is a real published package. Its manifest identifies Namastex Labs and `https://github.com/namastexlabs/codex`, not the OpenAI repository. Agentic-nix `fetch_package` successfully indexed 225 chunks as `crates.io::codex-protocol-0.63.0`; a subsequent hybrid search retrieved approval event types. This establishes availability for research, not compatibility with 0.162.1. Prefer the single pinned upstream source revision for implementation candidates rather than mixing old third-party protocol types into current internals without comparison.

Direct crates.io API requests were blocked with HTTP 403. Publication metadata was verified through the crates.io Git index and versioned docs.rs manifests. No blanket claim is made that every OpenAI-named component is unpublished.

## Hub integration

Official [Hub search documentation](https://huggingface.co/docs/huggingface_hub/en/guides/search) supports filtered model discovery. [Download documentation](https://huggingface.co/docs/huggingface_hub/en/guides/download) distinguishes a file's repository/revision from its cache path and supports full commit revisions. The [Hub API page](https://huggingface.co/docs/hub/api) points to its current OpenAPI description and rate-limit policy.

Design inference: expose our own versioned catalog API and isolate upstream API details in a daemon-owned adapter. Capture upstream response fixtures and validate precise endpoint/query/redirect semantics in T025; do not bake undocumented metadata assumptions into the TUI. No Python runtime is introduced. No arbitrary model repository code is executed.

## Decisions and alternatives

- Keep Rust native ax, reuse suitable libraries. A subprocess wrapper around the full Codex CLI would inherit its state/auth/storage assumptions and does not provide our local model picker or fenced orchestrator.
- Keep existing UDS inference workers. Recombining C++ into the daemon is unnecessary for this request.
- Keep JSONL for local session journals and PostgreSQL for relational task coordination. No second database service or SQLite import.
- Keep shared-network launch compatibility initially, explicitly documented and reviewed; do not call it an egress sandbox. A future network proxy restriction is separate work unless security review makes it necessary here.
- Model capability validation is explicit. A tiny test fixture proves runtime transport/generation, while deterministic tool fixtures prove protocol handling; neither proves arbitrary large-model task quality.

## Outstanding implementation gates

Upstream adapter compatibility and toolchain pin; exact Hub transport fixture validation; named Nix-pinned small model with license/hash/size; verification of the specified mTLS/relay role enforcement and frozen dependency-input protocol; real process-group and filesystem race tests. These gates have tasks and must not be silently skipped or replaced after failure.

## Implementation audit — 2026-10-11

The pinned Nix dev shell reports rustc 1.94.1 (e408947bf, 2026-03-25). Upstream's toolchain file requests 1.95.0, but the selected libraries compile and their adapters pass on 1.94.1; retain the existing flake.lock rather than updating unrelated inputs. This establishes the supported compiler, not a tested historical MSRV.

Adopted unchanged Git libraries: codex-utils-pty, codex-file-search and codex-execpolicy 0.162.1 at the single recorded revision. File-search resolves nucleo 0.5.0/nucleo-matcher 0.3.1 at 4253de9faabb4e5c6d81d946a5e35a90f87347ee; PTY resolves portable-pty 0.9.0; execution policy resolves starlark 0.14.2. [upstream-dependencies.txt](upstream-dependencies.txt) records the actual Linux normal/build dependency tree and declared licenses. It contains neither SQLite nor tree-sitter/C++ backend crates. Cargo.lock pins exact versions and checksums.

Upstream workspace crossterm/tungstenite patches are not dependencies of these selected library closures, so no root patch overrides are needed. Nucleo is already a direct pinned upstream Git dependency. Crane successfully fetched/vendored the initial PTY/search dependency closure and built ax; the expanded policy/patch closure receives a separate final build check.

Source inspection found unconditional follow_links(true) in file-search's walker. The user explicitly requested using it unchanged. The adapter therefore treats the root as a search starting point, uses the jail for filesystem isolation, bounds query/results and cancels the blocking walker when its caller is dropped. Its regression test follows a symlink between two disposable test directories; it does not inspect host source trees or /nix/store.

The full apply-patch crate requires codex-exec-server and tree-sitter. The user approved extracting its parser and pure text transforms instead. [vendor/codex-apply-patch/README.md](../../vendor/codex-apply-patch/README.md) records exact source hashes and modifications. Four modules and their upstream tests are byte-identical; the replacement algorithm is retained with crate visibility while filesystem/server wrappers are omitted. Agentix checks stale expected contents before transformation. Upstream's whitespace tolerance and final-newline behavior are preserved.

codex-rmcp-client is not adopted: its manifest directly imports config, exec-server, API, network-proxy, keyring, OAuth and telemetry, including its own HTTP-server transports. The existing workspace rmcp SDK is the selected protocol implementation for T035. This is a source-boundary decision, not a failed compilation. codex-ansi-escape targets ratatui 0.30.2 while ax uses 0.29; retain the existing TUI dependency instead of duplicating UI types. Protocol/core/TUI/app-server decisions above remain unchanged. The third-party 0.63.0 protocol package supplies no needed primitive and is not linked.

Codex Apache-2.0 license/notices and nucleo MPL-2.0 license/source location are retained in the repository and installed with ax. Upstream test assertions have a crate-scoped test-only panic-lint allowance; production retains denied panic lints.

The development shell no longer builds ax-jail (and transitively ax) on entry. This avoids requiring an already-updated lockfile/build just to enter the environment needed to update it. It supplies bubblewrap and explicit test executable paths; launcher packages remain available via nix build/run.
