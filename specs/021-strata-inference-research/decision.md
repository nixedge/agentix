# Decision: independently implement Qwen Flash Next in agentix

**Updated**: 2026-10-13
**Status**: Direction selected by user. A stateful CPU GGUF decoder and isolated Unix-socket worker now execute greedy text generation, with exact-model daemon routing. An opt-in CUDA path now dispatches only the quantized vocabulary projection; it streams weights each token and remains unverified on a GPU. Independent output parity, full CUDA execution, optimization stages and target-host acceptance remain pending.

## Selected direction

Build agentix-owned Qwen3.8-Flash-Next execution with independently written artifact handling, memory admission, expert scheduling/caching, CPU/GPU overlap, prefill, PLE lookup caching and eventually speculative decoding. Use original Qwen references as the authority for model mathematics and independently sourced compute libraries where suitable. Strata supplies research evidence and an optional external benchmark only.

Do not link, wrap, vendor, copy, translate or adapt Strata source or kernels, and do not depend on its setup scripts or generated runtime packs. Existing source study is acknowledged; this is independent implementation, not a claim that contributors have never seen Strata code. Record provenance for implementation choices and dependencies.

| Approach | Disposition |
|---|---|
| Strata library or executable wrapper | Excluded by user direction |
| Copy/port Strata kernels or runtime | Excluded by user direction |
| Existing llama backend | Retain as available backend and comparison; independently sourced primitives may be considered after support/provenance review |
| Agentix-owned Qwen runtime | Selected direction; staged correctness and performance gates in `plan.md` |
| Native DeepSeek engine | Superseded and out of scope; historical proposal retained with an explicit status |

## Verified integration context

At base `fd2202890a95e1deb9747ccd8ad39be482f526f8`, the daemon proxies local inference over a Unix socket to `agentix-llama`; `agentix-daemon/src/gateway/proxy.rs::forward` streams response bodies. The Qwen path now follows that process boundary through `agentix-qwen-worker`; inference libraries remain network-free and the daemon remains the public listener. MiniJinja remains the selected chat-template renderer and is also used by Agentix indexing.

At branch 020 revision `0a70143265f7bb7eb83211cdd7caf11c4e7d7ced`, inspected contracts add typed tool completion/cancellation, coding capability, context limits, busy state, artifact identity, readiness and model-operation phases. Preserve these semantics when reconciling the implementation base; no sibling branch was merged. See `research.md` for the prior source inspection.

## Performance and model choice

Full-model IQ3_S is the provisional quality-oriented candidate; IQ2_XS is the provisional capacity/performance comparison. Independent format/kernel support, exact artifact verification and actual memory budgets must confirm them. Coder pruning is a model change, not an engine optimization.

The 90% same-model Strata throughput criterion is a target to evaluate on the actual host. Published measurements on other CPUs and upstream estimates do not predict the new engine's throughput. First establish correct ordinary decoding, then retain optimizations only with measured benefit and correctness evidence. Speculative throughput counts accepted tokens only. Compare Strata's tail-skipping-disabled reference separately from its approximate defaults.

## Current evidence and blockers

