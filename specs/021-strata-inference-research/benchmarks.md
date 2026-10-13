# Benchmark protocol

**Status**: Protocol proposal frozen for implementation; no measurements recorded.
**Target**: RTX 3090 24 GiB, Ryzen 9 9950X, 128 GiB RAM (target-host inventory still incomplete).

This protocol is intended to keep runtime comparisons repeatable. Exact pinned prompt fixtures are now available, but the suite is not executable yet because the full model loader and target host are unavailable. Do not report projected values as measurements.

## Model and runtime identity

Every record names the exact model revision, acquired shard SHA-256 values, quantization, tokenizer/template hashes, runtime commit, Nix flake lock, CUDA/driver versions, and every enabled optimization. A full-model identity is mandatory. Keep the IQ3_S full-expert candidate as the provisional quality candidate and IQ2_XS as the capacity comparison only after their actual tensor mapping and memory fit are verified. Record the Coder model separately; it is not a matched-model comparison.

The direct Strata reference uses the pinned Strata revision and the same exact model artifacts and quantization. For correctness and matched performance, disable tail skipping and any other output-changing approximation. An upstream-default Strata speed run may be reported separately and labeled as approximate. Agentix and Strata use identical tokenizer/template, prompt token IDs, sampling settings, output limit, context, warm/cold state, and stopping conditions.

## Fixed workload matrix

| Workload | Prompt length | Maximum generated tokens | Repeats | Purpose |
|---|---:|---:|---:|---|
| Short prefill/decode | 4,096 tokens | 512 | 3 cold + 3 warm | Baseline startup and ordinary interactive use |
| Long prefill/decode | 32,768 tokens | 512 | 3 cold + 3 warm | Main long-context target |
| Capacity probe | 65,536 tokens | 128 | 3 cold + 3 warm if admitted | Budgeted context and memory probe |

The checked-in `fixtures/benchmark_prompts.json` contains the exact rendered prompts and token IDs for 4,096, 32,768, and 65,536 tokens, plus per-prompt little-endian token-ID SHA-256, vocabulary/stop-ID assertions, and construction provenance. It is generated with the Nix-pinned tokenizer/template by `nix develop --command cargo run -p agentix-qwen --example build_benchmark_prompt_fixtures`; `cargo test -p agentix-qwen --test benchmark_prompts` re-renders each prompt and verifies its token IDs and hash. The deterministic fixture explicitly records its repeated text segment and padding suffix, so prompt construction does not silently repeat text. Fixture file SHA-256: `c5a255bd4a271c3a14eebc762617dc511757dfa55b2019482d0d0075f021c939`. Cold means a fresh worker process and empty OS page cache where the operator can safely arrange it; if page-cache control is unavailable, record the limitation and call the run process-cold only. Warm means one unmeasured priming request followed by the measured request in the same loaded worker. Three measured repetitions are reported individually, with median and min/max; no outlier is discarded.

Use greedy decoding (temperature 0, top-p 1, top-k disabled, fixed seed recorded) for correctness and matched engine comparisons. If either engine cannot disable a sampler option, record the exact effective settings and mark the run unmatched. Stop at EOS or maximum output length. Record actual prompt and accepted output token counts.

## Measurements

Record worker startup, model validation/load duration, cold and warm request TTFT, prefill tokens/s, accepted decode tokens/s, end-to-end duration, and task completion time. Also record peak process RSS, peak host memory, peak and free VRAM before/after/peak, page faults, swap-in/out, PLE/expert cache hit/miss/eviction counts, host-device bytes, transfer stalls, and (for speculation) draft/accepted/rejected token counts. Keep raw per-run JSON/CSV and logs under `results/<date>/<engine>/<model-id>/`; summarize medians and ranges in the report. Define TTFT from request submission through the first generated output token, excluding model load and including tokenization/prefill.

Before each run, record background GPU processes, RAM/VRAM availability, storage free space, power mode, temperatures, clocks, PCIe link, and whether embeddings/voice or other inference services are active. Do not run concurrent inference workloads during the matched suite. If throttling, swapping, OOM, or service interruption occurs, retain the run record and label it invalid for parity while preserving the diagnostic evidence.

## Coding and tool suite

Use ten deterministic tasks against one pinned Agentix repository commit and a clean disposable worktree per task. Each task gets the same task statement, initial repository state, tool permissions, timeout, and maximum generated tokens for both engines. The evaluator runs the prescribed build/test/lint or scripted acceptance command and stores the diff, tool transcript, exit status, elapsed wall time, and evaluator output. A task passes only when its acceptance command succeeds and the diff has no unrelated changes. Report pass count and median completion time over successful tasks; include failures and timeouts in the denominator.

The task set must cover: (1) Rust bug fix with regression test, (2) Rust API addition across library and caller, (3) multi-file refactor preserving behavior, (4) failing-test diagnosis, (5) Clippy repair, (6) Nix package/development-shell change, (7) configuration and documentation update, (8) streaming/cancellation bug fix, (9) dependency boundary investigation with cited source evidence, and (10) multi-step task using shell, test, and Git diff tools. Exact statements, repository commit, acceptance commands, and hashes are a required checked-in fixture before T009/T011 can be marked complete.

## Comparison and reporting rules

Report same-model/same-quantization runtime comparisons separately from different-model or different-quantization comparisons. Pair each result with correctness status and effective approximation/sampler settings. Evaluate SC-003 only when the 4K and 32K configurations each have three valid runs and the 64K probe has three runs or a documented resource-based admission rejection. Evaluate SC-004 on the fixed ten-task suite. Evaluate SC-005 against same-host direct Strata medians: Agentix prefill and decode throughput must each reach 90% of reference and TTFT must be at most 110% of reference, with no task-suite correctness regression. If a required reference cannot run, report the metric as unavailable; do not substitute published or other-host numbers.
