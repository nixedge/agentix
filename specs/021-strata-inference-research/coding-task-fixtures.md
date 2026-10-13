# Coding and tool benchmark task fixtures

These ten task statements are frozen for the coding/tool benchmark. The pinned
baseline commit is `13f2a5cb52ebc801eab154b009b0ab01f19e7a6a`; it contains the
committed Qwen worker/decoder feature and all paths used by the task statements.
Each task must start independently from a clean checkout of that commit, use the
ordinary Agentix Rust/Nix setup, and must not commit. The evaluator applies each
acceptance command and rejects unrelated edits. The statements do not imply that
an evaluator or baseline run exists. The exact task-file SHA-256 is recorded in
`benchmarks.md`.

## Task 1 — Rust bug fix with regression test

In `agentix-harness/src/stagnation.rs`, make a zero-capacity
`StagnationDetector` safe: repeated pushes must not retain results or panic,
and the detector must never report stagnation when its history capacity is
zero. Add a unit test that checks the internal history remains empty after
repeated pushes. Preserve current behavior for positive capacities.

Acceptance: `nix develop --command cargo test -p agentix-harness`.

## Task 2 — Rust API addition across library and caller

Add `Router::route_with_default(&self, model: &str, default: RouteTarget)` in
`agentix-router` so an unrecognized model can use the caller's explicit
default while all recognized routing rules keep their current behavior. Update
one existing caller in the workspace to use this API for its configured
fallback, and cover both a recognized model and an unknown model in tests.

Acceptance: `nix develop --command cargo test -p agentix-router` and
`nix develop --command cargo check --workspace`.

## Task 3 — Multi-file refactor preserving behavior

Refactor `agentix-qwen/src/budget.rs` so host and device reservation accounting
share one checked summation helper without changing the public API or error
semantics. Keep the helper private. Add or retain tests proving overflow and
independent host/device admission behavior.

Acceptance: `nix develop --command cargo test -p agentix-qwen --test storage`.

## Task 4 — Failing-test diagnosis

Add a focused regression assertion that the mixed-case reference
`hf.co/Qwen/model:Q4_K_M` routes locally, while `anthropic/model` routes to
OpenRouter. Run it before changing implementation; use the observed behavior
to decide whether a code fix is needed. Keep the test as a permanent case.

Acceptance: `nix develop --command cargo test -p agentix-router`.

## Task 5 — Clippy repair

Run Clippy for `agentix-harness`. Fix any warning introduced by the
`StagnationDetector` implementation or tests without weakening lint settings.
If the crate is already clean, make no source change and report that evidence.

Acceptance: `nix develop --command cargo clippy -p agentix-harness -- -D warnings`.

## Task 6 — Nix package/development-shell change

Add a `qwen-check` shell command to the project development shell that runs
formatting, Qwen crate tests, and Qwen Clippy with warnings denied. Keep the
existing `nix develop` interface and avoid adding global tools. Document the
command in the shell's existing help text.

Acceptance: `nix develop --command qwen-check`.

## Task 7 — Configuration and documentation update

Update the Qwen quickstart to document that the text tokenizer has 248,077
addressable IDs while model embeddings are padded to 248,320, and explain that
these sizes must not be used interchangeably. Keep the values consistent with
the pinned configuration and tokenizer evidence.

Acceptance: `rg -n '248,077|248,320' specs/021-strata-inference-research/quickstart.md`.

## Task 8 — Streaming/cancellation bug fix

Inspect `agentix-daemon`'s local chat streaming path. Ensure a client disconnect
cancels the in-flight generation task and releases its owned resources. Add a
focused regression test using the existing streaming test infrastructure; do
not change the SSE wire format.

Acceptance: `nix develop --command cargo test -p agentix-daemon --test integration local_chat_streaming_returns_sse` and
`nix develop --command cargo clippy -p agentix-daemon -- -D warnings`.

## Task 9 — Dependency boundary investigation with cited source evidence

Audit whether `agentix-qwen` can load or execute model tensors without pulling
in the native `agentix-qwen-sys` crate. Record the exact direct dependency tree
commands and their results in a short section of the Qwen quickstart, and
identify which crate currently owns GGUF parsing/dequantization.

Acceptance: `nix develop --command cargo tree -p agentix-qwen --edges normal` and
`nix develop --command cargo tree -p agentix-qwen-sys --edges normal`; the
documentation must agree with both outputs.

## Task 10 — Multi-step shell, test, and Git-diff task

Extend `agentix-qwen/tests/storage.rs` to cover both context-limit rejection
without reducing the prompt/output request and checked-add overflow at
`usize::MAX`. Run the focused test, inspect the final Git diff, and remove
unrelated edits before finishing.

Acceptance: `nix develop --command cargo test -p agentix-qwen --test storage` and `git diff --check`.