- Completed: pinned Strata mechanism study, pinned Qwen configuration/tokenizer/template identities, direct inspection of original operator source and safetensors namespaces, initial agentix boundary inspection, revised feature specification and worker/daemon integration contract.
- Source indexing failed; user-authorized pinned direct reads completed the source inspection. No index completeness is claimed.
- Selected `llama-cpp-sys-2` 0.1.154 as the isolated GGUF/GGML native boundary. The CPU package contains a metadata-only GGUF inspector; synthetic one-tensor and truncated-file parser tests pass. MiniJinja template fixtures pass with the two-method compatibility callback.
- Pinned official llama.cpp Qwen4-Exp converter source confirms PLE shard concatenation order and row-width validation. Header-only reads verify 128 ordered source PLE shards of BF16 `[2500012,160]`, whose 320,001,536 physical rows match the padded GGUF extent and include 90 rows beyond the logical n-gram table. `rows::SafeTensorFile` now opens bounded BF16 row readers, and `ple::ShardedEmbeddingRows` composes them without materializing the table. One real 320-byte source row was read through that API and compared with the published GGUF Q4_0 row within a 0.003 maximum absolute error; the full 128-shard payload set remains unread. The ggml-org preset's second shard stores PLE as Q4_0 `[160,320001536]`; the separate IQ3_S candidate stores it as IQ4_NL with the same extents. `agentix-qwen-sys` has bounded readers for all six storage types in the ggml-org inventory (F32/F16/BF16/Q4_0/IQ4_NL/Q8_0) and both candidate expert formats IQ2_XS/IQ3_S. IQ2_XS/IQ3_S use the pinned native GGML CPU `to_float` callback with validated 256-value block geometry and bounded rows; tests cover synthetic blocks. A pinned IQ3_S expert row from a rank-three `[2560,640,512]` tensor was also decoded, and its header, raw bytes and output are recorded in `evidence/ista_iq3s_*`; there is no independent numeric oracle comparison for this row. Other quantized decoders and F16/BF16 conversion match pinned native callbacks; one published Q4_0 row and one published IQ4_NL row match within `1e-7`. The native row adapters remain in `agentix-qwen-sys`; `agentix-qwen` remains native-free. An Agentix source-name map covers all 1,294 text index entries and produces the same 1,224 unique names as the pinned config schema and published header. CPU transform primitives cover mapped split/reorder/norm/squeeze operations but are not yet integrated with payload loading. Typed PLE hash-array reading and GGUF UINT64 metadata rewriting are implemented with synthetic tests that verify the arrays and preserve tensor payload bytes; full source-array parity and model-wide serialization remain open.
- Parsed the ggml-org second-shard metadata directory into a pinned 1,224-row name/type/shape/byte-size inventory. It contains 292 IQ4_NL, 518 Q8_0, 388 F32, 24 BF16, one F16, and the Q4_0 PLE tensor. The full first shard is hash-verified and has zero tensors; the sparse second-shard header plus first file pass Agentix's complete split-index/name-uniqueness check. The second payload remains unverified. The checked header crosswalk and geometry test verify all text-source shapes against their transformed GGUF targets. A direct 128-value BF16-to-F32 norm vector matches exactly, and the 48-value A_log source vector matches published GGUF within tolerance after -exp and reorder; broader source payload values remain unverified.
- Pending: verifying source values and quantization, exercising the index-driven PLE opener on the complete 128-tensor/33-file checkpoint, wiring the source map and existing CPU transforms into source checkpoint conversion, PLE source-array parity and model-wide serialization, and full-model reference vectors. A compact synthetic four-block GGUF fixture now has a pinned generated hash and exact zero-logit greedy output; it does not establish nonzero model correctness. The IQ2_XS/IQ3_S candidate formats are separate artifact research and are not claimed for the inspected ggml-org artifact. Analytical CPU vectors and the two-token IQ3_S sampler do not establish original-checkpoint correctness.
- Pending: model weight verification, target driver/free-memory/bandwidth/PCIe/storage inventory and host access. This session runs on a different machine (RTX 5090 Max-Q, without an available NVIDIA management utility), so the requested RTX 3090 measurements are unavailable here.
- Pending: CUDA device execution, full-model correctness and target-host experiments. No throughput, fit or coding capability is advertised by the CPU foundations.

## Constitution review — current worker and crate boundary

The current `agentix-qwen` and `agentix-qwen-sys` split satisfies the relevant
library-first and isolation requirements: the former has no network surface or
native dependency; the latter owns the pinned GGUF/GGML binding and checked
metadata/row utilities. Both are independently testable and have Nix package
attributes. The generic inference/router crates do not depend on them, and the
daemon remains the only public HTTP listener. No Strata dependency is present in
the Qwen crate trees. Follow-up review covers the worker, final normal dependency
trees, CUDA feature packaging, and daemon assembly. `agentix-qwen-worker` owns
the public-to-private Unix-socket boundary and calls the same stateful decoder;
the daemon routes only the exact configured model ID to that socket, while
existing local models retain the llama path. CUDA remains an opt-in feature of
`agentix-qwen-sys` forwarded by the worker feature and Nix package. Its only
decoder dispatch is the final quantized vocabulary projection; device selection
is checked before the worker binds its readiness socket. Generic inference and
router crates do not gain a Qwen or native dependency, and the normal worker and
daemon dependency trees contain no Strata dependency. Nix CUDA compilation
derives architecture targets from `pkgs.config.cudaCapabilities`; runtime still
requires the matching host driver and has not been verified on the target GPU.

