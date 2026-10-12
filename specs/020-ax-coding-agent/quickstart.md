# Validation quickstart and coding checkpoint

Run from `/home/sam/home/agentix/020-ax-coding-agent`. See **Coding checkpoint validation — 2026-10-11 continuation** below for current evidence, limits and launch commands. The initial acceptance plan and foundation notes are retained as history; later model/session/orchestration journeys remain unimplemented.

## Environment

Use the project Nix dev shell and `nix run .#dev` for real PostgreSQL and configured daemon/worker services. T004 must record the fixture's exact identity/hash/license/size and the reference host CPU/RAM/terminal settings here. No production model or multi-gigabyte download is required for automated tests. Tests must fail clearly if required services/fixtures are absent.

`AGENTIX_GATEWAY_URL` points to the administrator-configured trusted daemon. Hugging Face and database credentials stay on that service. The model-store path must be shared consistently by daemon and llama worker. Never copy host credentials into `~/.ax` or the jail. For orchestration, provision a supervisor TLS client identity and daemon host/repository allowlist outside all jail mounts; the launcher creates scoped parent/worker UDS relays. Missing/revoked identity must disable delegation. Legacy cloud API clients must migrate to the typed escalation endpoint in API/daemon 0.2.0.

## Acceptance sequence

1. **Single-agent baseline**: In a disposable repository, launch `ax-jail`, choose a configured compatible model, ask for a small patch and its test, then refer to the result in a follow-up. Observe streaming and concrete command status. Repeat headless with `ax-jail -- exec --json 'Inspect the fixture project'`.
2. **Approvals and cancellation**: Deny a proposed edit and confirm content unchanged. Cancel a command that spawns children and confirm the whole group exits. Cancel generation and ensure no later tool fires. Confirm terminal recovery after failure.
3. **Existing model picker**: Open `/model`; check resident, installed-cold and incompatible records. Select a cold model and verify readiness before activation. Force a load failure and retain the previous selection/history.
4. **Hub flow**: Search a controlled catalog, choose exact repository/file/revision/size, confirm download, watch phases and activate. Repeat offline, gated, rate-limited, bad digest, full disk and cancellation paths. Confirm partials are not listed as installed. Optional real Hub smoke is separate from reproducible fixtures.
5. **Session/context**: Restart/resume, fork, load project instructions and an explicit local skill, run a configured stdio MCP tool and compact near the context limit. Confirm preserved decisions, sources, tool pairs and no replay of an interrupted side effect.
6. **Delegation**: From a clean disposable repo, launch with `ax-jail --workers 2`; exercise a chain and diamond graph. Stop/seal results, approve exact hashes, and verify the dependent's input contains every approved ancestor delta once before its model starts. Inject an input conflict and require Blocked with no model invocation. Parent integration is separate and must preserve current parent edits on conflict. Repeat with staged, unstaged and nonignored untracked parent files: default launch must refuse; `--base <full-commit-id>` must explicitly exclude those files and leave the parent index/content byte-for-byte unchanged.
7. **Recovery**: Kill one worker, revoke another lease, temporarily disconnect the supervisor, cancel a prerequisite and restart the parent. Verify no replacement writer before old-process termination, no stale accepted result and clear blocked reasons. Exercise explicit detach/reattach.
8. **Boundaries**: Run real sentinel/credential/Git-config tests for single and worker jails. Attempt supervisor registration/stopped acknowledgment from a worker relay and directly over HTTP; test forged headers, cross-run access, revoked certificates/capabilities and protected key/artifact mount aliases. Require rejection without retry eligibility. Test every legacy cloud route returns structured_escalation_required with zero provider calls, while valid typed escalation follows policy and local routes retain compatibility. Test model-management source restrictions separately; shared network is not filesystem isolation. Verify no automatic commit/push/cleanup.

## Required automated gates

```bash
nix develop --command cargo fmt --all --check
nix develop --command cargo clippy -- -D warnings
nix develop --command cargo test --workspace
nix build .#ax .#ax-jail .#agentix-api .#agentix-harness .#agentix-infer .#agentix-llama .#agentix-daemon .#agentix-orchestrator .#orchestrator-mcp .#orchestrator-ctl
```

