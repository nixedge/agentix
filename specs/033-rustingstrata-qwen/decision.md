# Decision: adopt RustingStrata for Qwen

2026-11-12. The owner authorized direct library replacement and retirement of
custom Strata emulation if testing worked. Physical validation on the available
RTX 5090 Laptop GPU passes: CUDA/CPU fixture parity, production GGUF loading,
coherent text, streaming equality, function-call/result round trip, busy and
disconnect recovery, and a 2,234-token request followed by a clean short request.
The repeated 128-token benchmark records a 55.25 tokens/s median decode rate.

Use the pinned RustingStrata Rust library as the Qwen execution engine. Further
custom GGML execution, expert scheduling and Strata algorithm-porting work from
021 is superseded. Keep the old worktree and measurements as historical evidence;
no reset, deletion or migration of that worktree is authorized or necessary.

This worktree already has only the direct library engine; no optional legacy
execution path is retained. Agentix continues to own its tokenizer/template,
protocol validation, worker/gateway integration and cancellation boundaries.
Upstream remains unmodified; its accepted license and required notices are shipped.

Evidence and exact qualifications: [physical results](results/2026-11-12/summary.md).
These tests use different hardware from the old RTX 3090 deployment and do not
establish a same-host speedup or achieve the 100–140 tokens/s goal. Deploying or
merging this branch is a separate action and was not performed here.
