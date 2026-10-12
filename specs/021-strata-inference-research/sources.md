# Source and artifact provenance

**Recorded**: 2026-10-12. Model metadata was fetched; model weight bytes were not downloaded or hashed locally.

| Source | Revision | Evidence |
|---|---|---|
| agentix feature base | `fd2202890a95e1deb9747ccd8ad39be482f526f8` | Checkout and lockfile inspected |
| Strata | `7695859aa13afb248bdb4d36f9b2e95707db079d` | Clean detached checkout, CMake version 0.1.43 |
| Strata's ggml/llama.cpp dependency | `3cf03257f219afbe7334045ff7c6a06ac68c627d` | `CMakeLists.txt` FetchContent pin and `third_party/ggml/VERSION.txt`; dependency checkout not built |
| agentix 019 | `c96657c281e2ac74167b759a799b14a33e71f26e` | Ref recorded |
| agentix 020 | `0a70143265f7bb7eb83211cdd7caf11c4e7d7ced` | Inference/API contracts and changed-file inventory inspected |
| Full GSQ-RCO model | `ed59f92082b1e93c0e96d60a8b11aab089b52f09` | Strata setup pin confirmed with HF metadata |
| Coder GSQ-RCO model | `5348543e0147355ac9cbcb031184a3546350988e` | Strata setup pin confirmed with HF metadata |
| Unsloth candidate | `38bb39ee97821de2c9009abb7e93950eec396e66` | Strata setup pin; not selected for first experiment |
| Original Qwen MTP tensors | `de4b8e4d43b917e7706784d8bb445c9af86a3540` | `tools/mtp_fetch.py::PINNED_REVISION`, per-tensor SHA256 table |

## Selected model manifests

- [Full model metadata](evidence/full-model-manifest.json): IQ2_XS and IQ3_S paths, sizes and publisher-provided LFS SHA256.
- [Coder metadata](evidence/coder-model-manifest.json): IQ1_M paths, sizes and publisher-provided LFS SHA256.

| Candidate | Shard 1 bytes | PLE shard bytes | Total decimal GB |
|---|---:|---:|---:|
| IQ2_XS full | 39,225,954,592 | 28,800,138,432 | 68.026 |
| IQ3_S full | 54,817,524,224 | 28,800,138,432 | 83.618 |
| IQ1_M Coder | 29,608,446,496 | 28,800,138,432 | 58.409 |

All three PLE shards have the same publisher SHA256, allowing verified deduplication. Coder's label does not mean all expert tensors are one-bit: Strata's setup describes its expert storage as comparable to IQ3_S and half the original experts retained. Confirm the tensor allocation in actual files when preparing artifacts.

Additional artifacts to resolve independently: tokenizer/vocabulary/merges and exact chat template, any agentix-owned packing metadata, optional measured expert profile, and MTP source/prepared weights if speculation is enabled. Do not require Strata-generated packs or its preparation scripts. Vision projector is excluded from the first text-only experiment. Hash artifacts after independent deterministic preparation; the GGUF digest alone does not identify the complete runtime configuration. Existing model metadata is a candidate inventory, not proof of independent loader/kernel support.

The installed baseline catalog reports Qwen3-Coder-30B-A3B-Instruct-Q4_K_M at 18,556,689,568 bytes. Its installed digest and source revision still need host verification.

## Licenses and provenance limits

Strata's root LICENSE and its vendored ggml notice are MIT. These are historical research observations: user direction excludes reuse of Strata code regardless of license. Source any general-purpose compute dependency independently and preserve its own attribution. Model-hub metadata for the selected ISTA snapshots reports Apache-2.0, while the upstream Qwen model card reports a Qwen community license. Do not treat either metadata label as resolving the full derivative-weight licensing chain; preserve model provenance and review the actual applicable license files before redistribution. Local research does not settle redistribution terms.

`setup.py` and `tools/mtp_fetch.py` contain fallbacks from a missing pinned revision to current files. A reproducible agentix preparation path must fail closed instead of inheriting that fallback. Do not run the upstream installer as a Nix build step.

## Source access

[Strata pinned tree](https://github.com/Niko1221/Strata/tree/7695859aa13afb248bdb4d36f9b2e95707db079d). Local checkout: `/home/sam/home/Strata/research-7695859`. Direct reads explicitly authorized after embedding/indexing failed; the failed index is not used as evidence of complete coverage.
