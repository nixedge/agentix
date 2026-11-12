# Physical GPU validation — 2026-11-12

## Hardware and artifact identity

RTX 5090 Laptop GPU, 24,463 MiB VRAM; NVIDIA driver 595.71.05;
AMD Ryzen AI 9 HX 370; 64,928,692 KiB host RAM. This differs from feature 021's
kursk RTX 3090 / Ryzen 9950X / 128 GB baseline.

RustingStrata 1.2.1 at `9dd6340d25c7c6770f7918134a27984cf20d7ce3`, unmodified.
The existing Nix runtime's NVRTC 12.9 compiled CUDA kernels for sm_120; no toolkit
or dependency changes were necessary. Both production GGUF shard hashes match
the publisher values; see model.sha256 and model-integrity.log. Standard-named
symlinks under target/gpu-model.yOlcXd point to the original cached files.
No model conversion or copying was needed.

## Fixture correction and CUDA parity

The initial CPU fixture used 32-wide attention heads. CUDA initialization and
kernel compilation succeeded, then inference rejected that unsupported width
(64/128/256/512 are supported). The owner approved updating the fixture before
production testing. Short-prompt/decode/reset parity passed with 64-wide heads;
the batched case then exposed the quantized GEMM requirement that matrix columns
be multiples of 64. The fixture now uses 64-wide hidden/head/expert/rank geometry
throughout and is approximately 34 MB, below the 50 MB limit. No inference
algorithm or upstream source was changed to accommodate the fixture.

The explicit GPU test compares every vocabulary logit with the CPU reference
within 1% relative + 1e-3 absolute tolerance for GPU Q8 activation rounding, and
requires exact greedy token agreement. Short prefill, four decode steps, reset,
and 64-token batched prefill pass. Maximum absolute logit differences range from
0.00863 to 0.13699; every greedy selection is token 42. See
cuda-fixture-parity.log. This is not full-model tensor-level parity.

## Production smoke and benchmark

Model: pinned ISTA-DASLab Qwen3.8-Flash-Next GSQ-RCO IQ3_S split artifact,
83,617,662,656 bytes total. Both complete shards match their published SHA256.

Bounded smoke: `Reply with OK.`, 16 prompt tokens, max 4 output tokens,
context 128, chunk 64. Output `OK`, token 3793, followed by a stop token;
normal process exit. GPU memory plan: 4,224 MB weights/state/scratch plus
18,874 MB expert slots, 9,671 of 24,576 experts resident. See
production-smoke.jsonl and production-smoke.log.

Benchmark: exact earlier counting prompt, 45 prompt tokens, 128 generated tokens,
context 4096, chunk 256, greedy decoding, MTP disabled. One warmup plus three
measured runs; each request resets sequence state while retaining loaded weights
and adaptive expert placement. All four runs produce identical 128 token IDs.

| Run | Warmup | First token (s) | Decode tokens/s | Total (s) |
|---|---|---:|---:|---:|
| 0 | yes | 6.2307 | 16.1937 | 14.0738 |
| 1 | no | 0.4675 | 39.6275 | 3.6728 |
| 2 | no | 0.2301 | 55.2458 | 2.5291 |
| 3 | no | 0.2237 | 58.2417 | 2.4043 |

Median measured decode: **55.2458 tokens/s**; median first token: **0.2301 s**.
This measures the Agentix library path, excluding HTTP overhead. It is not a
matched speedup comparison with the previous 4.681 tokens/s on different hardware.
The 100–140 tokens/s goal is not achieved here. Rates rise across these repeated
counting runs; they do not establish throughput on arbitrary coding prompts.

## Production HTTP worker acceptance

The installed worker uses the same verified shards, CUDA backend, context 4096
and chunk 256. Startup verifies both complete shard hashes before readiness.
All requests and raw responses/timings are retained alongside this summary.

- Health identifies `rustingstrata`, `cuda`, `qwen4exp`, ready; model discovery succeeds.
- Basic request returns `OK` and `finish_reason: stop`.
- Nonstreaming counting returns `1, 2, 3, 4, 5, 6` with the requested 16-token bound.
- SSE reconstructs exactly the same text and ends with `[DONE]`.
- Tool request produces a valid `run_command` call with JSON arguments
  `{"command":"pwd"}` and `finish_reason: tool_calls`.
- Returning the current directory as the tool result and asking for the output
  yields exactly `/home/sam/home/agentix/033-rustingstrata-qwen`, normal stop.
  This validates the protocol round trip; the worker does not execute tools itself.
- An overlapping request receives HTTP 503 with the original `busy` error code.
- A long SSE request is deliberately disconnected after five seconds (expected
  curl exit 28). A subsequent fresh request returns `OK`; the slot/state recover.
- A 2,234-token prompt returns `OK` with normal stop, 12.47 s total request time.
  Usage reports all 2,234 prompt tokens, establishing no prompt truncation.
- A fresh short request after that long context also returns `OK`.

See http-assertions.json, stream-comparison.json, long-context-assertions.json,
response-busy.status and cancelled-stream.exit for executable assertions/results.
The temporary worker is stopped after acceptance; no system service is deployed.

## Updated checks and decision

After the fixture correction, the full workspace suite passes: 321 passed,
8 ignored. The new GPU-dependent test is one of those ignored by default and
passes in a separate explicit CUDA run. Focused strict Clippy, formatting, and
canonical Nix builds of the Qwen library/benchmark and worker pass. Final package
paths are recorded in nix-build-paths.txt. The gateway source is unchanged from
its earlier successful build/tests. Existing daemon integration-test Clippy
findings remain documented in the feature verification record.

Functional adoption gates on this available GPU pass. Use RustingStrata for Qwen
and stop further custom-engine emulation/optimization work in feature 021, as
the owner requested conditional on successful tests. Preserve 021 and its raw
baseline evidence as history. Separate RTX 3090 deployment, same-host comparison,
and the 100–140 tokens/s target remain future validation, not results of this run.
