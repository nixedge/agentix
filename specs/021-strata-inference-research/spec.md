# Feature Specification: Agentix-owned Qwen Flash Next inference

**Feature Branch**: `021-strata-inference-research`
**Updated**: 2026-10-20
**Status**: Stateful CPU decoder and hybrid CUDA matrix path implemented; independent full-model parity, target-device execution and optimization acceptance remain incomplete
**Input**: Focus on Qwen Flash Next, engineering the useful Strata features into agentix without using Strata code directly.

## Objective and scope

Run text and coding workloads with Qwen3.8-Flash-Next on the user's RTX 3090 (24 GB VRAM), Ryzen 9 9950X (16 cores), and 128 GB RAM. Implement an agentix-owned execution path using independently written scheduling, placement, artifact handling and integration. Strata is a research and benchmark reference only: no linked library, subprocess dependency, vendored source, translated code, copied kernels or upstream preparation scripts in the delivered engine.

Initial target is the full-expert model, with IQ3_S as the provisional quality-oriented candidate and IQ2_XS as a capacity/performance comparison. Artifact support and memory measurements must validate this choice. The pruned Coder variant is optional comparison evidence, not an interchangeable model. DeepSeek, vision, multi-GPU and concurrent generation are out of scope. Reuse of independently sourced, pinned general-purpose compute libraries is allowed; their provenance must be explicit.

## User Scenarios & Testing

### US1 — Correct local Qwen execution (P1)

As an operator, I can load a verified model and obtain correct streamed text and tool calls through agentix.

**Independent test**: Small pinned fixtures check operators, tokenization and lifecycle; separately, full-model runs compare a pinned independent reference.

1. Given supported model and auxiliary artifacts, when loading, then validate identity, geometry and quantization before allocation and report readiness only after initialization.
2. Given ordinary, reasoning and tool messages, when generating, then tokenization, context accounting, output events and terminal semantics match the declared model contract.
3. Given unsupported artifacts, cancellation or insufficient memory, then return a typed failure or cancellation without leaked allocations or continued generation.

### US2 — Useful throughput within this host's memory (P2)

As an operator, I can use CPU RAM and GPU memory together without sustained swapping, and inspect whether each optimization actually helps.

**Independent test**: Matched runs with each optimization enabled/disabled report accepted output throughput, TTFT, memory and correctness.

1. Given a validated ordinary decoder, when expert caching and overlapped CPU/GPU work are enabled, then results remain within predefined numerical tolerances and memory stays within admitted budgets.
2. Given long prompts or speculative decoding, then prompt processing and accepted generation are measured separately, with all state rolled back correctly after rejected drafts.
3. Given insufficient resources, then admission rejects the request or offers an explicitly selected smaller context; it never silently changes the model or skips routed experts.

### US3 — Maintainable independent implementation (P3)

As a maintainer, I can build, test and run the backend without Strata installed or fetched.

**Independent test**: Inspect dependency/provenance records and build the backend in Nix; exercise daemon integration and restore the existing backend through configuration.

1. Given a clean build environment, then pinned dependencies and model fixtures suffice without Strata source, binaries, generated runtime packs or setup scripts.
2. Given an unavailable experimental backend, then the existing backend remains selectable without changing public API contracts or silently changing the chosen model.

## Requirements

- **FR-001**: Pin artifact revisions, hashes, templates, auxiliary weights and supported architecture/quantization; verify acquired bytes before use.
- **FR-002**: Implement independently from model specifications, original model references and independently sourced compute primitives. Record provenance; do not copy or translate Strata implementation code. Existing source research is acknowledged, so this is not a claim of formal clean-room development.
- **FR-003**: Implement correct dense, attention/state, routing, shared/routed expert and n-gram/PLE semantics before optimizations; reject unsupported geometry and tensor types explicitly.
- **FR-004**: Budget host and device allocations separately, including resident weights, file-backed pages, pinned staging, context, scratch, caches and service headroom. Prevent double counting mapped pages; admit context before allocating remaining cache space.
- **FR-005**: Support bounded expert residency, CPU expert execution and asynchronous transfers with explicit synchronization and cancellation-safe buffer ownership.
- **FR-006**: Provide separate prefill and decode scheduling; implement bounded disk-backed PLE lookup, validated against uncached lookup results.
- **FR-007**: Add prefix reuse and optional MTP/lookup speculation only after ordinary decoding passes correctness gates. Invalidate prefix state on model/template/settings changes; validate speculative acceptance and complete state rollback. No expert skipping in the initial implementation.
- **FR-008**: Preserve agentix tokenization, streaming, tools, reasoning, cancellation, backpressure, readiness, busy state, unload and error contracts; declare unsupported capabilities honestly.
- **FR-009**: Keep inference libraries network-free and native dependencies isolated from agentix-infer/router. Use Nix and keep blocking/native work off Tokio workers. Preserve existing backend selection and jailed coding-task execution.
- **FR-010**: Measure cold/warm load, TTFT, prefill, accepted decode, task wall time, memory, faults, cache behavior, transfers and speculative acceptance with fixed settings.
- **FR-011**: Separate matched-model runtime comparisons from model/quantization comparisons. Disable Strata's output-changing tail skipping in the correctness reference; label upstream-default speed runs separately.
- **FR-012**: Maintain small pinned-fixture automated tests, target-host experiments, provenance review and the constitution's build/quality gates.

## Success Criteria

- **SC-001**: Every implementation component has an original model specification or independent dependency reference and provenance review; builds and runtime require no Strata code or binary.
- **SC-002**: Operator and state comparisons pass tolerances fixed before optimized implementation; tokenizer fixtures match token IDs exactly; cancellation, load/unload and error integration tests pass.
- **SC-003**: At least three runs per feasible configuration at 4K and 32K input, plus a budgeted 64K probe, report median/range, memory peaks and swapping. Missing host evidence remains a blocker, not a pass.
- **SC-004**: A fixed suite of at least ten coding/tool tasks covers Rust, Nix, debugging, multi-file edits and tool round trips; record correctness and full completion time.
- **SC-005**: Performance parity is a target: at least 90% of same-host, same-model direct Strata median prefill/decode throughput and TTFT no more than 110% of reference, with no task-suite correctness regression. Match approximation, cache, speculation and output settings; label unavailable comparisons. This is not an achieved result or release guarantee.
- **SC-006**: Required Nix builds, formatting, Clippy and workspace tests pass, dependency boundaries remain intact, and the original backend remains usable. No production-readiness claim before host evidence and unresolved design gates are closed.

## Edge cases and assumptions

Corrupt shards, missing auxiliary tensors, incompatible quantization, disk stalls, allocation pressure, cache thrashing, transfer/cancellation races, partially rejected drafts, stale prefix state and long reasoning output require explicit handling. A small active parameter count does not imply a small resident model. Model licensing and checkpoint provenance remain separate from engine authorship.

The current session is not the target host; Target access is user-operated; driver/CUDA compatibility, free memory, RAM bandwidth, PCIe and storage remain unverified. Start from master `fd2202890a95e1deb9747ccd8ad39be482f526f8`; inspect 019/020 contracts without silently merging sibling branches. This revision selects the engineering direction and staged work, not a completed backend.
