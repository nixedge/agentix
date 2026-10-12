# Strata source study

**Revision**: `7695859aa13afb248bdb4d36f9b2e95707db079d` (CMake version 0.1.43)
**Date**: 2026-10-12
**Status**: Source study complete for the mechanisms below. No engine build or target-host benchmark performed.

## Findings

Strata's advantage is a coordinated CPU/GPU inference implementation specialized for Qwen3.8-Flash-Next, not a general setting that lets any large GGUF run quickly. The model's dense operations run on GPU; frequently used experts occupy a VRAM cache; other experts are computed on CPU or transferred for GPU execution. Dedicated prefill and speculative verification paths exploit different workload shapes. Its large n-gram table can remain on disk behind a bounded row cache.

The selected direction is an independently implemented Qwen Flash Next backend in agentix. Strata is a research and optional benchmark reference; no Strata source, kernels, executable or preparation scripts may become an implementation dependency. Original Qwen references and independently sourced compute primitives must establish the implementation contract. See [decision.md](decision.md). The source references below document observed behavior, not code to port.

For this host, prioritize full-expert IQ3_S and IQ2_XS, with Coder IQ1_M as a smaller comparison. The user has enough nominal system memory to investigate full variants; this is a feasibility estimate, not a verified allocation or performance result. See [hardware.md](hardware.md).

## Implementation evidence

