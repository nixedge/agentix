# Tasks: Agentix-owned Qwen Flash Next inference

**Scope**: Updated research, independent design, staged implementation and verification. CPU reference/storage/tokenizer/identity foundations, schema-validated GGUF row access, a reusable stateful 48-block CPU decoder, an isolated Unix-socket Qwen worker, and exact-model daemon chat routing are implemented. Worker lifecycle, capability breadth, independent model parity, CUDA execution and target-host acceptance remain open.
Paths are relative to this spec directory unless a workspace path is named. Prior research task numbering is retained where useful; revised descriptions reflect the selected direction.

## Established research and direction

- [x] T001 Allocate and verify existing feature/worktree 021 from master.
- [x] T002 Revise `spec.md`, `plan.md`, `decision.md` and checklist for Qwen with no Strata code reuse.
- [x] T003 Record pinned Strata/dependency/model metadata in `sources.md`; record failed indexing and user-authorized direct reads in `research.md`. The selected IQ3_S GGUF shards are now fully downloaded and publisher-hash verified; a full original SafeTensors checkpoint download is not claimed.
- [ ] T004 Complete actual target inventory and separate RAM/VRAM budgets in `hardware.md`. The read-only NixOS inventory command is now recorded; its output from `kursk` and measured RAM bandwidth are still pending.
- [x] T005 Record inspected Strata mechanisms and output-changing approximations in `research.md` as reference evidence, not implementation instructions.
- [ ] T006 [US1] Pin original Qwen reference/configuration/tokenizer/template and quantization definitions; write complete operator/state/tensor contract in `qwen-design.md` and provenance in `sources.md` (depends T003). Partial: pinned converter evidence establishes numeric PLE shard concatenation and common row width; the ggml-org IQ4_NL preset has Q4_0 PLE `[160,320001536]`, while the separate IQ3_S candidate has IQ4_NL PLE with the same extents. The full two-shard IQ3_S candidate now matches both publisher SHA256 values and passes schema validation for all 1,224 tensors. All 1,294 text checkpoint names map to 1,224 GGUF targets; all 128 source PLE headers verify BF16 `[2500012,160]` rows, totaling 320,001,536 physical rows, 90 beyond the 320,001,446 logical n-gram rows. Sampled Q4_0 and IQ4_NL rows match pinned GGML dequantization within `1e-7`; one source BF16 PLE row matches published Q4_0 within `0.003`. Direct BF16 source rows for both `token_embd.weight` and `output.weight` are captured with exact source offsets and hashes, but comparisons against the candidate GGUF are pending. Actual candidate decoding covers all 13 observed storage types, and the artifact sampler now runs one token through all 48 blocks and emits finite logits. Remaining source-value coverage, independent full-model output parity, and target-device correctness remain open.
- [ ] T007 [US3] Select independent loader/compute dependencies and Ampere/Nix strategy; document supported formats, provenance and no-Strata dependency audit in `qwen-design.md` (depends T006). Partial: pinned llama.cpp converter confirms upstream architecture support exists; `llama-cpp-sys-2` 0.1.154 provides generic GGUF primitives but no Qwen4-Exp model loader. Pinned `safetensors` 0.8.0 metadata deserialization now validates source headers and a bounded BF16 file-row reader feeds the pure PLE interface. Exact normal dependency trees in `quickstart.md` confirm that `agentix-qwen` has no native GGML/GGUF dependency and neither Qwen tree includes Strata; the native sys crate owns GGUF parsing and row decoding. GGUF bounded row decoding covers all 13 types in the inspected IQ3_S candidate and 15 types across the inspected artifacts; quantized and F16/BF16 readers are checked against native GGML conversion callbacks, and Q4_0/IQ4_NL also match one hash-verified published row each. One real IQ3_S expert row from the pinned candidate was decoded through the bounded rank-three reader; no independent numeric oracle comparison exists. A CUDA matvec probe and an ignored device-parity test are implemented but cannot execute in this environment because `libcuda.so.1` and NVIDIA device nodes are absent. CUDA-enabled sys package compilation succeeds, but the current Nix derivation fails its check phase without `libcuda.so.1`; full-model tensor mapping/loading, CUDA kernels and target-device execution remain open.

