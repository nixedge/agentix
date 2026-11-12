# Feature Specification: RustingStrata Qwen replacement

**Feature Branch**: `033-rustingstrata-qwen`
**Created**: 2026-11-11
**Status**: Implementation authorized
**Input**: Use RustingStrata directly as a Rust library in a new worktree; test it as the replacement for custom Strata emulation. If successful, abandon that custom implementation.

## User Scenarios & Testing

### User Story 1 - Run Qwen with RustingStrata (P1)
As the operator I can build and run the Qwen worker with the RustingStrata library as its sole execution engine.
**Independent test**: Build the Nix package and run a small pinned Qwen fixture through the library on CPU; run the actual Qwen3.8 split GGUF on the RTX 3090.
1. Given valid GGUF weights, when the worker starts, then it loads the library once and exposes readiness only after initialization.
2. Given a chat request, when generation runs, then inference executes outside Tokio and uses the library's prefill and decode operations.
3. Given a missing GPU or unsupported artifact, when initialization fails, then the worker reports the reason and does not silently switch engines.

### User Story 2 - Preserve Agentix behavior (P1)
As a client I retain model discovery, nonstreaming/streaming greedy text, function tools, context admission, stop handling, cancellation, and busy responses.
**Independent test**: Worker contract tests and real fixture HTTP requests, including consecutive requests, disconnect and overlap.
1. Given two requests, when the second arrives during generation, then it receives busy without altering the first sequence.
2. Given cancellation or completion, when the next request begins, then model state is reset and no previous request leaks into its output.
3. Given a model routed through the gateway, when chat is requested, then it reaches the Qwen socket.

### User Story 3 - Decide replacement using measurements (P2)
As the operator I can reproduce the build and collect correctness, first-token latency and decode throughput evidence on the target GPU.
**Independent test**: Same host, GGUF hashes, prompt, greedy settings, context and output bound; repeated runs with recorded hardware/runtime identity.

## Requirements
- FR-001: RustingStrata is a pinned Rust dependency; no custom inference implementation, FFI shim or upstream HTTP server is introduced.
- FR-002: Retain Agentix's pinned Qwen tokenizer/template, request validation and API boundaries.
- FR-003: A persistent inference thread owns mapped weights, model and GPU state; reset state for every request and serialize admission.
- FR-004: Explicit CPU reference and CUDA/Vulkan choices; default CUDA for the target deployment. Errors are actionable, not implicit fallbacks.
- FR-005: Package runtime driver/NVRTC discovery with Nix; preserve upstream license and required attribution. User accepted this license on 2026-11-11.
- FR-006: Small deterministic GGUF fixture tests are reproducible without downloading the production model. Production GPU acceptance remains a separate explicit gate.
- FR-007: Preserve old feature 021 worktree as the comparison baseline. Its no-library/custom-runtime constraints are superseded for feature 033 by this request.

## Edge Cases
Missing/truncated GGUF shards; unsupported quantization; missing driver/NVRTC; context overflow; empty prompts; tokenizer/model mismatch; cancellation during prefill/decode; engine thread failure; repeated requests; SSE decoding across incomplete UTF-8; busy worker; malformed tool output.

## Success Criteria
- SC-001: Nix packages build, focused tests/Clippy/formatting and workspace tests pass before merge.
- SC-002: Small real-model fixture returns deterministic finite logits and consistent reset output through the library and HTTP worker.
- SC-003: Target Qwen3.8 answers basic text and tools coherently, supports streaming and cancellation, and completes three repeated 128-token measurements with artifact/hardware identity recorded.
- SC-004: Record actual speed and limitations; do not infer the 100–140 tokens/s goal from upstream benchmarks. Decide retirement of custom execution only after target acceptance.

## Assumptions
The original deployment target is kursk (RTX 3090, 128 GB RAM); no SSH access is assumed. On 2026-11-12 the owner supplied GPU access through a restarted jail on an RTX 5090 Laptop host. Physical library/production-model acceptance is recorded for that available host; RTX 3090 deployment and a same-host comparison remain separate. Existing master is the base; feature 021 is read-only historical reference. No deployment, push or merge is implied.
