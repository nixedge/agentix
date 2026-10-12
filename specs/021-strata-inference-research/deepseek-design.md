# Proposed direction: native DeepSeek V4 Flash inference in agentix

**Date**: 2026-10-12
**Status**: Superseded historical feasibility proposal. The user selected an independently implemented Qwen Flash Next backend instead; see [decision.md](decision.md). None of this document's proposed work or tasks governs the current implementation. No DeepSeek engine was implemented or benchmarked here.

## Objective

Use Strata as a source of execution ideas while building an agentix-owned backend specifically for DeepSeek V4 Flash on the RTX 3090 / Ryzen 9 9950X / 128 GB RAM host. Keep Rust responsible for model lifecycle, memory admission, scheduling and agentix integration; isolate GPU/native kernels behind the existing library boundaries. Owning the runtime does not require rewriting every arithmetic kernel.

## What transfers from Strata

- Profile-guided expert residency, followed by bounded adaptive replacement.
- Concurrent GPU-resident expert work, CPU expert work, and asynchronous PCIe transfers.
- Separate execution strategies for long prompt batches and low-batch decoding.
- Memory budgets that reserve context/scratch capacity before filling the expert cache.
- A staged speculative path after ordinary decoding is validated.
- Instrumentation for cache hit rate, bytes read/transferred, prompt processing, accepted output, and end-to-end latency.

These are design patterns, not drop-in Qwen kernels. Strata's n-gram/PLE disk tier is specifically useful for its Qwen architecture; do not assume an analogous large lookup table exists in DeepSeek.

## DeepSeek-specific requirements