## Design and benchmark gates

Update 2026-10-13: T007's candidate storage coverage now includes all 13 types
observed in the pinned IQ3_S candidate, and a ranged-header two-shard set passes
schema validation (see `evidence/ista_iq3s_candidate_inventory.json`). This
does not close T007's independent loader/compute and Ampere execution review or
T018's complete payload loading/parity requirements.

Follow-up: both full IQ3_S payloads are now downloaded and match their
publisher SHA256 values; complete-file validation matches all 1,224 schema
entries. One real row per each of the candidate's 13 storage types decodes to
finite output. See `evidence/ista_iq3s_full_payload_sample_rows.tsv`. This
does not close full-model tensor mapping/placement, independent weight parity,
CUDA execution, or target-host performance acceptance.

Further implementation: `agentix-qwen-sys::QwenGgufTensorStore` now provides a
split-aware name index with aggregate schema validation and lazy unified dense
or quantized row opening. Synthetic tests cover incomplete-schema rejection
and reader dispatch; the complete IQ3_S set passes the real row sampler through
this API. Full transformed model loading, graph execution and GPU residency are
still open under T018/T019/T020.

Follow-up: `GatedDeltaNetState::step_from_rows` and `process_from_rows` apply
block QKV, gate, beta, decay, and output projections from bounded matrix rows.
They commit convolution/recurrent state only after output projection succeeds.
`agentix-qwen-sys::GatedDeltaNetGgufLayer` now opens the named matrices/vectors,
checks their configured geometry, and provides row-backed step/prefill entry
points while leaving layer norm/residual mixing to its caller. The full IQ3_S
sampler exercises this loader for block 0; outputs are finite for single-token
decode and two-token prefill with synthetic hidden vectors and zero initial
states. Synthetic tests match dense single-token/two-token outputs and states,
and confirm a later-token row-read failure rolls back the whole prefill.
Evidence is
updated in `evidence/ista_iq3s_full_payload_sample_rows.tsv`. This remains a
single-layer CPU operator check, not independent quantized numeric comparison
or model inference.

Follow-up: `gated_residual_from_rows` and `ResidualMixerGguf` now stream the
attention/FFN hyper-connection down/up/injection matrices. The actual IQ3_S
sampler composes block 0 end to end through both residual mixers, the actual
GatedDeltaNet and routed/shared MoE, starting from the actual `token_embd`
row for token ID 1 and four-stream replication. The resulting 10,240-value
hyper-state is finite. Block 0 has no PLE; this does not yet compose the PLE
block or the full 48-block model, and there is no independent expected output
for this quantized path.

Follow-up: `agentix-qwen-sys::QsaGgufLayer` now loads and checks one configured
QSA block's indexer/main-attention matrices and norm vectors, owns sequential
per-request raw-index/KV caches, and commits those caches only after decode
succeeds. The IQ3_S sampler runs two sequential positions through block 3 using
actual weights and finite synthetic inputs; its separate 4,097-position
synthetic-past test continues to cover sparse token-budget behavior. This does
not provide model-derived block inputs or validate multi-layer execution.

Follow-up: the real-artifact sampler now also composes block 3 from actual QSA
attention, both residual mixers, and routed/shared MoE weights, using a
synthetic four-stream input. The finite 10,240-value output and single-token
cache are recorded in the refreshed TSV (SHA256
`edfffb60eb066a1d34ded1da09c3605d69b40d2edee65a609925c312b38e75d5`). Block
0 remains the actual-token entry point for this sampled path; at this point the
path did not yet include PLE propagation or multi-block execution, and no
independent numeric oracle existed.

