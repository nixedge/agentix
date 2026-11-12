# Verification — 2026-11-11

Base master fd9fe12. RustingStrata 1.2.1 pinned to
9dd6340d25c7c6770f7918134a27984cf20d7ce3, used without source changes.

## Passed

- 37 focused tests: 4 library execution/benchmark CLI, 7 pinned tokenizer/template
  tests including Unicode streaming, 17 worker/protocol/tool tests, a real
  library-backed HTTP worker test, and 8 routing tests. HTTP coverage includes
  streaming vs nonstreaming equality, repeated requests, context admission,
  unknown model, busy response and disconnect recovery. See results/qwen-tests.log.
- Gateway Qwen routing/busy forwarding test and all ten existing gateway
  integration tests. See results/gateway-and-qwen-tests.log.
- Strict all-target Clippy for agentix-qwen, agentix-qwen-worker, agentix-router
  and agentix-api. Strict binary Clippy for agentix-daemon. See results/qwen-clippy.log.
- Workspace Rust formatting check and targeted Nix/Rust fixture formatting.
- NixOS module evaluation with Qwen+gateway enabled: socket/model configuration,
  expected shard hash list, explicit CUDA backend and gateway service ordering.
  See results/nixos-module.json.
- Dependency tree for agentix-qwen-worker includes rusting_strata and contains
  no llama/ggml dependency.
- Canonical `nix build .#agentix-qwen .#agentix-qwen-worker .#agentix-daemon --no-link`
  succeeds, including package tests. Final output paths are in results/nix-build-paths.txt.

## Corrections during verification

- The workspace originally resolved ureq 3.4.0, which lacks the upstream code's
  timeout_per_read method. Updated to RustingStrata's own lockfile version 3.5.1;
  its requirements also update rustls to 0.23.46 and webpki roots/certificates.
  No upstream code was patched.
- The synthetic fixture's Nix generator needed the C linker in its native inputs.
- Brought over the additive ChatMessage tool_calls/tool_call_id fields from 021
  and initialized them in the current assistant, preserving tool history in master.
- Fixed the HTTP test's Unix-socket path argument and a needless-question-mark lint.
- The benchmark's initial template setting used API reasoning_effort=none where
  the native template expects medium plus enable_thinking=false. Corrected this
  and added an executable-level fixture regression test.
- Switched streaming text decoding to the tokenizer's incremental decoder;
  Unicode byte fragments are withheld until valid, with final bytes flushed to
  match nonstreaming output.
- Moved startup shard hashing and tokenizer loading into the blocking pool
  alongside model initialization; the production shard set must not be hashed
  on a Tokio runtime thread.

## Pending/limited gates

Full-workspace tests pass (exit 0): initially 321 passed, 7 ignored, including inference
fixture execution and all workspace documentation tests. See
results/workspace-summary.log and results/workspace-counts.txt. Final
canonical packages pass, with output paths in results/nix-build-paths.txt.
Worker tests and strict Clippy were repeated after the startup I/O correction
and pass; see results/worker-final-tests.log and results/worker-final-clippy.log.
The default development shell began rebuilding its
unrelated launcher closure; those two waiting shell launches were stopped and
workspace tests run in the project's dedicated workspace-tests shell.

All-target Clippy for agentix-daemon additionally checks its pre-existing
integration.rs tests and fails on 25 unwrap_used findings. Those unrelated tests
were not changed. This is not an all-target daemon Clippy pass or merge approval.

The initial 2026-11-11 session had no GPU or production model access and used a
17 MB synthetic CPU fixture. On 2026-11-12 the owner supplied physical GPU access;
the original verified production shards were also found in the repository-family
cache. The fixture was corrected to GPU-compatible 64-wide geometry (34 MB).
The revised workspace suite passes with 321 passed and 8 ignored; the new GPU
parity test is explicitly run and passes separately on CUDA. Final Nix library
and worker builds, focused strict Clippy and formatting pass again.

Production CUDA acceptance now passes on the RTX 5090 Laptop GPU: basic text,
streaming equality, function-call/result round trip, busy/disconnect recovery,
2,234-token input and subsequent clean short request. The repeated 128-token
benchmark has a 55.25 tokens/s median decode rate and 0.230 s median first token.
See [physical evidence](results/2026-11-12/summary.md) for artifacts, measurements
and qualifications. This does not establish full-model tensor parity, a matched
RTX 3090 speedup, or the 100–140 tokens/s goal. No RTX 3090 deployment was performed.
The [adoption decision](decision.md) supersedes further custom engine work in feature 021;
the historical worktree and baseline data remain preserved.

The installed Nix benchmark executable was exercised on the synthetic CPU
fixture, producing the expected `[42,42,42,42]` / `KKKK` twice with distinct warmup
and measured records. See results/packaged-synthetic-smoke.jsonl. Those CPU
fixture rates have no production-model throughput meaning. Verified the
installed rlib and required author/license notices in both package outputs.
