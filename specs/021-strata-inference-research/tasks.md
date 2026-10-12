# Tasks: Agentix-owned Qwen Flash Next inference

**Scope**: Updated research, independent design, staged implementation and verification. No backend code has been written.
Paths are relative to this spec directory unless a workspace path is named. Prior research task numbering is retained where useful; revised descriptions reflect the selected direction.

## Established research and direction

- [x] T001 Allocate and verify existing feature/worktree 021 from master.
- [x] T002 Revise `spec.md`, `plan.md`, `decision.md` and checklist for Qwen with no Strata code reuse.
- [x] T003 Record pinned Strata/dependency/model metadata in `sources.md`; record failed indexing and user-authorized direct reads in `research.md`. Index coverage and downloaded weight verification are not claimed.
- [ ] T004 Complete actual target inventory and separate RAM/VRAM budgets in `hardware.md`.
- [x] T005 Record inspected Strata mechanisms and output-changing approximations in `research.md` as reference evidence, not implementation instructions.
- [ ] T006 [US1] Pin original Qwen reference/configuration/tokenizer/template and quantization definitions; write complete operator/state/tensor contract in `qwen-design.md` and provenance in `sources.md` (depends T003).
- [ ] T007 [US3] Select independent loader/compute dependencies and Ampere/Nix strategy; document supported formats, provenance and no-Strata dependency audit in `qwen-design.md` (depends T006).

## Design and benchmark gates

- [ ] T008 [US2] Select initial full-model artifact and fallback quantization using measured available capacity; validate exact shard/auxiliary identities in `hardware.md` and `sources.md` (depends T004/T006/T007).
- [ ] T009 [US2] Freeze 4K/32K/64K protocols, output lengths, settings, three-repeat procedure and ten coding/tool tasks in `benchmarks.md` (depends T006).
- [ ] T010 [US3] Write pinned Nix fixture/dependency/package strategy and reproduction commands in `quickstart.md`; package implementation is T030 after design review (depends T007).
- [ ] T011 [US2] Run feasible existing-backend and separate Strata reference experiments on the actual host, retaining raw records under `results/` (depends T008/T009/T030).
- [ ] T012 [US2] Report matched-engine and different-model results separately; missing measurements remain blockers in `benchmarks.md` (depends T011).
- [ ] T013 [US3] Resolve implementation base and daemon/library or existing worker assembly against recorded 019/020 contracts; update `contracts.md` without silently merging branches (depends T006).
- [ ] T014 [US1] Prepare small pinned model fixture and independent operator/state vectors; fix numerical tolerances, tokenizer cases and full-model reference protocol in `qwen-design.md` (depends T006/T007).
- [ ] T015 [US1] Finalize lifecycle, streaming, tools/reasoning, cancellation/backpressure, memory admission, capabilities and error contracts in `contracts.md` (depends T013/T014).
- [ ] T016 [US3] Review design and constitution before creating proposed `agentix-qwen` and optional `agentix-qwen-sys`; confirm non-overlapping purposes and no native leakage into generic crates (depends T007/T010/T013/T015).

## Ordinary inference implementation

- [ ] T030 [US3] Implement pinned Nix fixture/dependency/package definitions and independently packaged benchmark reference following the reviewed strategy; validate fixture availability and build environment (depends T010/T014/T016).

- [ ] T018 [US1] Implement loader, identity/geometry/type validation and reference operators in approved backend crate paths; test corrupt/missing/unsupported inputs (depends T014/T016/T030).
- [ ] T019 [US1] Implement independent CUDA/native compute path with bounded allocations and CPU/device operator/state comparisons (depends T018).
- [ ] T020 [US1] Implement tokenizer/template integration, ordinary decoding and sampling, context accounting and full-model correctness checks (depends T008/T019).
- [ ] T021 [US1] Wire backend into existing inference and daemon assembly; test streamed tools/reasoning, cancellation, backpressure, readiness, unload and typed errors with pinned fixtures (depends T015/T020).

## Optimization implementation

- [ ] T022 [US2] Implement bounded expert placement/cache and instrument hits, evictions, memory and transfer bytes; compare outputs against ordinary baseline (depends T021).
- [ ] T023 [US2] Implement CPU/GPU/transfer overlap with event-owned staging buffers; test cancellation and reuse races; profile cost and numerical equivalence (depends T022).
- [ ] T024 [US2] Implement bounded PLE row cache/read batching and separate prefill scheduling/scratch budgets; compare uncached results and measure TTFT (depends T023).
- [ ] T025 [US2] Implement prefix-state reuse and identity/eviction validation (depends T024).
- [ ] T026 [US2] Independently prepare MTP artifacts and implement optional speculative acceptance/commit/rollback, including recurrent state and sampling-mode validation (depends T025).

## Evidence and final review

- [ ] T027 [US2] Execute optimization ablations and full host/coding suite; evaluate SC-003–005 using accepted output and matched settings in `benchmarks.md` (depends T012/T026).
- [ ] T028 [US3] Audit source/dependency provenance, isolated native boundary and Strata-free build/runtime; verify existing backend remains selectable (depends T021/T026).
- [ ] T029 [US3] Run Nix package builds, fmt, Clippy and workspace tests; record results and recheck constitution in `decision.md` (depends T027/T028).
- [ ] T017 Map every FR/SC to evidence or named blockers in `decision.md`; update checklist and final implementation status (depends T029).

## Coverage and execution

FR-001/003 → T006/T008/T014/T018–020; FR-002 → T007/T028; FR-004 → T004/T008/T021–024; FR-005 → T022/T023; FR-006 → T024; FR-007 → T025/T026; FR-008 → T013/T015/T021; FR-009 → T007/T016/T021/T029; FR-010/011 → T009/T011/T012/T027; FR-012 → T014/T028/T029. SC-001 → T028; SC-002 → T014/T018–026; SC-003/004/005 → T027; SC-006 → T028/T029.

Host access blocks measurements, not independent design. No parallel-agent execution is requested. If an approach or test fails, explain the failure and ask before implementing a different approach.

## Implementation refinement — independently testable foundations

- [ ] T031 [US1] Implement and test independent n-gram hashing/history, reference routing and recurrent update primitives in `agentix-qwen/src/`, based on pinned original Qwen/Transformers contracts in `qwen-design.md`.
- [ ] T032 [US2] Implement checked separate host/device admission and bounded on-demand row storage in `agentix-qwen/src/`; test EOS/chunk/restore behavior, invalid state inputs, file bounds and cache capacity.
- [ ] T033 [US3] Add workspace/Nix packaging for the dependency-free foundation and run focused tests, formatting, Clippy and Nix builds. These tasks do not mark full-model tasks T006–T030 complete.