Follow-up: streamed PLE key/value projections, transactional n-gram and
convolution state, and `PleGgufLayer` now bind the candidate's actual PLE table
and block-1 weights. The artifact sampler carries actual token ID 1 through all
48 configured blocks, reads the 16 PLE rows at block 1, applies each
GatedDeltaNet/QSA and MoE path with the residual mixers, then performs final
stream combine and the full vocabulary projection. It emits finite logits; the
greedy token is 79104 (logit 11.96474266). Updated TSV SHA256:
`fdf8b108f59530d135161a81e4bd447145300a1f13ebd2598b4f5fbf66214869`. This is
one CPU forward smoke run, not independent full-model parity or a persistent
generation worker.

The actual-artifact sampler also streams `output_hc_down.weight` (BF16,
320 x 10,240) through `reference::linear_from_rows`; all 320 projected values
are finite. This validates one stored-matrix/operator connection, not
independent numeric parity or end-to-end model output.

Follow-up: `QwenGgufTensorStore::open_group_matrix_rows` exposes one bounded
expert matrix view from a rank-three `[in, out, experts]` tensor. The full
IQ3_S sample now projects through expert 0 of `blk.17.ffn_gate_exps.weight`
(2,560 inputs to 640 outputs), with finite output, and rejects an out-of-range
expert index and a rank-two tensor passed as an expert stack. This closes a
storage-to-linear gap for one MoE expert; gate/up transforms, routing, and
full layer execution remain separate work.

The current follow-up streams the complete gate/up/down SwiGLU for real
block-17 expert 0 and produces 2,560 finite outputs. A synthetic CPU reference
test matches the materialized SwiGLU equations. No independent expected values
exist for that real quantized expert; the subsequent `MoeGgufLayer` follow-up
below now exercises routing/shared-expert integration for one real block.

The sampler now composes that storage path into `moe_block_from_rows` for one
real block-17 token: router, config-selected top-10 experts, normalized route
weights, shared SwiGLU and sigmoid gate, all with finite 2,560-value output.
This is an operator smoke run using a constant hidden vector, not a reference
logit comparison or full-model token execution.

Follow-up: `agentix-qwen-sys::MoeGgufLayer` now validates router/shared
projection and rank-three expert-stack geometry, opens only routed expert
groups, and passes the configured `norm_topk_prob` behavior to the streamed
MoE reference. The option is parsed from Qwen config and defaults to the pinned
Transformers value (`true`) when omitted. The actual block-17 candidate check
now runs through this layer loader. It remains a one-token CPU operator test
with synthetic input, without independent expected logits or model graph
execution.

`qsa_attention_step_from_rows` now executes the existing QSA main-attention
reference with streamed quantized Q/K/V/output matrices. Its synthetic fixture
matches the dense reference including cache contents. The full IQ3_S sample
runs block 3 from one constant hidden vector and returns a finite hidden output
plus KV cache elements; the row-backed indexer follow-up below now supplies a
sparse visibility mask to this operator.

The follow-up adds `qsa_indexer_project_from_rows` and
`qsa_indexer_step_from_rows` for separate quantized query/key readers. The
full artifact check uses one actual block-3 hidden-vector projection with a
synthetic 4,096-token raw-key past, applies the pinned 2,048-token budget,
retains 2,049 tokens (budget plus incomplete tail), and feeds that sparse mask
into row-backed main attention. This verifies operator wiring and selection
shape only; past activations/KV state are synthetic and no reference logits
exist. Full-sequence indexer prefill and real model cache continuation remain
open.

`qsa_decode_step_from_rows` now owns this composition and returns the output,
selected indices and both updated caches. A synthetic fixture matches the
existing materialized `qsa_decode_step`; the actual IQ3_S sampler uses the
combined API for the block-3 smoke decode. Full layer transitions and
model-derived cache continuation remain unimplemented.

