# Personal record and retrieval libraries

Worktree: /home/sam/home/agentix/025-personal-assistant.

The durable foundation and lexical projection expose Rust libraries. HTTP/tools, natural-language execution, semantic embeddings and the Android backend connection remain later increments. No GPU, model server, embedding service or production database is needed for these checks.

Run from this worktree:

```bash
nix build .#assistant-core .#assistant-lance
nix develop .#assistant --command cargo test -p agentix-assistant-core -p agentix-assistant-sled -p agentix-assistant-lance
nix develop .#assistant --command cargo clippy -p agentix-assistant-core -p agentix-assistant-sled -p agentix-assistant-lance --all-targets -- -D warnings
nix develop .#assistant --command cargo fmt --all --check
nix develop .#assistant --command alejandra --check perSystem/packages.nix
```

The focused `assistant` shell provides the project's pinned Rust toolchain and required native dependencies without bundling the coding-agent/jail executables. The default project dev shell remains available. Nix package/check `assistant-core` executes core and Sled tests; `assistant-lance` executes the real storage projection integration suite.

Application composition uses these boundaries:

- Open `SledRepository` in a dedicated authoritative directory; call the `Repository` trait with caller-generated command/request IDs and expected revisions. On uncertain failure, retry the identical request ID and command.
- Open `LanceProjection` in a separate dedicated index directory. The profile is `personal-lexical-v1`, explicitly without vectors. An independent process/owner cannot open the same index; clones share it.
- Call `project_pending(&repository, &projection, limit)` for one bounded worker pass. Retry failed passes with backoff in the later hosting service. Saves work independently; this library does not start an automatic daemon.
- Call `search_records` for rehydrated results, including pending/discarded-candidate indicators. Querying `Projection::search` directly returns untrusted candidate IDs/revisions rather than authoritative records.
- For rebuilding, create a new directory with `LanceProjection::open_fresh`, then call `rebuild_projection`. Catch up through `project_pending` before selecting the new index in the hosting application. No live directory is deleted or swapped by these libraries.
- Release cloned repository handles and await `SledRepository::close` before immediately reopening its path. Closing with active clones/operations fails clearly. Ordinary Drop offloads cleanup, so it does not synchronously guarantee directory-lock release.

Use only temporary/copied data during this increment. Pointing the new repository at a legacy unversioned Sled store is rejected. No automatic legacy migration occurs. Durability tests use actual local storage and close/reopen; they do not constitute power-loss qualification. Backup/restore and phone acceptance are later explicit gates.

## Validation

Verified on 2026-10-29:

- Both canonical Nix targets passed. `assistant-core` executed two domain tests and nine real Sled tests; `assistant-lance` executed eight integration tests using actual Sled and LanceDB. No model, GPU or service was required.
- The targeted development-shell test command passed all 19 tests, with zero failures. Foundation coverage includes replay/reopen, concurrent edits, reference integrity, stale outbox acknowledgement, schema/corruption rejection and awaited close/reopen. Projection coverage includes lexical filters/limits, revision barriers, tombstones, authoritative rehydration, outage preservation, cancellation before ack, restart/replay, fresh rebuild, single ownership and independent profile/dimension/schema guards.
- Strict all-targets Clippy passed for all three crates. Workspace Rust formatting, Alejandra and Git whitespace checks passed.
- The branch was refreshed onto master `cd83a25` after Android024 merged. Assistant source/manifests/build definitions were verified unchanged across that refresh.

Full workspace Cargo tests/Clippy, live Qwen evaluation, personal semantic-profile evaluation, power-loss behaviour, production load, backup/restore, service deployment and phone end-to-end acceptance are separate gates and have not been established by these targeted checks.

## Conversational backend work in progress

Increment C adds the engine-free `agentix-assistant` application library, additive Sled turn/message journal and opt-in `POST /v1/assistant/tell` routes in agentix-agent. The new API requires an owner bearer token, request/conversation UUIDs and an IANA timezone. No service was deployed. Owner-approved focused item-text/task-completion tools replace generic model replacement; Rust validates target/revision and preserves other fields. Live synthetic operation/replay checks pass, but broader clarification quality remains incomplete. See research.md. Android remains disconnected.

Backend checks:

```bash
nix build .#assistant-core .#assistant-lance .#assistant-api .#agentix-agent
nix develop .#assistant --command cargo test -p agentix-assistant -p agentix-assistant-core -p agentix-assistant-sled
nix develop .#assistant --command cargo test -p agentix-agent
```

After the model gate passes, enable the new routes by adding an explicit section to the existing agent.toml, with dedicated directories:

```toml
[assistant]
db_path = "/var/lib/agentix-agent/personal"
index_path = "/var/lib/agentix-agent/personal-index"
token_env = "AGENTIX_ASSISTANT_TOKEN"
inference_timeout_secs = 30
```

Supply the owner token at runtime through `services.agentix.agent.environmentFile` or the process environment. It must be32–512 visible ASCII bytes and must not be embedded in Git/Nix configuration outputs. The application uses the configured existing local gateway/model; cloud model choices and redirects are rejected. The listener/bind address is unchanged. Route the protected `/v1/assistant/` path through the owner's HTTPS/WireGuard entry; legacy routes retain their existing access policy.

Opt-in synthetic live acceptance (temporary personal store, existing installed model):

```bash
AGENTIX_ACCEPTANCE_GATEWAY=http://ai.lan.disasm.us:11434 \
AGENTIX_ACCEPTANCE_MODEL=hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Qwen2.5-7B-Instruct-Q8_0.gguf \
nix develop .#assistant --command cargo test -p agentix-agent assistant::tests::installed_qwen_acceptance -- --ignored --nocapture
```

This opt-in live gate is distinct from hermetic fixture/HTTP tests. It checks actual persisted effects and request replay for grocery add/read/correction, note/task creation, task completion and no-write clarification for ambiguous time/unsupported email. The final prompt passed three fresh-store trials (24 cases). Earlier task-completion failures used the next revision instead of the current one; the prompt now states the distinction explicitly. These assertions do not establish the quality of clarification text: Qwen still sometimes offers email confirmation although email is unavailable. Broader conversational quality, VRAM and full load/phone/backup acceptance remain unestablished.

Focused backend validation (2026-10-29): 25 core/Sled/application tests and17 agent regression tests passed, with the manual Qwen gate ignored in hermetic runs. Strict all-targets Clippy passed for core/Sled/application; production-agent Clippy and workspace rustfmt passed. Canonical `assistant-core`, `assistant-lance` (8 projection tests), `assistant-api` (3 HTTP fixture tests) and `agentix-agent` Nix targets all completed successfully for the final source. Full workspace/GPU checks were not run.