All paths below refer to the pinned [Strata tree](https://github.com/Niko1221/Strata/tree/7695859aa13afb248bdb4d36f9b2e95707db079d).

| Mechanism | Inspected implementation | Consequence for agentix |
|---|---|---|
| Architecture-specific loading | `include/strata/artifact/gguf_reader.hpp:588`, `check_architecture` requires `general.architecture == qwen4exp` and validates geometry; `src/core/native_dense.cpp:214` calls it | GGUF format support is insufficient. Reject unsupported architecture/geometry early. |
| Expert residency | `include/strata/core/expert_cache.hpp`, residency table maps layer/expert to slot; `src/core/expert_cache.cpp:670`, `ExpertCache::fill_slot` uploads bytes with `cudaMemcpyAsync` | Per-expert placement is materially different from the existing agentix GPU-layer knob. |
| Concurrent GPU/CPU/PCIe work | `src/core/expert_source.cpp:3665`, `expert_pool_dispatch_multi` deduplicates routed experts, classifies VRAM hits and PCIe misses, publishes the GPU plan before CPU work; `src/core/verify.cpp:2402`, `Verifier::fetch_dma` performs asynchronous transfers and signals completion | Design scheduling and synchronization together independently; isolated arithmetic improvements do not establish pipeline throughput. |
| Native CPU quantized computation | `CMakeLists.txt:1205` and `src/kernels/cpu/native_expert.cpp`; ggml CPU support plus dedicated AVX2/AVX-512 source files | CPU ISA, RAM bandwidth, thread count, and PCIe bandwidth all matter. More cores alone do not imply proportional speed. |
| Batched prompt processing | `src/prefill/prefill.cpp`, staging ring and stream-all threshold near lines 106–145, grouped MMQ execution near 3736–3778; `src/program/generate.cpp` prefill cache loans | Prefill needs separate measurement; cache capacity temporarily funds prompt buffers. Large decode tok/s says little about TTFT. |
| Disk-backed n-gram reads | `src/ngram/ple_reader.cpp`, `RowCache`, I/O worker, `DirectFile::submit`, cache lookup near line 442 | A roughly 28.8 GB table does not have to occupy VRAM. Its access pattern is unlike repeatedly streaming whole expert matrices. Account for row cache and disk latency separately. |
| Speculation | `src/spec/controller.cpp`, `Controller::choose` estimates MTP/lookup draft value; `src/core/verify.cpp`, `Verifier::run` and `commit`; `tools/mtp_fetch.py:32` pins draft weights | MTP is an additional artifact/preparation dependency. Count accepted output tokens, not drafted tokens, in throughput. |
| Resident process and cancellation | `serve/server.py:671` launches `strata --serve`; `_control` near 1041 sends STOP and drains responses; `src/program/generate.cpp:9051` emits READY, near 9997 parses GEN/GENI, near 12193 emits DONE statistics | Historical interface evidence only; a Strata adapter is excluded. Design agentix-owned readiness, cancellation and failure handling against agentix contracts. |
| Tokenization and chat semantics | `tools/strata_tokenizer.py`, byte-level BPE with qwen35 pre-tokenization; `serve/server.py:6276` selects the model chat template | Reusing agentix's Qwen3 Coder formatting blindly would be incorrect. Match actual token IDs, special-token handling and rendered templates. |

### Output-changing optimization that must be isolated

`src/core/expert_source.cpp:3691` implements `STRATA_ROUTE_TAIL_SKIP`: some nonresident experts whose ranks remain below a threshold across a verification window contribute zero, without renormalizing the remaining weights. `src/program/generate.cpp:8203` enables rank 7 by default under eligible CUDA configurations, except conditions such as nearly complete residency, batching, or peer GPUs.

This is an approximation, not just a faster way of calculating identical model output. Use `STRATA_ROUTE_TAIL_SKIP=0` in the first correctness/performance reference, then evaluate the upstream default separately. Do not attribute its speed gain to an exact kernel improvement. For repeatability, also record adaptive placement, prefix cache, PCIe split, speculation and all runtime environment flags. Disabling tail skipping alone does not guarantee bit-identical output across hardware or engines.

### Claims not established by this study

- Upstream's RTX 3090 speed figures in `docs/DETAILS.md` are explicitly estimates using other hardware. No 9950X/3090 throughput has been established here.
- Published quantization/pruning quality results are upstream reports, not our ten-task evaluation.
- A successful CMake configuration, Nix build, driver compatibility, and runtime memory footprint remain untested.
- A stable public C ABI was not established. CMake provides internal static C++ libraries and an executable, which is not equivalent to a stable embedding API.

## Agentix boundary verification

Base `fd2202890a95e1deb9747ccd8ad39be482f526f8` already has a worker process: `agentix-daemon/src/gateway/mod.rs` sends local chat requests to `llama_socket`, and `gateway/proxy.rs::forward` streams HTTP over `UnixStream`. `agentix-llama/src/main.rs` assembles `InferEngine` and `agentix_llama::LlamaCppBackend` in that worker. The initial library-only framing was incomplete.

`agentix-infer/src/backend/mod.rs` defines the inference abstraction. `agentix-llama/src/lib.rs` exposes GPU-layer/context controls, not Strata's expert-tier scheduler. The lockfile pins llama-cpp-2 and llama-cpp-sys-2 at 0.1.154; Flash Next support in that exact dependency was not established, and must not be assumed.

Branch 020 (`0a70143265f7bb7eb83211cdd7caf11c4e7d7ced`) adds typed tool completion/cancellation, coding readiness, context limits, model identity and operation lifecycle. Its contracts should be the reference for integration. No sibling branch was merged or modified.

## Historical DeepSeek assessment — outside current scope

A Strata worker does not add DeepSeek support: its loader rejects non-qwen4exp architectures. The maintainer independently confirms that DeepSeek V4 Flash would need new loading/packing, kernels and drafting support and is not currently planned: [issue 479](https://github.com/Niko1221/Strata/issues/479).

DeepSeek's [model card](https://huggingface.co/deepseek-ai/DeepSeek-V4-Flash) lists 284B total / 13B active parameters. The idealized raw weight cost is 106.5 GB at 3 bits or 142 GB at 4 bits, excluding metadata, mixed precision, auxiliary weights and runtime state. Those are lower-bound arithmetic estimates, not actual file sizes or fit guarantees. A low-bit backend is worth a separate feasibility study; practical speed and usable context on this host are unknown. Keep backend selection and memory admission independent of Qwen geometry, but do not enlarge this implementation into a DeepSeek engine port.

## Retrieval and validation record

MCP source discovery was attempted first. Two revision-pinned ingestion attempts cloned Strata but failed with HTTP 500 from `/api/embed`. Small one-string, two-string-array and empty-string-array probes succeeded; the failed batch's exact cause is unresolved. In the checked-out indexer, batches contain 16 chunks with up to four concurrent flushes; HTTP error bodies are discarded. These are diagnostic leads only.

The user explicitly authorized direct pinned-source reads after the failures. Source was cloned in the normal bare-plus-worktrees layout at `/home/sam/home/Strata/research-7695859`, verified detached at the pin. Index completeness is not claimed. Upstream installers and Python helpers were inspected but not executed or adapted. No engine tests or GPU benchmarks were run.

The current session host is `iviron` (Ryzen AI 9 HX 370), not the target host. Target SSH identity/access remains unanswered. This blocks host inventory and measurements, not the source study.

## Updated design direction

The user selected Qwen Flash Next for an agentix-owned implementation and explicitly prohibited direct use of Strata code. The revised [spec.md](spec.md), [plan.md](plan.md) and [tasks.md](tasks.md) govern this work. The DeepSeek discussion and [deepseek-design.md](deepseek-design.md) are historical research only. Implementation requires independent model/operator contracts before optimization work.
