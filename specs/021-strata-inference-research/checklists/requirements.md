# Requirements quality checklist

**Updated**: 2026-10-12
**Scope**: Revised specification quality; checked items do not certify a working engine.

- [x] Qwen Flash Next, full-expert initial scope and confirmed target hardware are explicit.
- [x] No Strata source copying/translation, runtime dependency, kernels or preparation scripts are allowed.
- [x] Prior source exposure is acknowledged; independent dependency provenance is required.
- [x] DeepSeek, vision, multi-GPU and concurrent generation are outside initial scope.
- [x] Correct ordinary inference precedes caching, overlapping execution and speculation.
- [x] Approximate expert skipping is excluded; quantization/model changes are separate comparisons.
- [x] RAM/VRAM budgets, context, staging, caches and service headroom are covered.
- [x] Tokenization, tools/reasoning, cancellation, lifecycle and rollback have acceptance requirements.
- [x] Same-model host benchmarks, three repetitions, coding correctness and performance targets are explicit.
- [x] Requirements and success criteria map to implementation tasks.
- [x] Source study and revised plan are distinguished from unimplemented/unmeasured behavior.
- [ ] Original Qwen operator contract and independent dependencies finalized.
- [ ] Target inventory, artifacts and memory budgets verified.
- [ ] Backend and optimizations implemented and correctness-validated.
- [ ] Host measurements and same-model comparison complete.
- [ ] Provenance review, Nix builds, tests and final constitution checks complete.
