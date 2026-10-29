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