The worker currently creates request-scoped decoder state, limits execution to
one generation at a time, performs inference on the blocking pool, and bounds
SSE buffering. Tool calls and reasoning output remain explicitly unsupported;
the CUDA projection uploads streamed weight rows per token and is a correctness
probe, not a throughput feature. Thus the packaging and boundary review is
complete, while target-host execution, CUDA parity, model-backed lifecycle
conformance, and performance acceptance remain separate open requirements.
The workspace test suite passes, and the documented workspace format and
Clippy commands now pass after cleanup recorded in `implementation-log.md`. The
stronger Clippy run with `--all-targets` still reports existing panic-style
calls in test targets; that is outside the documented command and is recorded
separately.

## Acceptance and rollback

The spec and task coverage table define all FR/SC gates. Build/run without Strata, retain the existing selectable backend, fail explicitly on unsupported capabilities or memory pressure, and do not silently substitute models or cloud execution. Final release assessment must map every gate to evidence or a named blocker; design documentation alone is not a passing implementation result.

## Requirement coverage snapshot — 2026-10-13

| Requirement | Current evidence | Status and remaining gate |
|---|---|---|
| FR-001 | Pinned Qwen config/tokenizer/template hashes, candidate manifests, full IQ3_S shard SHA256 verification, 1,224-tensor schema, one-row source-to-target PLE parity, and sampled BF16-to-IQ3_S comparisons for `token_embd.weight` and `output.weight` | Partial: full source/model transform-map verification and all auxiliary artifact identities remain open. |
| FR-002 | Independent Agentix Rust references and `llama-cpp-sys-2` provenance; no Strata dependency in selected Qwen crate trees | Partial: final runtime/dependency and binary audit remains open. |
| FR-003 | Analytical CPU vectors for residual, MoE, GatedDeltaNet, PLE and sequential QSA; bounded dense/quantized readers; finite real-row samples for all 13 IQ3_S storage types; one source/target PLE row; sampled norm/A_log/dt_bias parity; and sampled source BF16-to-IQ3_S embedding/output row metrics | Partial: tensor-map/transforms are not integrated with a full model graph; broad source parity and full-model unsupported-parameter rejection are missing. |
| FR-004 | Checked host/device budget arithmetic, prompt context admission, bounded encoded-row cache | Partial: target hardware inventory and full runtime allocation accounting/admission are missing. |
| FR-005 | No implementation evidence | Open: bounded expert residency, CPU experts, asynchronous transfer ownership/cancellation and parity tests. |
| FR-006 | Bounded row store, source SafeTensors BF16 and Q4_0/IQ4_NL GGUF readers implementing the PLE row interface, ordered sharding, and one source/target parity row | Partial: all source shards are not yet opened in one run; production PLE cache metrics and separate prefill/decode scheduling remain unimplemented. |
| FR-007 | N-gram history snapshots and transactional recurrent/PLE/QSA state foundations | Open: prefix cache and MTP/speculative decoding are not implemented or tested end to end. |
| FR-008 | Detailed lifecycle, event, cancellation, backpressure and capability contract; isolated worker and exact-model daemon route; bounded SSE and typed unavailable/unsupported errors | Partial: real-worker inference tests, cancellation, load/unload and full API conformance remain. |
| FR-009 | CPU/native crates and Nix attributes; worker and daemon package builds; native dependency isolated from generic inference/router; exact Qwen dispatch leaves other local models on the llama socket | Partial: the CUDA sys package compiles, but its Nix check phase cannot load `libcuda.so.1` in this sandbox; CUDA device execution and jail task path remain. |
| FR-010 | Frozen measurement fields and protocol in `benchmarks.md` | Open: no executable engine/host runs or raw result records. |
| FR-011 | Comparison rules and approximation labels documented | Open: no matched-model or different-model measurements. |
| FR-012 | Workspace tests, formatter and Clippy pass; Qwen/sys/indexer/daemon/worker Nix builds pass, including CUDA-feature package builds | Partial: the compact fixture, CPU sys tests, focused strict sys Clippy and formatting pass; earlier Qwen, worker, daemon and indexer package builds pass. The latest CUDA-enabled sys Nix derivation compiles but fails its check phase because `libcuda.so.1` is unavailable in the sandbox. Workspace-wide current-state verification, the approved CUDA package strategy, and target-host experiments/device checks remain. |
| SC-001 | Source/dependency provenance recorded; Cargo normal dependency trees and worker/daemon packages build without Strata; native dependency remains isolated to `agentix-qwen-sys` | Partial: target-host runtime and acceptance run remain. |
| SC-002 | Analytical CPU operator/state tests, exact tokenizer and benchmark prompt fixtures, a hash-pinned compact full-decoder fixture, one source-to-published-GGUF PLE row comparison, and sampled norm/A_log/dt_bias vector comparisons pass | Partial: no original-checkpoint full-model reference comparison or cancellation/load/unload integration tests. |
| SC-003 | Protocol specifies 4K/32K/64K runs | Open: RTX 3090 access and executable model are unavailable here; no measurements. |
| SC-004 | Ten coding/tool task statements and acceptance commands are frozen against clean feature baseline `13f2a5cb52ebc801eab154b009b0ab01f19e7a6a`; task-file SHA-256 is recorded in `benchmarks.md` | Open: clean-checkout evaluator, per-task run records, and baseline results are absent. |
| SC-005 | 90%/110% thresholds documented as targets | Open: no engine or same-host Strata result. |
| SC-006 | Workspace tests, format and Clippy pass; Qwen/sys/indexer/daemon/worker package compilation, NixOS service evaluation, normal dependency audit and exact-route selectability checks pass | Partial: the latest CUDA-enabled sys Nix derivation fails at test execution without `libcuda.so.1`; target runtime and acceptance evidence remain. |

