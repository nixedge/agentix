# Qwen backend integration contracts

**Status**: Implementation-stage contract, updated 2026-10-13. A first isolated Unix-socket worker and exact-model daemon route now exist; streaming text is implemented, while full capability and lifecycle conformance remains open.
**Verified base**: `fd2202890a95e1deb9747ccd8ad39be482f526f8`.
**Compatibility reference**: branch 020 `0a70143265f7bb7eb83211cdd7caf11c4e7d7ced`.

## Assembly and dependencies

Keep the existing worktree base; do not merge sibling branches implicitly. Model computation belongs in the network-free `agentix-qwen` library. Put generated/unsafe GGML and GGUF calls behind `agentix-qwen-sys`. Run the CUDA-linked Qwen engine in an `agentix-qwen-worker` process separate from `agentix-llama`, using the daemon's existing Unix-socket HTTP/SSE proxy boundary. Neither the generic inference crate nor router may depend on CUDA/native code through this backend.

The daemon uses Unix-socket worker proxying and does not link `agentix-infer` directly. `AGENTIX_QWEN_SOCKET` defaults to `/run/agentix/qwen.sock`, and `AGENTIX_QWEN_MODEL` selects one exact identifier (default `qwen3.8-flash-next`). The gateway now routes exact chat-completion matches to the Qwen socket and returns an explicit service-unavailable error without falling back to llama/cloud. Unsupported embeddings, responses, and Anthropic-message requests for the exact Qwen ID fail explicitly. Existing llama selection and socket remain unchanged. The worker validates pinned shard hashes, config geometry and tokenizer vocabulary before binding its local socket; it exposes health, model-list, greedy text completion and bounded SSE text streams. Tools, reasoning-channel output, non-greedy sampling, embeddings and multimodal input are rejected. No endpoint may start or depend on Strata.

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

## Typed errors and transport mapping

Keep backend failures typed until the HTTP/SSE boundary. The worker returns one of
`invalid_request`, `unsupported_capability`, `context_length_exceeded`, `busy`,
`model_unavailable`, `invalid_model_artifact`, `resource_exhausted`,
`cancelled`, or `internal`. Map the first three to HTTP 400, `busy` to 503,
unavailable/invalid-artifact/resource failures to 503, and unexpected internal
failures to 500. Cancellation is a control result rather than a successful model
response. These status mappings are adapter behavior and do not require a public
API schema change.

Before response headers are sent, return an OpenAI-compatible JSON error envelope
(`error.message`, `error.type`, `error.code`, and optional `error.param`). Once
SSE headers are sent, emit one error event using the same code/message and close
the stream without a success finish reason or a later terminal success event.
Messages are safe for callers; internal paths, artifact contents and native
error details stay in logs. An overload response may include `Retry-After` only
when the worker can provide a defensible retry interval.

The base `LoadedModel` only exposes text completion, tokenization and a single memory estimate. It lacks branch 020's typed tool requests, cancellation, busy state and context/coding capability hooks. Stage integration through additive, backwards-compatible generic contracts after reconciling the 020 definitions; do not register a text-only stub as coding-ready. Branch 020's tool stream also lacks an explicit reasoning variant, so reasoning preservation requires an intentional API addition rather than silently folding it into answer text.

## Validation requirements

Pinned-fixture coverage must include initialization failure, corrupt artifacts, context overflow, busy rejection, each typed error mapping, cancellation during prefill/decode/backpressure, unload while active, receiver drop, repeated load/unload, UTF-8 fragmentation, malformed tools and exactly-once terminal behavior. Independent operator/state vectors validate math; a small fixture does not establish full-model quality. Host memory and throughput validation remain separate release gates.
