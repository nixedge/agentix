# Decision: independently implement Qwen Flash Next in agentix

**Updated**: 2026-10-12
**Status**: Direction selected by user; operator/kernel design and target-host validation pending. No engine implemented.

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

At base `fd2202890a95e1deb9747ccd8ad39be482f526f8`, the daemon proxies local inference over a Unix socket to `agentix-llama`; `agentix-daemon/src/gateway/proxy.rs::forward` streams response bodies. An agentix-owned worker, if needed, is different from depending on the Strata executable. Final assembly remains a design task; inference libraries remain network-free and the daemon remains the public listener.

At branch 020 revision `0a70143265f7bb7eb83211cdd7caf11c4e7d7ced`, inspected contracts add typed tool completion/cancellation, coding capability, context limits, busy state, artifact identity, readiness and model-operation phases. Preserve these semantics when reconciling the implementation base; no sibling branch was merged. See `research.md` for the prior source inspection.

## Performance and model choice

Full-model IQ3_S is the provisional quality-oriented candidate; IQ2_XS is the provisional capacity/performance comparison. Independent format/kernel support, exact artifact verification and actual memory budgets must confirm them. Coder pruning is a model change, not an engine optimization.

The 90% same-model Strata throughput criterion is a target to evaluate on the actual host. Published measurements on other CPUs and upstream estimates do not predict the new engine's throughput. First establish correct ordinary decoding, then retain optimizations only with measured benefit and correctness evidence. Speculative throughput counts accepted tokens only. Compare Strata's tail-skipping-disabled reference separately from its approximate defaults.

## Current evidence and blockers

- Completed: pinned Strata mechanism study, published model metadata/manifests, initial agentix boundary inspection, user-confirmed hardware and revised spec/plan/tasks.
- Source indexing failed; user-authorized pinned direct reads completed the mechanism study. No index completeness is claimed.
- Pending: original Qwen operator/tensor/state contract, independent quantization/kernel dependency selection, fixture/tolerance design, final integration contracts and exact build graph.
- Pending: model weight verification, target driver/free-memory/bandwidth/PCIe/storage inventory and host access. This session runs on another machine.
- Pending: all engine implementation, correctness tests, Nix builds and target-host experiments. No throughput or fit claim is measured here.

## Acceptance and rollback

The spec and task coverage table define all FR/SC gates. Build/run without Strata, retain the existing selectable backend, fail explicitly on unsupported capabilities or memory pressure, and do not silently substitute models or cloud execution. Final release assessment must map every gate to evidence or a named blocker; design documentation alone is not a passing implementation result.
