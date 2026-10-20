# Implementation Plan: Agentix-owned Qwen Flash Next inference

**Branch**: `021-strata-inference-research` | **Updated**: 2026-10-20
**Spec**: [spec.md](spec.md)
**Status**: CPU decoder, native GGUF access and isolated worker are implemented. Independent full-model parity, CUDA layer execution, remaining optimization stages and target-host acceptance are open; see `tasks.md` for staged progress.

## Architecture and boundaries

Agentix owns the model loader, execution graph, scheduler, memory accounting, expert placement, caches, tokenizer integration and lifecycle. Use Rust 2021 for orchestration and independently sourced compute primitives or original kernels behind an isolated native boundary. Do not reimplement an entire arithmetic library without evidence that existing primitives are insufficient.

The implemented `agentix-qwen` library owns pure Rust reference execution and model contracts with no networking. `agentix-qwen-sys` isolates native GGUF/GGML access and the optional CUDA boundary. `agentix-qwen-worker` assembles them behind a Unix socket; the daemon dispatches the exact configured Qwen model ID to that socket and retains the existing llama route for other local models. These crates do not replace the generic inference/pool layer or existing llama backend. Broader inference-contract and lifecycle conformance remains tracked in T021.

Target NVIDIA Ampere compute capability 8.6. Do not assume a kernel built for newer GPUs works on the 3090. Independently verify CUDA/compiler/dependency support in Nix. Preserve pure-Rust dependency boundaries for agentix-infer and agentix-router. Large model blobs stay in existing content-addressed storage; no new database.

## Stage 0 — Freeze independent contracts and evidence

Use original Qwen model configuration, reference implementation, tokenizer and quantization format documentation to write `qwen-design.md`: tensor shapes, operator equations, attention/recurrent state, n-gram mapping, shared/routed experts and auxiliary/MTP layout. Strata-specific runtime packs are not an input requirement. Where a GGUF encoding lacks an independent supported path, record the gap rather than translating its Strata loader. Select independently sourced loader/kernel dependencies and document licenses, source revisions, supported tensor types and Ampere capability in `sources.md`.

Write `contracts.md` for identity, admission, readiness, text/reasoning/tool events, cancellation, errors and unload; reconcile sibling API changes without automatic merges. Fix numerical tolerances and reference outputs before optimized kernels. Prepare a Nix-pinned fixture below 50 MB or independently generated reduced-geometry fixture, plus deterministic operator vectors; clearly separate their coverage from full-model correctness.

Complete target-host inventory and budgets in `hardware.md`. Freeze benchmark workloads in `benchmarks.md`. Host access blocks measured feasibility, not independent design work.

## Stage 1 — Correct ordinary inference

Implement validated artifact loading and basic execution, tokenization/templates, sampling and context accounting. Start single-request, ordinary decoding with no speculative work or expert omission. Validate the CPU/reference operators first, then device operators and complete state transitions against the independent reference. Integrate streaming, tools, cancellation, memory admission and unload before throughput claims.

### 2026-10-20 CUDA execution completion

Replace the output-only probe as the worker's normal CUDA path with native
matrix execution throughout every block. Extend the pure `LinearWeightRows`
contract with an optional checked matrix operation; the native adapter dispatches
all dense/quantized projections, including selected expert groups, through GGML.
Keep small normalization, routing, recurrent-state updates and attention/cache
bookkeeping on the CPU initially. This is hybrid inference; log that distinction.
Reuse one backend and a bounded LRU of original encoded device weights across
tokens and requests. Budget actual aligned weight allocations separately from
activation buffers and a device reserve; upload misses in bounded chunks and
synchronize before eviction. Never silently fall back after a device error.
CUDA-enabled workers default to this path and fail startup when the requested
device/budget is unavailable; CPU execution requires an explicit compute choice.
Expose compute mode/cache counters and prompt progress. Cancel disconnected
non-streaming requests at block boundaries as well as streaming requests.
Test native dispatch and stateful full-decoder parity on GGML's CPU backend,
then compile the same engine with CUDA. Supply explicit device parity and
real-chat acceptance commands for the 3090; successful builds do not replace
those execution checks.