Some attributes above are planned additions under T056. Do not treat missing current attributes as delivered packages. Also require green CI before merge, and retain evidence for actual model generation, PostgreSQL and bwrap tests. Deterministic scripted tool responses establish protocol behavior; they do not establish model coding quality.

## Evidence record

Planning checks: canonical host worktree verified; Spec Kit paths/prerequisites checked; source and upstream package metadata inspected. Implementation, Nix dependency integration, performance measurements, inference, jail/worker runtime and CI checks remain pending. Record actual commands/results and limitations here as T058/T059 execute.

## Implementation evidence — 2026-10-11

- T001 audit and adopted dependency closure are in research.md and upstream-dependencies.txt.
- T003: `nix develop --command cargo test -p agentix-harness -p agentix-codex-patch` passed 35 tests: 21 preserved upstream tests, one pure-text extraction test, four existing harness tests, eight adapter tests and one doctest.
- `nix develop --command cargo clippy -p agentix-harness -p agentix-codex-patch --all-targets -- -D warnings` passed.
- One added mixed-line-ending assertion initially expected a missing final newline to remain missing. It was corrected to upstream's established final-newline behavior; the implementation was unchanged and the suite passed.
- The initial PTY/search-only `nix build .#ax --no-link` passed. The final expanded policy/patch `nix build .#ax .#agentix-test-generation-model --no-link` also passed, completing T002 with unchanged flake.lock and its verified compiler.
- T004: `nix develop --command cargo test -p agentix-infer --test generation_fixture` passed. This verifies the actual GGUF hash, size, architecture and completion capability; it does not yet prove inference or coding quality.
- Generation model: [ggml-org/tiny-llamas](https://huggingface.co/ggml-org/tiny-llamas/tree/99dd1a73db5a37100bd4ae633f4cfce6560e1567), revision 99dd1a73db5a37100bd4ae633f4cfce6560e1567, stories15M-q4_0.gguf, 19,077,344 bytes, SHA-256 6151b1929d7f5aa3385d9ddef3393e55587c0a55de661562322bc51dfda93a04. Nix SRI: sha256-YVGxkp1/WqM4XZ3e8zk+VVh8ClXeZhViMivFHf2pOgQ=. Origin [karpathy/tinyllamas](https://huggingface.co/karpathy/tinyllamas/tree/0bd21da7698eaf29a0d7de3992de8a46ef624add) declares MIT; these are trained-from-scratch TinyStories weights, not Meta Llama weights.
- Nix exposes `agentix-test-generation-model` and `AGENTIX_TEST_GENERATION_MODEL_PATH`. The old embedding fixture/environment variable is retained. Deterministic tool-call/result fixtures are in agentix-harness/tests/fixtures.
- Reference host observed through the existing coding jail: AMD Ryzen AI 9 HX 370, 24 logical processors, MemTotal 64,928,788 KiB. CPU information came from /proc because lscpu could not access the jailed /sys CPU topology. PTY tests use 80x24 then 120x40; no UI latency measurement has run.
- No feature UI/daemon orchestration acceptance, actual model generation, database tests or CI checks have run yet. The adapters are not wired into user commands.

### Shared contracts and turn/inference foundations

- T005: API/daemon manifests are now 0.2.0, with dependent manifest constraints and lockfile updated. Strict schema-v1 agent/model/orchestration DTOs and nine compatibility/validation tests pass. Migration details are in docs/api-0.2-migration.md. Legacy cloud routes are **not yet restricted**: T010/T011 must land before this branch is deployable as the completed 0.2 release.
- T006: the checked turn machine covers preparation, compaction, generation, call validation, approvals, execution, progress evaluation, finalization and terminal states. Six transition tests cover all states and approval/cancellation/budget invariants. Four scripted loop tests prove complete matching results for over-budget batches, duplicate-ID rejection before execution, stagnation handling and one tool-free finalization. The existing runner now uses this machine; actual interactive approvals/cancellation and typed frontend events remain T014–T020.
- T007: typed tool definitions/calls/messages/events and a sticky cancellation signal are library interfaces in agentix-infer/src/exchange.rs. Request validation enforces complete unique tool/result pairs and bounded requests. Backends without tool support explicitly return ToolCompletionUnsupported; actual llama templates and worker cancellation remain T008/T009.
- Pool lifecycle changes keep active models in capacity accounting, forbid active eviction/duplicate residency, serialize same-model loads with cancellation-safe owned locks, and retain guards for completion streams. Six lifecycle tests cover active capacity, rejected-admission preservation, zero capacity, cancelled waiters, concurrent warmup, load failure/retry, stream lifetimes and unsupported/pre-cancelled typed requests. These use a scripted backend and the real pinned GGUF for store metadata; they do not claim actual generation.
- The pool reports admitted resident capacity. Peak backend allocation during loading and backend stop acknowledgment are not yet resource guarantees; worker controls and model-operation reservation must address them in T009/T022–T024.
- Three additional typed-exchange tests cover tool pairs, strict decoding and cancellation notification. Existing inference tests also pass with both fixture environment variables supplied by Nix.
- A first inference compile caught a missing closing brace in the new method. The syntax was corrected without changing approach, then the suite passed.
- `nix develop --command cargo clippy -p agentix-api -p agentix-harness -p agentix-infer -p agentix-codex-patch --all-targets -- -D warnings` passed.
- The combined `cargo test` for those four crates passed 83 tests with no failures or ignored tests (including one harness doctest). Targeted `cargo fmt --check` and `git diff --check` passed. Checksums of the four unmodified upstream modules still match the recorded values after formatting.
- `nix develop --command cargo check -p agentix-daemon -p agentix-llama -p agentix-whisper -p agentix-ax` passed, including both C++ backend dependency builds. This is compile validation, not generation or transcription acceptance.
- The final `nix build .#ax --no-link` passed after T005–T007. Next implementation task is T008 (validated llama tool templates), followed by actual worker transport/generation tests. Full workspace tests, release builds, PostgreSQL, bwrap acceptance and CI remain outstanding.

## Coding checkpoint validation — 2026-10-11 continuation

The earlier foundation notes above are historical. The current implementation now
has a shared ax controller, strict SSE tool-call assembly, scoped approvals,
conflict-checked file tools, Codex filename search/patch/process adapters, streamed
command output, Ctrl+C cancellation, in-memory follow-up history and headless
schema-v1 JSONL with exit codes 0/1/3/130. Direct unjailed coding is refused.
`--allow-cloud` is opt-in; `--no-shell` and `--no-cloud` remove those tools.

Verified during this continuation:

- Qwen2.5's pinned native Jinja template renders tools, argument objects and results;
  strict parser rejects malformed/truncated/unadvertised calls. Four tests pass.
- The actual llama worker generates with the pinned 19 MB TinyStories fixture,
  tokenizes, reports readiness and returns to idle after a dropped stream. This
  fixture has a 256-token context and intentionally does not support coding tools.
- Real daemon HTTP → private worker UDS transport generates fixture text and reports
  coding incompatibility. Cloud-shaped legacy requests are rejected. This is actual
  inference, separate from the scripted coding fixtures.
- Daemon routing fixture tests cover every legacy cloud route, local aliases that
  resemble cloud names, malformed/disabled typed escalations and zero accidental
  provider traffic. A valid enabled typed request makes exactly one fixture call.
- Real bwrap tests pass for sentinels, credential aliases, Git config/hooks,
  single/all-worktree grants and access to the explicit Nix daemon socket.
- Controller HTTP/SSE tests pass for read → search → approved patch → shell test →
  follow-up history, denied edits, headless approval-required and truncated streams.
- Real jailed headless ax passes JSONL schema decoding and exit-code-3/no-write tests.
- File tests cover stale/new preimages, read-only roots, symlink/traversal rejection,
  binary input and deletion. Process tests cover stdin, exit status, bounded output,
  cancellation and descendants holding output open.
- Ax/harness/API/infer/router all-target clippy passed with denied warnings at the
  first checkpoint sweep. A broader daemon/llama all-target sweep found pre-existing
  `unwrap()` violations in old daemon integration tests; it is not a passing release
  gate. Newly added daemon fixtures are checked separately.
- Final Nix builds pass for `ax`, `ax-jail`, `agentix-daemon` and `agentix-llama`.
  The packaged ax help includes `--check`, `--json` and opt-in `--allow-cloud`.
- The real jailed PTY test passes for Unicode cursor editing, bracketed paste,
  approval, writing a file, follow-up history and restoring the terminal on exit.
  The simultaneous-resize/key regression now also passes (see picker continuation).
- Real jailed headless SIGINT preserves partial schema-v1 events, emits
  `turn_cancelled`, and exits 130. The symlink-swap stress test and patch-preflight
  conflict test pass. Shell tests also verify actual PTY allocation and timeout.
- Final ax/harness all-target clippy passes after formatting a flagged assignment.
  Targeted daemon/llama binaries and their new fixtures pass denied-warning clippy;
  fmt checks for all changed crates and `git diff --check` pass.

The interactive resize regression is fixed with the user-approved crossterm 0.29
update, its upstream `use-dev-tty` polling backend, and bounded 1 ms input polling
on a blocking worker. The version update alone still used the affected Mio backend;
the TTY backend also requires nonzero poll duration. All three real jailed terminal
tests pass, including model selection followed by an approved edit and follow-up.
T019/T021 are complete; 21 of the 59 feature tasks are checked complete.

This is not complete Codex parity. Restart persistence, context compaction, project
instruction/skill loading, Hugging Face download UI and multi-agent orchestration
remain later tasks. Existing MCP CLI integration is retained; its full protocol,
shutdown and authorization hardening remains T035. Coding tools currently accept
directory grants only, edits require existing parent directories, patches reject
renames, and multi-file patch writes are not transactional. Approval bodies are
limited to 8 KiB so every approved argument is displayed in full.

### Try the checkpoint with an installed compatible model

Use this branch's **daemon and llama worker as well as ax**. An older deployed daemon
will not provide the new readiness/tool contracts. Deployment to the user's server
is authorized but still pending SSH access; no host service has been changed.

### Model picker continuation

Rebase integration (2026-10-11): the checkpoint is based on master `fd22028`,
including the home assistant and Piper voice service. Piper routes, voice-directory
configuration and voice/Whisper implementations are preserved. The harness retains
`with_system_prompt` across turns without duplicating the system message. Existing
home-assistant tools explicitly retain service-level authorization; coding tools
still require their per-action approvals. Home-assistant test fixtures now use SSE
and the current HTTP client field. Its Nix package includes the harness's vendored
patch dependency.

Post-rebase validation: `cargo test --locked -p agentix-agent -p agentix-harness
-p agentix-ax -p agentix-codex-patch` and daemon `--test local_routing` pass in the
Nix dev shell. These include home-tool execution, system-prompt history, Piper
route preservation, model selection, and real terminal/jail tests. Nix derivation
evaluation passes for ax, home agent, voice, daemon, llama and Whisper. Full Nix
package builds have not been repeated locally after this rebase; the older build
outputs below predate this integration. Post-deployment checks are recorded below.

`AGENTIX_GATEWAY_URL` selects the endpoint and is explicitly passed through
`ax-jail`. The user's existing server is `http://ai.lan.disasm.us:11434`.
After the user deployed the updated daemon and worker, live checks on 2026-10-11
confirmed the catalog and readiness APIs. Qwen2.5-7B-Instruct reports coding support
and 32768 context. The packaged interactive picker fetched the live catalog,
filtered to Qwen, and selected it. A headless turn called `read_file` and returned a
random value from a disposable project file. An interactive turn requested approval
for `write_file`; after approval it wrote the exact expected `AX_LIVE_OK` contents.
Both turns completed successfully. Piper's voice list still returned
`en_US-lessac-medium`. These are smoke checks, not broad model-quality validation.

To use the deployed services:

```sh
export AGENTIX_GATEWAY_URL=http://ai.lan.disasm.us:11434
nix run /home/sam/home/agentix/020-ax-coding-agent#ax -- --list-models
# Run from a disposable project directory for the first coding test:
nix run /home/sam/home/agentix/020-ax-coding-agent#ax-jail
```

With no `--model`/`AGENTIX_MODEL`, ax opens the installed-model picker on startup.
The explicit `nix run .#ax-jail -- --dangerous` option skips tool approvals in both
interactive and headless modes. It retains jail/path/conflict checks and disabled
tools; direct unjailed coding remains unavailable. The launcher already forwarded
this flag, but ax's parser and controller lacked the corresponding policy before
the 2026-10-12 fix. Default-mode denial, dangerous-mode writing, launcher argument
forwarding, and disabled-tool/conflict behavior are covered by regression tests.
Use `/model` between turns to reopen it; type to filter, arrows to navigate, Enter
to load/select, Ctrl+R to refresh, and Esc to close. Residency is distinct from
installation and coding support; cold models are checked on load. Failed, busy,
incompatible or insufficient-context selections retain the current model. A late
result from a closed picker cannot override a newer selection. Closing the picker
cancels selection, not a server-side load already underway.

Selection checks tokenized serialized history plus a 4096-token coding reserve.
Exact rendered prompt/tool budgeting and persisted readiness operations remain
unfinished Phase 4 work. Selection is in-memory; use `AGENTIX_MODEL` or `--model`
for a repeatable explicit startup choice. `--list-models` and `--check --model ID`
work outside the jail; coding itself requires `ax-jail`.

Validation: ax/harness/infer suites pass; selection tests cover paginated catalogs,
busy/incompatible/too-small contexts, load errors and stale selection results.
The actual daemon/worker fixture verifies catalog metadata, hash, residency and
invalid page-size rejection. Targeted Clippy passes for these crates and daemon/
worker targets. The new Nix rebuild exhausted the shared disk twice. Tested
executables were preserved in `/tmp/agentix-020-validation`, then only this
worktree's disposable development build output was cleaned before another serial
retry; source files and other projects were left intact.

The serial retry succeeded for all four packages. Stable ignored build links in
this worktree are `result-ax-jail`, `result-daemon`, and `result-llama`. The packaged
ax passes all three terminal tests. The packaged daemon passes the real transport
test with the validated CPU worker. The CUDA package builds but cannot start in
this session because the host NVIDIA `libcuda.so.1` is not exposed; its runtime
validation remains pending on the server before deployment.

The actual packaged launcher passed `AGENTIX_GATEWAY_URL` through to ax and
completed a live text-only turn against the existing server's Qwen2.5 model:
streamed `AX_CONNECTION_OK`, schema-v1 `turn_completed`, exit 0. This verifies
connectivity, not coding-tool support in the old server. To reproduce from a
disposable project directory before deployment:

```sh
AGENTIX_GATEWAY_URL=http://ai.lan.disasm.us:11434 \
AGENTIX_MODEL='hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Qwen2.5-7B-Instruct-Q8_0.gguf' \
/home/sam/home/agentix/020-ax-coding-agent/result-ax-jail/bin/ax-jail -- \
  --json 'Reply exactly AX_CONNECTION_OK. Do not call tools.'
```

The earlier SSH deployment blocker was resolved by the user's deployment; no SSH
access was required for the subsequent live API and coding smoke checks. The exact
server build identity and CUDA configuration were not inspected through SSH.

### Durable warm-operation foundation (not yet connected to the UI)

The infer library now provides a synchronous `OperationStore` and asynchronous
`WarmOperations` runner over an injected `WarmLoader`. Snapshots use private
0700 directories/0600 files, exclusive OS writer locks, atomic replacement and
file/directory fsync. Defaults bound active operations to 8, retained records to
1024 and each snapshot to 4 MiB; terminal records expire after 24 hours. Reusing a
key returns the same job; changing its request is rejected. Restart marks pending
jobs interrupted and never retries them automatically. Failed writes require
reopening before further access. Backend error codes are fixed, without raw
upstream diagnostics.

Queued cancellation is immediate. During loading it records cancellation intent
but retains capacity until the loader returns; a late successful load then becomes
cancelled rather than ready. This does not claim cancellation stops backend FFI.
All persistence runs on blocking threads through the async runner, and accepted
work survives a submitter disconnect. HTTP/SSE routes, daemon loader assembly,
picker reconnection and exact rendered-context budgeting remain pending.

Validation: 12 operation tests cover persistence, idempotency conflicts, restart,
stale results, cancellation, capacity, retention, writer locking, corruption,
private paths, failed writes, concurrent retries and loader failures. All 41 infer
tests and `cargo clippy -p agentix-infer --all-targets -- -D warnings` pass through
the Nix dev shell. No daemon deployment is required for this library-only change.

Build from this worktree:

```sh
cd /home/sam/home/agentix/020-ax-coding-agent
nix build .#ax .#ax-jail .#agentix-daemon .#agentix-llama --no-link
```

For an isolated local test, start the worker in one terminal with an existing model
store (replace the model-store path; this does not download new weights):

```sh
mkdir -p "$HOME/.local/state/agentix-coding-checkpoint"
AGENTIX_LLAMA_SOCKET="$HOME/.local/state/agentix-coding-checkpoint/llama.sock" \
AGENTIX_MODELS_DIR=/absolute/path/to/existing/model-store \
AGENTIX_LLAMA_MODELS= \
nix run /home/sam/home/agentix/020-ax-coding-agent#agentix-llama
```

Start the matching gateway in another terminal on a separate localhost port:

```sh
AGENTIX_LLAMA_SOCKET="$HOME/.local/state/agentix-coding-checkpoint/llama.sock" \
AGENTIX_GATEWAY_HOST=127.0.0.1 AGENTIX_GATEWAY_PORT=11431 \
AGENTIX_ALLOW_CLOUD_ESCALATION=0 \
nix run /home/sam/home/agentix/020-ax-coding-agent#agentix-daemon
```

List installed IDs with `curl http://127.0.0.1:11431/v1/models`, then check one:

```sh
nix run /home/sam/home/agentix/020-ax-coding-agent#ax -- \
  --gateway http://127.0.0.1:11431 --model 'EXACT_INSTALLED_ID' --check
```

It must report `coding_supported: true`. Validated Qwen-style tool templates are the
initial supported profile; other templates fail explicitly. The tiny generation
fixture is not a coding model. No coding quality claim is made for a particular
installed production model until it is exercised.

From a disposable project or worktree you want ax to edit:

```sh
nix run /home/sam/home/agentix/020-ax-coding-agent#ax-jail -- \
  --gateway http://127.0.0.1:11431 --model 'EXACT_INSTALLED_ID' --no-cloud
```

Ask it to read a small file, make a small change, run the relevant test and report
the observed result. Review each write/command with `y` or `n`; use arrow/Page keys
to inspect wrapped approval text. Ctrl+C cancels a running turn and waits for command
cleanup; Ctrl+C while idle exits. Ask a follow-up in the same session to exercise
history. Restart persistence is not implemented yet.

A positional task runs headless; add `--json` for versioned event envelopes. Reads
can complete headlessly, but writes/commands return exit code 3 instead of waiting
for approval. `-` reads a task of at most 64 KiB from stdin.

### Qwen3-Coder checkpoint

User-selected model: `hf.co/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF:Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf`
(18.6 GB weights). Expected SHA-256:
`fadc3e5f8d42bf7e894a785b05082e47daee4df26680389817e2093056f088ad`.

The native function/parameter parser lives in agentix-llama; deploy the updated
**llama worker**. The daemon's existing structured-tool transport needs no protocol
change. Rebuild ax/ax-jail to get the coding system prompt (Git status, staged and
unstaged diffs for reviews; read-before-edit; evidence-backed test reporting).
Automatic AGENTS.md and skill loading is still T033, not implemented here.

For the 3090, start with these environment settings on the **worker service**, not
in the ax terminal:

```sh
AGENTIX_MAX_CTX=16384
AGENTIX_MAX_LOADED_MODELS=1
```

With the repository's NixOS module, the corresponding settings are:

```nix
services.agentix.llama.maxCtx = 16384;
services.agentix.llama.maxLoadedModels = 1;
```

Keep its existing CUDA configuration. The model file size excludes KV cache and
runtime buffers. Leave space for any Whisper residency; simultaneous GPU workloads
have not been validated. The model's advertised 256K context is not a 24 GB memory
budget. Restart the worker through the normal deployment so settings take effect.

If the model is not already installed, the existing operator pull endpoint is:

```sh
curl --fail-with-body -H 'Content-Type: application/json' \
  -d '{"name":"hf.co/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF:Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf"}' \
  http://ai.lan.disasm.us:11434/api/pull
```

This legacy endpoint downloads without warming and returns only after completion;
it is not the planned resumable Hub/job UI. Verify the catalog SHA-256 against the
value above. Do not infer readiness from a successful download.

After deployment:

```sh
export AGENTIX_GATEWAY_URL=http://ai.lan.disasm.us:11434
nix run .#ax-jail
```

Filter the picker to `Qwen3-Coder`, select it, and require readiness to succeed.
From a disposable Git project, ask for a review of a small staged/unstaged change;
verify real `run_command`/file-read events and an evidence-based answer. Then ask
for a small file edit, approve it, and inspect exact bytes plus the resulting diff.
Live CUDA generation/tool use with this model remains pending the user's deployment.

Local validation on 2026-10-12:

- Pinned upstream Qwen3-Coder template rendering/history replay, typed arguments,
  nullable edit preconditions, literal strings/whitespace, malformed/duplicate/
  unavailable calls, batch atomicity and size/count bounds pass; Qwen JSON
  regressions remain green.
- `cargo test -p agentix-llama -p agentix-ax -p agentix-harness -p agentix-codex-patch`
  passes under Nix, including real terminal/approval/resize and small-model UDS
  generation/tokenization/cancellation checks. Two existing optional grammar
  generation tests remain explicitly ignored; this is not a full workspace gate.
- The required infer `generation_fixture` check passes. Targeted all-target Clippy
  for llama/ax/harness and workspace formatting pass. Three old grammar-test
  unwraps were replaced with checked assertions; terminal fixtures now find the
  user message by role after adding the system prompt.
- Logs: `/tmp/agentix-qwen3-full-tests.log`,
  `/tmp/agentix-qwen3-grammar-tests.log`, `/tmp/agentix-qwen3-clippy.log`,
  `/tmp/agentix-qwen3-generation.log`, `/tmp/agentix-qwen3-fmt.log`.

The operator pull completed successfully on ai.lan.disasm.us on 2026-10-12.
The catalog reports the exact ID above, architecture `qwen3moe`, 18,556,689,568
bytes and the expected SHA-256. Residency is `unloaded`; coding support remains
`unknown` until the updated worker is deployed and the model is warmed. No server
service was deployed or restarted by this change.

Final `nix build .#agentix-llama .#ax-jail --no-link -L` succeeded, including the
CUDA worker (configured CUDA architectures 86 and 120) and updated client.
Build log: `/tmp/agentix-qwen3-build-final.log`. The host NVIDIA driver is a runtime
dependency supplied by the deployed machine; this build is not a CUDA runtime test.
Workspace rustfmt also normalized existing formatting in six voice-related files;
those changes are formatting-only. Live acceptance remains pending the user's deployment.

### Qwen3-Coder deployed-template correction — 2026-10-12

After the user's deployment, the actual Q4_K_M loaded successfully but reported
`coding_supported: false` (worker context 32768). The GGUF embeds an Unsloth-modified
6896-byte template rather than the original Qwen tokenizer template used by the
first regression fixture. Its parameter loop requires `parameters.properties`;
the compatibility probe supplied only `{"type":"object"}`, causing rejection.
The probe now advertises a complete parameter schema.

The exact embedded template is retained unmodified in
`agentix-llama/tests/fixtures/qwen3-coder-unsloth.jinja`, with source revision,
GGUF/template hashes and its Apache-2.0 notice. Expanded replay tests also exposed
non-JSON nested-value display in that template. The Coder adapter now passes
non-string historical arguments as JSON text, preserving string content verbatim.
Both original-Qwen and embedded-Unsloth templates round-trip nested nulls/booleans,
arrays, escaped text and file whitespace. Worker logs now report template rejection
reasons instead of silently discarding them.

The llama suite and all-target Clippy pass; the real small-model UDS worker check
also passes. Two existing optional grammar-generation tests remain ignored.
Logs: `/tmp/agentix-unsloth-all-tests.log`, `/tmp/agentix-unsloth-clippy.log`.
The separate Nix entry-point fix sets `meta.mainProgram = "ax"`; both
`nix run .#ax -- --help` and live `--list-models` work.

Live Qwen3-Coder tool acceptance remains pending another worker deployment with
this correction. The model weights need no further download.

The corrected CUDA worker Nix build also passes:
`nix build .#agentix-llama --no-link -L`.
Log: `/tmp/agentix-unsloth-build-final.log`. Formatting and `git diff --check` pass.