- [ ] T008 [US2] Select initial full-model artifact and fallback quantization using measured available capacity; validate exact shard/auxiliary identities in `hardware.md` and `sources.md` (depends T004/T006/T007).
- [ ] T009 [US2] Freeze 4K/32K/64K protocols, output lengths, settings, three-repeat procedure and ten coding/tool tasks in `benchmarks.md` (depends T006). The protocol, exact Nix-pinned prompt/token fixtures, frozen ten-task file/hash, and clean feature baseline commit are now pinned. A clean-checkout evaluator and baseline run records remain required before the coding suite or model benchmark execution.
- [ ] T010 [US3] Write pinned Nix fixture/dependency/package strategy and reproduction commands in `quickstart.md`; package implementation is T030 after design review (depends T007). Current CPU/native dependency boundary, tree audit, pinned tokenizer/template hashes, compact synthetic full-decoder fixture command/hash, and a NixOS smoke-deployment recipe with shard checksums are recorded; target-device execution, independent benchmark-reference packaging, and the approved CUDA test-package strategy remain open.
- [ ] T011 [US2] Run feasible existing-backend and separate Strata reference experiments on the actual host, retaining raw records under `results/` (depends T008/T009/T030).
- [ ] T012 [US2] Report matched-engine and different-model results separately; missing measurements remain blockers in `benchmarks.md` (depends T011).
- [x] T013 [US3] Resolve implementation base and daemon/library or existing worker assembly against recorded 019/020 contracts; update `contracts.md` without silently merging branches (depends T006). The base remains unchanged, assembly is specified through the existing Unix-socket proxy, and gaps against branch 020 are explicit in `contracts.md`; this is a contract/design completion only, not executable routing.
- [x] T014 [US1] Prepare small pinned model fixture and independent operator/state vectors; fix numerical tolerances, tokenizer cases and full-model reference protocol in `qwen-design.md` (depends T006/T007). Analytical CPU vectors cover gated residual, routed/shared experts, PLE gate/convolution, QSA selection/projection/decode/prefill, and GatedDeltaNet step/prefill; tokenizer/config tests cover exact token IDs, tool/reasoning special-token round-trip, and unsupported prompt rejection. The generated four-block synthetic GGUF fixture is hash-pinned and exercises the complete stateful decoder against exact zero-logit/token outputs. `qwen-design.md` records scalar/sample tolerances and the independent full-model protocol. Nonzero checkpoint outputs and full-model threshold calibration remain open under T018/T020.
- [x] T015 [US1] Finalize lifecycle, streaming, tools/reasoning, cancellation/backpressure, memory admission, capabilities and error contracts in `contracts.md` (depends T013/T014). The contract now defines lifecycle, strict context admission, exactly-once stream termination, cancellation/backpressure, unsupported capabilities and typed error/HTTP/SSE mapping. Prompt preparation enforces rendered-token context limits. This is a completed design artifact only; worker/API integration and fixture-based conformance remain T021.
- [x] T016 [US3] Review design and constitution; confirm `agentix-qwen` and `agentix-qwen-sys` have non-overlapping purposes and no native leakage into generic crates, and review the worker/CUDA extension before expanding the package graph (depends T007/T010/T013/T015). `decision.md` records the full normal dependency-tree, worker/daemon routing, CUDA feature and Nix capability-packaging review. The boundary and package review passes; target driver/device execution, CUDA parity and model-backed lifecycle acceptance remain open under T019/T021/T027.

## Ordinary inference implementation

- [ ] T030 [US3] Implement pinned Nix fixture/dependency/package definitions and independently packaged benchmark reference following the reviewed strategy; validate fixture availability and build environment (depends T010/T014/T016).

