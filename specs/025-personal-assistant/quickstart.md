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

Increment C adds the engine-free `agentix-assistant` application library, additive Sled turn/message journal and opt-in `POST /v1/assistant/tell` routes in agentix-agent. The new API requires an owner bearer token, request/conversation UUIDs and an IANA timezone. No service was deployed. Owner-approved focused item-text/task-completion tools replace generic model replacement; Rust validates target/revision and preserves other fields. The expanded two-step operation/replay gate now passes; Android has an opt-in HTTPS adapter. Service deployment and physical phone acceptance remain open. See the final increment below and research.md.

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

This opt-in live gate is distinct from hermetic fixture/HTTP tests. It checks actual persisted effects, immutable replay, ambiguous-time clarification and fixed unsupported-action replies. The two-step adapter passed13 cases in three fresh stores; see the final increment below. Broader model reliability, VRAM, load, phone and backup acceptance remain separate gates.


## Two-step capability and Android increment

The local adapter selects an operation first, then fills only that operation's fields (or clarification). Unsupported email/calendar/reminder/other operations return fixed Rust answers without a second model call or mutation. Three fresh-store trials passed all13 cases, including supplied email address, send confirmation/status, calendar, reminders and the original ambiguous-time/task/correction cases. Warm synthetic times were about0.35–0.4s for unsupported answers and0.7–1.5s for supported operations; not a general reliability or memory qualification.

Backend validation:45 regression tests (26 core/Sled/application +19 agent) and strict targeted Clippy pass. Canonical assistant-core, assistant-lance (8 projection tests), assistant-api (5 HTTP fixture tests) and agentix-agent targets pass. Full workspace/GPU tests and runtime memory measurement remain open.

Android now defaults to the owner-supplied configurable address `https://ai.lan.disasm.us`, with backend disabled until explicitly enabled and given the owner token in Settings. Settings are private and backup-excluded. The immutable instruction/request/conversation/timezone/endpoint envelope is committed locally before dispatch. Uncertain HTTP/auth/transport/parse/cancellation results keep it; retry reuses the same request. Other instructions and endpoint changes are blocked while unresolved. A validated definitive response is persisted atomically with clearing the pending envelope. Recovered replies/pending instructions are visible when the activity reopens. No auto-retry or default-assistant change.

```bash
nix build .#agentix-android --out-link result-assistant-android
```

APK: `result-assistant-android/share/agentix-android/agentix-android-debug.apk`. Canonical build runs all core JVM tests (including4 new retry/identity/recreation/storage-failure tests) and Android lint. No new Gradle dependencies. HTTPS uses normal certificate validation, no redirects, bounded response size/read deadlines; bearer token is never in Git/Nix or request logs.

Live handoff: the supplied HTTPS endpoint returned502 during a GET of `/v1/assistant/tell`; routing/deployment/readiness is not established. Configure the existing agentix-agent service with the opt-in `[assistant]` section above, provide its owner token privately through the runtime environment file, and route only the protected assistant path from HTTPS to that listener. Then enable matching settings on the phone and test grocery add/read/correction and an uncertain retry. API401 without a token, followed by a valid authenticated instruction and unchanged replay, are required live checks. No service was deployed or phone installed/toggled here. Keep current default assistant until real usefulness is verified; never uninstall an existing app with pending requests or recordings to work around debug-signature changes. Physical persistence, lifecycle/TLS/network behavior and microphone/TTS remain phone acceptance items, separate from JVM fixtures/lint.

For nginx on the same host as agentix-agent, whose example `bind_addr` is `127.0.0.1:8765`, the upstream is that agent listener:

```nginx
location /v1/assistant/ {
    proxy_pass http://127.0.0.1:8765;
    proxy_set_header Authorization $http_authorization;
    proxy_read_timeout 150s;
}
```

Use the service's actual configured bind address if different. Preserve the full `/v1/assistant/tell` path; `proxy_pass` above has no URI suffix. The agent service performs bearer-token checks and talks separately to the configured inference gateway (`http://ai.lan.disasm.us:11434` in the development config). No nginx configuration was changed by this feature work.