## Stage 2 — Residency and overlapping work

Keep appropriate dense/state tensors on GPU, bound expert residency after context/scratch reservations, and execute remaining experts on CPU or via staged GPU transfers according to measured cost. Begin with deterministic placement; add adaptive caching only with hit/miss evidence. Use bounded pinned-memory staging and explicit completion events; do not reuse buffers while any CPU/device operation still owns them. Validate each placement against baseline results and profile CPU bandwidth, transfer volume and GPU idle time.

## Stage 3 — Prefill, PLE and prefix reuse

Implement bounded disk-backed PLE row caching and batched reads, then separate prompt batching and scratch budgets from decode. Add prefix reuse with complete model/template/token/settings identity and state eviction. Verify cached and uncached paths, and exercise disk stalls and pressure. Measure TTFT independently of decode speed.

## Stage 4 — Speculative decoding

Prepare MTP artifacts independently from original model sources. Implement lookup drafting and/or MTP with target-model acceptance appropriate to the sampling mode; commit accepted tokens and roll back all rejected attention/recurrent/position state. Report accepted tokens and draft cost. Keep an ordinary-decoding fallback and enable speculation only when measured useful. No tail-expert skipping in this feature.

## Stage 5 — Host comparison and release gates

Build an optional, separately pinned Strata benchmark reference outside the delivered runtime. Compare identical artifacts when both engines support them; record unsupported formats. Run correctness-oriented Strata with tail skipping disabled, then label upstream defaults as a separate experiment. Use 4K/32K inputs and a budgeted 64K probe, one warmup and three repetitions, at least 256 accepted generated tokens or explicitly report early EOS. Separate cold load, fresh context and reused prefixes. Run ten fixed coding/tool tasks.

The reported 3090 Strata speeds are context, not agentix predictions. SC-005 is a measured comparison target, not an expected initial implementation speed. Produce reproducible Nix commands in `quickstart.md`, small raw records in `results/`, and the final evidence assessment in `decision.md`.

## Constitution check

| Principle | Planned compliance |
|---|---|
| I / V | Network-free backend library; generic API/router stay independent of native code; daemon assembles execution |
| II | Local execution; no implicit cloud fallback |
| III | Pinned Nix dependencies, artifacts and packages; no upstream installers or mutable downloads during builds |
| IV | Existing sandbox constraints and jailed coding-task execution preserved |
| VI | Small pinned fixtures, integration coverage, blocking/native work off Tokio; host experiments separate |
| VII | Preserve harness state transitions, cancellation and budget semantics |
| VIII | fmt, Clippy, workspace tests and Nix packages before merge; provenance and unsafe-boundary review |

No exception adopted. Actual dependency graph, fixture adequacy, package builds and worker assembly must be rechecked before release.

## Complexity and remaining design gates

This is a specialized inference engine, substantially more work than an adapter. Main risks are full operator correctness, quantized kernel coverage, bandwidth/synchronization, speculative state rollback and preserving coding quality. Gates: original Qwen operator contract; independent quantization support; native dependency choice; exact artifact selection and memory fit; reconciled agentix contracts; target access. All remain explicit tasks rather than assumptions hidden behind a throughput target.

## Implementation refinement: independent CPU foundations

The original pinned Transformers operator study is now recorded in `qwen-design.md`. Build dependency-free model-specific CPU reference/state/storage primitives first in `agentix-qwen`, with analytical and integer golden tests. This resolves crate purpose and dependencies for that limited layer. Full-model loader/quantized/native dependency selection, complete model fixture, integration and host gates still apply before registering a usable inference backend. No GPU or coding capability is advertised by the foundation library.