This is the local evidence map for T017, not a release assessment. Requirements
with partial or open status still need the named artifact, CUDA, target-host,
optimization or benchmark gates before acceptance.

## Follow-up evidence update — 2026-10-13

The provisional IQ3_S quality candidate is now acquired in full at the pinned
revision. Both publisher SHA256 values match; the complete two-shard inventory
passes the Qwen schema check for all 1,224 names and shapes; and one row from
each of the candidate's 13 storage types decodes to finite values. This supports
using IQ3_S as the current validation artifact, but it does not establish
independent weight parity, memory fit, runtime loading, CUDA execution, or
quality/performance. IQ2_XS remains a capacity comparison candidate with only
synthetic format coverage. The acquisition and sample evidence are in
`sources.md` and `evidence/ista_iq3s_candidate_inventory.json`.

## Worker assembly update — 2026-10-13

The CPU path now opens the full IQ3_S GGUF store through the reusable 48-block
stateful decoder. `agentix-qwen-worker` validates ordered shard hashes, tensor
schema/layer geometry and tokenizer vocabulary before binding its Unix socket;
it exposes readiness, model listing, greedy text completions and bounded SSE.
`agentix-daemon` routes only the configured exact Qwen model ID to this socket
and returns an explicit unavailable response instead of falling back. A NixOS
service option and worker package are present. This is executable assembly, not
full-model acceptance: the checked real-artifact run is a two-token sampler,
not a rendered chat prompt; no independent full-model output oracle, CUDA
execution, lifecycle breadth or target-host acceptance is recorded yet. See
`tasks.md` T018–T021 for the remaining gates.

The router remains unchanged for existing local models: all model families it
classifies as `RouteTarget::Local` still use the llama Unix socket, while only
the exact configured Qwen ID is intercepted for the Qwen socket. The worker
and daemon Nix package boundaries remain separate, and `agentix-qwen` remains
native-free. Normal Cargo dependency-tree review found no Strata crate;
`llama-cpp-sys-2` appears only below `agentix-qwen-sys`. Both daemon and
CUDA-enabled worker Nix packages build. CUDA device execution and target-host
runtime acceptance are still unverified.