The official [DeepSeek V4 Flash model card](https://huggingface.co/deepseek-ai/DeepSeek-V4-Flash) describes 284B total / 13B active parameters, compressed sparse attention (CSA), heavily compressed attention (HCA), and manifold-constrained hyper-connections (mHC). It provides dedicated message encoding/parsing code rather than a Jinja chat template. Exact tensors, router equations, attention-state layout and draft structure must be pinned and source-verified before implementation.

Correction: the earlier description of its attention as simply MLA, taken from Strata's issue reply, was incomplete. Use DeepSeek's own implementation/report as the design authority. In particular, the model card's 10% KV-cache claim compares V4-Pro with V3.2 at 1M context; it is not a Flash-on-3090 memory measurement.

Implement or reuse validated components for:

1. Exact checkpoint/quantized tensor loading and dequantization.
2. DeepSeek routing and shared/routed expert semantics.
3. CSA/HCA attention, compression/indexing and cache updates.
4. mHC and the precise normalization/residual operations.
5. Tokenizer, message encoding, reasoning modes, tool encoding and output parsing.
6. Draft/verification state and acceptance semantics, only after non-speculative correctness.

## Hardware feasibility

Raw arithmetic for 284B scalar weights gives 106.5 GB at 3 bits and 142 GB at 4 bits. Actual artifacts include scales, mixed-precision tensors and potentially auxiliary weights, so these are lower bounds. Official metadata reports mixed FP4/FP8 storage; this is not proof that the 3090 can execute the format efficiently or that the original checkpoint fits.

RAM and VRAM are separate pools. GPU-resident bytes must not also be fully duplicated in an anonymous host arena if the design depends on their combined capacity. Account for file-backed pages, pinned staging, quantization buffers, attention state, runtime and other services. The starting question is which verified low-bit or mixed-precision artifact fits with headroom, not whether 128+24 exceeds a nominal weight-size estimate.

Only a fraction of DeepSeek's experts can occupy 24 GB VRAM after dense/state/scratch reservations. Measure routing locality; expert-cache capacity fraction is not the same as runtime hit rate. CPU bandwidth and PCIe misses may dominate decode. Model intelligence, fitting in memory and interactive speed are separate acceptance gates.

## Proposed architecture

- Pure Rust scheduling/lifecycle library integrated with `agentix-infer`; no public listener.
- DeepSeek-specific backend with explicit capability and architecture checks.
- Quantized CPU expert execution and CUDA compute/copy streams behind an isolated native boundary.
- Bounded residency manager owning separate RAM/VRAM budgets and measuring transfer/miss costs.
- Existing agentix worker/daemon separation retained for process isolation; no dependency on the Strata executable or Python server.
- Reuse suitable kernel libraries where verified. Rust ownership of the system is compatible with CUDA/C++ primitives.

## Staged decision gates

1. Pin the exact Flash release (including whether the user wants original or 0731), reference implementation, quantized artifact and tokenizer/encoding sources. Verify target host access and resource inventory.
2. Build a memory ledger and measure host RAM bandwidth, PCIe transfers and representative expert kernels. Estimate latency from measured bytes and costs before committing to a full runtime.
3. Establish numerical reference checks for routing, expert outputs, attention updates and mHC on small deterministic fixtures. No default expert skipping.
4. Produce correct non-speculative short-context decoding with a simple CPU/GPU split.
5. Add expert caching, transfer overlap and dedicated prefill; ablate each optimization under fixed model/settings.
6. Add optional speculation with correctness checks and accepted-token metrics.
7. Integrate agentix tools/readiness/cancellation and evaluate realistic coding tasks against existing supported runtimes.

Initial context target should be deliberately bounded, such as 8K–32K, subject to the memory ledger. It is not a promise to expose the model's nominal 1M context. Final throughput targets must follow the microbenchmarks; no 50–100 tok/s promise is justified yet.

## Scope impact

Feature 021 currently specifies a Strata study and integration decision. A native DeepSeek engine changes its principal model, correctness oracle, benchmarks and deliverables. Before code, revise spec/plan/tasks around the above feasibility study and implementation gates, retaining the completed Strata report as reference evidence. Do not silently mark Qwen/Strata benchmark tasks complete or treat a wrapper plan as a native-runtime design.

## Rough performance planning range

For one request on the user's 3090/9950X, with a feasible low-bit artifact resident in RAM/VRAM and no sustained swap, use **10–20 generated tokens/s** as a provisional DeepSeek planning range. This is an inference from nearby community measurements of existing engines, not a prediction validated for a new agentix runtime. **20–30 tok/s is a stretch target** for caching/overlap/speculation, not an assumed improvement. A first implementation could be substantially slower.

Relevant first-hand reports checked 2026-10-12:

- [3090 / 5950X / 128 GB DDR4](https://www.reddit.com/r/LocalLLaMA/comments/1uz76sp/deepseek_v4_flash_iq3_xxsas_iq2_s_bench_mainline/): roughly 11–13.5 tok/s for IQ3_XXS-AS/IQ2_S and about 250–280 prompt tok/s. Different CPU/RAM and workloads; not our benchmark.
- [3090 Ti / 9950X](https://www.reddit.com/r/DeepSeek/comments/1vf9t2e/running_deepseekv4flash_0731_on_a_single_rtx_3090/): headline 25.8 tok/s was corrected by the author to approximately 15 tok/s after addressing looping. Do not use the headline as a quality-valid result.
- [Strata 0.1.43 / single 3090 / i7-13700K](https://github.com/Niko1221/Strata/issues/1988): Qwen UD-IQ4_XS medians 57.9–74.7 decode tok/s over the listed workloads, approximately 1,400–2,100 prompt tok/s on 4K–32K fresh prompts. Short output caps on prompt tests; speed-only, no correctness check.
- [Strata estimates](https://github.com/Niko1221/Strata/blob/7695859aa13afb248bdb4d36f9b2e95707db079d/bench/results/2026-09-24-final/estimates.md): 3090 Q2_0/IQ2_XS around 100–140 decode tok/s at shorter/moderate contexts, explicitly extrapolated rather than measured on that card.

This suggests Qwen through Strata is likely several times faster in raw decode on this host. That comparison changes model, precision, runtime, speculation and sometimes approximation policy; it is not a measure of relative engine efficiency or coding quality. RAM speed/configuration and actual expert hit rates remain unknown. All time estimates exclude prompt ingestion, model loading, tool execution and additional reasoning tokens.
