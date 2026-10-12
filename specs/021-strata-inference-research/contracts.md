# Qwen backend integration contracts

**Status**: Implementation-stage contract draft, 2026-10-12. No executable backend yet.
**Verified base**: `fd2202890a95e1deb9747ccd8ad39be482f526f8`.
**Compatibility reference**: branch 020 `0a70143265f7bb7eb83211cdd7caf11c4e7d7ced`.

## Assembly and dependencies

Keep the existing worktree base; do not merge sibling branches implicitly. Model computation belongs in a network-free `agentix-qwen` library. A separate native binding crate is justified only after compute dependency selection. Neither the generic inference crate nor router may depend on CUDA/native code through this backend.

The base daemon uses Unix-socket worker proxying and does not link agentix-infer directly. Therefore prefer an agentix-owned worker assembly consistent with that boundary when wiring the completed backend. Final packaging/transport placement must be verified against the current worker before implementation. It must not start or depend on Strata. Existing llama selection remains available.

## Identity and loading

A model identity covers verified shard hashes, original configuration revision/hash, quantization, tokenizer/template hashes, optional MTP artifact hashes, and runtime packing version. Merely recognizing GGUF does not imply architecture support. Unsupported tensor types, geometry or missing shards fail before readiness.

Load progression is explicit: unloaded → validating → admitting → loading → ready. Failure drops owned resources and returns to unloaded. The public loaded handle is returned only once initialization succeeds. A request cannot race loading or unloading.

## Memory and execution

Host and GPU reservations are separate. Account for weights, context/recurrent state, PLE cache, pinned staging, scratch and service headroom; count file-backed pages once. Reserve context before expert cache. Checked arithmetic must reject overflow. Initial concurrency is one generation per loaded instance; conflicting requests get a busy error rather than sharing mutable state.

Execution states: ready → prefill → decode → draining → ready. Cancellation/error from either execution phase enters draining; all in-flight device work must release buffer ownership before reuse. Unload during a request either waits for cancellation/draining or returns busy. Native work and file reads must not block Tokio worker threads.

## Tokenization and request admission

Use the pinned original tokenizer and chat template. Compute prompt tokens after rendering system, user, assistant, tools and reasoning controls. Reject zero/invalid generation bounds and requests exceeding the admitted context; never silently truncate messages or shrink requested output. Reject unsupported vision/embedding/transcription/grammar capabilities explicitly.

Prefix identity includes model artifacts, rendered token prefix, template, position/state semantics and execution settings that affect state. Cache reuse cannot cross identities or a cancelled/incomplete state boundary.

## Streaming and cancellation

Emit UTF-8-safe text deltas and distinct reasoning events where the API supports them. Tool calls cross the backend boundary only after complete parsing and validation; malformed output yields an explicit error rather than an invented call. Emit exactly one successful terminal event; an error terminates the stream without a later success event. Bound output queues; cancellation and receiver closure interrupt stalled sends and stop scheduling more compute.

The base `LoadedModel` only exposes text completion, tokenization and a single memory estimate. It lacks branch 020's typed tool requests, cancellation, busy state and context/coding capability hooks. Stage integration through additive, backwards-compatible generic contracts after reconciling the 020 definitions; do not register a text-only stub as coding-ready. Branch 020's tool stream also lacks an explicit reasoning variant, so reasoning preservation requires an intentional API addition rather than silently folding it into answer text.

## Validation requirements

Pinned-fixture coverage must include initialization failure, corrupt artifacts, context overflow, busy rejection, cancellation during prefill/decode/backpressure, unload while active, receiver drop, repeated load/unload, UTF-8 fragmentation, malformed tools and exactly-once terminal behavior. Independent operator/state vectors validate math; a small fixture does not establish full-model quality. Host memory and throughput validation remain separate release gates.