- [ ] T018 [US1] Implement loader, identity/geometry/type validation and reference operators in approved backend crate paths; test corrupt/missing/unsupported inputs (depends T014/T016/T030). Partial foundation: content identity and a pinned, fail-closed text-geometry config parser are implemented; the sys crate checks generic GGUF payload/type/shape and split structure, retains bounded scalar metadata/array descriptors, and reads F32/F16/BF16/Q4_0/IQ4_NL/Q8_0/IQ2_XS/IQ3_S rows on demand. `DenseRows` converts dense float rows; `QuantizedRows` decodes five quantized formats, rejects unsupported encodings, flattens checked trailing tensor dimensions, and reports rows per group for expert stacks without introducing native dependencies into `agentix-qwen`. Quantized decoders and dense F16/BF16 rows are checked against pinned native GGML conversion callbacks; Q4_0 and IQ4_NL also match one hash-verified published PLE row each. IQ2_XS/IQ3_S have synthetic block tests; a real IQ3_S expert row from a `[2560,640,512]` tensor was decoded via the bounded reader and is retained with its header inventory, but no independent numeric oracle comparison exists. F32/F16/BF16 GGUF row parsing and non-finite rejection are tested. `rows::SafeTensorFile` validates bounded SafeTensors metadata and exact file extent; `Bf16Rows` reads finite BF16 rows on demand; `SafeTensorFile::float_rows` reads rank-two F32/F16/BF16 tensors as finite F32 rows with bounded caching, and `float_vector` reads bounded rank-one vectors. `ple::open_qwen4exp_ple_rows` uses the checkpoint index and config geometry to open the 128 ordered source PLE tensors from their files, and `ShardedEmbeddingRows` composes their readers without materializing the table. Unit tests exercise the public opener with a reduced valid config and synthetic shard files, including cross-shard row order; helper tests cover index resolution, missing tensors, unsafe traversal/symlinks, storage decoding, truncation and unsupported types. One actual source BF16 PLE row matches the corresponding published Q4_0 GGUF row within 0.003, and one complete 128-value non-PLE BF16 norm vector matches its published F32 GGUF target exactly after the direct source transform. A strict Qwen GGUF directory index rejects unrecognized types and out-of-file ranges; the real 1,224-tensor ggml-org header indexes with all six observed types. The Agentix-owned geometry schema matches all 1,224 tensor names/extents. All 1,294 text source names map to those 1,224 GGUF names/metadata entries with fan-in/fan-out recorded. All 128 source PLE headers verify BF16 `[2500012,160]` rows matching the GGUF table total. Checked CPU transforms cover indexer Q/K split, expert gate/up split, PLE zero-centered norm offset/convolution squeeze, and GatedDeltaNet grouped-to-tiled value-head reordering, including pinned -exp(A_log) before head reorder; captured A_log and dt_bias BF16-to-F32 source/GGUF vectors match within their recorded tolerances; `source::transform_qwen4exp_f32_tensor` now dispatches mapped float tensors through those checked primitives. PLE hash metadata remains integer-preserving and the PLE shard path remains file-backed; neither is part of this dense-float transform function. CPU references cover single-token QSA indexer projection/query RMSNorm/RoPE/cache/selection, combined one-token main QSA attention with sparse mask, and sequential fresh-sequence and prefix-cache QSA prefill. They also cover n-gram-addressed PLE row lookup through a decoded-row interface, transactional PLE projection/gate/convolution and sequential transactional one-sequence prefill, text partial RoPE, routing, normalization, dense linear projections, gated residual mixing/reinjection, routed/shared SwiGLU MoE, and tokenwise GatedDeltaNet step/prefill. Tokenizer-backed prompt preparation enforces actual rendered-token context admission. The full IQ3_S payload is now schema-validated and read through the lazy full-tensor store into a reusable 48-block stateful CPU decoder; a two-token real-artifact smoke pass is recorded. Full source checkpoint materialization/serialization, broader source-value parity, batched QSA prefill, native/CUDA kernels, independent full-model numeric parity, and full-model unsupported-type rejection remain unimplemented.
- Additional T018 full-graph check: the IQ3_S sampler carries token ID 1 through all 48 blocks, including block-1 PLE, all GatedDeltaNet/QSA layers and routed/shared experts, then final stream combine and output projection. The reusable decoder processes greedy token 79104 in a second full pass, preserves per-layer state, and emits 248,320 finite logits at both positions. Evidence is in `evidence/ista_iq3s_full_payload_sample_rows.tsv`. This verifies stateful execution, not independent reference parity or full-model acceptance.
- Additional T018 boundary check: an exhaustive pass over all 248,320 model vocabulary IDs with evolving n-gram history confirms each of the 16 emitted PLE row IDs is less than the 320,001,446 logical rows. The 90 source-alignment padding rows in the 320,001,536-row physical table are therefore not reachable through `Ngram` addressing.
- Additional T018 transform dispatch: the new `agentix-qwen::source::transform_qwen4exp_f32_tensor` connects source-name mapping to the checked Q/K and expert splits, norm offset, PLE convolution squeeze and all pinned GatedDeltaNet value-head reorder cases. PLE shard chunks remain on the indexed reader path. `SafeTensorFile::i64_vector` and `ple::open_qwen4exp_ple_hash_metadata` read the three PLE hash arrays through the checked index/path boundary, preserve values above F32's exact range, and return typed integer metadata. `agentix-qwen-sys::rewrite_qwen4exp_gguf_ple_hash_metadata` emits the pinned three metadata arrays as GGUF UINT64 while preserving original tensor payload bytes; a synthetic qwen4exp GGUF test covers values, array descriptors, payload preservation and failure cases. Metadata read and emission are implemented; parity against the full source checkpoint and model-wide conversion remain open.
- [ ] T019 [US1] Implement independent CUDA/native compute path with bounded allocations and CPU/device operator/state comparisons (depends T018). Partial: `agentix-qwen-sys` exposes GGML CUDA device inventory, bounded F32 matvec, and chunked matvec over original quantized rows from validated GGUF matrix views. `QwenGgufDecoder` and the worker can opt into CUDA for only the final quantized vocabulary projection; all other operators remain CPU-side, and output weights are re-uploaded for every token. The CUDA test binary links, strict all-target Clippy passes, the raw-row test covers byte preservation/range checks, and `compare_cuda_gguf_matvec` provides a target-host comparison over the real Q6_K output tensor. Tests cannot launch here because `libcuda.so.1` is unavailable; no device result, CUDA numerical parity, stateful layer support, practical GPU residency, or target execution is verified.
- [ ] T020 [US1] Implement tokenizer/template integration, ordinary decoding and sampling, context accounting and full-model correctness checks (depends T008/T019). Partial: pinned MiniJinja rendering/tokenization and actual rendered-token context admission are implemented; `QwenGgufDecoder::generate_prepared_greedy` connects an admitted `PreparedPrompt` to the existing deterministic greedy loop and feeds only new IDs into persistent model state. The real IQ3_S sampler exercises this API with token ID 1 as a minimal prepared prompt; two consecutive full 48-block steps preserve per-layer recurrent, QSA, PLE n-gram, and position state and yield finite logits, with first greedy token 79104 and second 245228. The sampler now has an opt-in path that prepares a real user message through the pinned tokenizer/template before full-model decode; it is not yet run against the full artifact. There is still no independent full-model logit oracle or numeric acceptance result.
- [ ] T021 [US1] Wire backend into existing inference and daemon assembly; test streamed tools/reasoning, cancellation, backpressure, readiness, unload and typed errors with pinned fixtures (depends T015/T020). Partial: added `agentix-qwen-worker`, which verifies ordered GGUF SHA256 values, schema/layer geometry and tokenizer vocabulary before binding a Unix socket; it serves health/model-list, greedy text completions and bounded per-token SSE, with a one-request semaphore, blocking-pool inference and disconnect checks. The daemon routes only the exact configured model ID to that socket, returns a typed 503 on socket unavailability without using the llama socket, and explicitly rejects unsupported Qwen endpoints. Worker tests reject invalid reasoning-option types and unsupported reasoning modes. A NixOS service option/package path and exact-route tests are in place. An experimental `AGENTIX_QWEN_CUDA_OUTPUT_DEVICE` setting can opt into a CUDA final projection when the worker is CUDA-enabled; startup validates the selected device, but target execution and model-backed service acceptance remain open. User-provided kursk journal output now proves the immediate startup error is a missing `/var/lib/agentix/qwen/iq3s-00001.gguf`; it does not establish second-shard presence, successful download/hash verification, readiness, or model-backed request acceptance. The quickstart records the pinned source paths and destination names. Tool parsing/execution, reasoning deltas, streamed cancellation under real inference, unload/reload, full error conformance, actual-worker fixtures and target runtime acceptance remain open.

## Optimization implementation

- [ ] T022 [US2] Implement bounded expert placement/cache and instrument hits, evictions, memory and transfer bytes; compare outputs against ordinary baseline (depends T021).
- [ ] T023 [US2] Implement CPU/GPU/transfer overlap with event-owned staging buffers; test cancellation and reuse races; profile cost and numerical equivalence (depends T022).
- [ ] T024 [US2] Implement bounded PLE row cache/read batching and separate prefill scheduling/scratch budgets; compare uncached results and measure TTFT (depends T023).
- [ ] T025 [US2] Implement prefix-state reuse and identity/eviction validation (depends T024).
- [ ] T026 [US2] Independently prepare MTP artifacts and implement optional speculative acceptance/commit/rollback, including recurrent state and sampling-mode validation (depends T025).

## Evidence and final review

- [ ] T027 [US2] Execute optimization ablations and full host/coding suite; evaluate SC-003–005 using accepted output and matched settings in `benchmarks.md` (depends T012/T026).
- [x] T028 [US3] Audit source/dependency provenance, isolated native boundary and Strata-free build/runtime; verify existing backend remains selectable (depends T021/T026). Normal Cargo trees for `agentix-qwen`, `agentix-qwen-sys`, `agentix-qwen-worker` and `agentix-daemon` show no Strata dependency; only the sys crate introduces `llama-cpp-sys-2`. The CPU crate remains native-free, and exact configured-model dispatch leaves existing `RouteTarget::Local` requests on the llama socket. Worker and daemon Nix packages build.
- [ ] T029 [US3] Run Nix package builds, fmt, Clippy and workspace tests; record results and recheck constitution in `decision.md` (depends T027/T028). CPU sys tests (30), strict sys Clippy, and format pass. The latest CUDA-enabled `nix build .#agentix-qwen-sys` compiles the release crate but its sandbox test phase cannot load `libcuda.so.1`; the separate CPU Nix test run succeeds. Earlier worker/daemon/indexer builds and NixOS evaluation passed. Awaiting an approved CUDA-package check-phase adjustment before closing the current package gate; target-device acceptance remains under T019/T021/T027.
- [x] T017 Map every FR/SC to evidence or named blockers in `decision.md`; update checklist and final implementation status (depends T029). The 2026-10-13 coverage map records implemented evidence and remaining external/model/optimization gates; this closes the mapping task, not the open requirements.

## Coverage and execution

FR-001/003 → T006/T008/T014/T018–020; FR-002 → T007/T028; FR-004 → T004/T008/T021–024; FR-005 → T022/T023; FR-006 → T024; FR-007 → T025/T026; FR-008 → T013/T015/T021; FR-009 → T007/T016/T021/T029; FR-010/011 → T009/T011/T012/T027; FR-012 → T014/T028/T029. SC-001 → T028; SC-002 → T014/T018–026; SC-003/004/005 → T027; SC-006 → T028/T029.

Host access blocks measurements, not independent design. No parallel-agent execution is requested. If an approach or test fails, explain the failure and ask before implementing a different approach.

## Implementation refinement — independently testable foundations

- [x] T031 [US1] Implement and test independent n-gram hashing/history, reference routing and recurrent update primitives in `agentix-qwen/src/`, based on pinned original Qwen/Transformers contracts in `qwen-design.md`.
- [x] T032 [US2] Implement checked separate host/device admission and bounded on-demand row storage in `agentix-qwen/src/`; test EOS/chunk/restore behavior, invalid state inputs, file bounds and cache capacity.
- [x] T033 [US3] Add workspace/Nix packaging for the CPU foundation and run focused tests, formatting, Clippy and Nix builds. These tasks do not mark full-model tasks T006–T030 complete.
