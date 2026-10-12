# Implementation progress

## Recovery checkpoint — 2026-10-12

The worktree directory was mistakenly created on the jail tmpfs; only `master` and `.bare` are persistent mounts. This commit preserves all project files in the persistent bare repository on branch `021-strata-inference-research`. Restore a host-visible checkout before further implementation. A separate archive also exists at `/home/sam/.codex/agentix-021-recovery.XZnpn3/worktree-files.tar.gz`.

Current work includes the revised Qwen specification/design/contracts and initial untested Rust sources for n-gram hashing/history, memory admission and row storage. The crate is not a workspace member yet; `lib.rs` references an unwritten `reference` module. This is a recovery snapshot, not a buildable or complete backend. No tests, model inference or GPU benchmarks have run.

Direct pinned upstream reads have now been authorized. Transformers source revision `2fa1fdd5b27ae54bdcee0516848c7963393a5f5c` was inspected; the model/operator notes are in `qwen-design.md`. The Nix dev-shell check completed successfully with Cargo 1.94.1. The earlier pending entries below are historical.


## 2026-10-12 — implementation authorized

- User requested implementation of the independent Qwen backend.
- Spec Kit prerequisite check passed in worktree 021.
- Official Qwen configuration resolved to revision `de4b8e4d43b917e7706784d8bb445c9af86a3540`; text architecture is `qwen4_exp_text`, not a generic Qwen3 decoder. Original operator source is required before implementing the execution graph.
- Agentix MCP search confirmed backend selection and loading boundaries; local files and branch 020 contract definitions were inspected.
- Added `contracts.md` draft, including gaps in the base and 020 reasoning/tool interfaces. No sibling branch merged.
- Transformers 5.8.0 indexing failed with an Ollama error. Requested authorization for direct pinned upstream source retrieval; do not treat failed ingestion as evidence of complete source coverage.
- Requested target SSH hostname/user for GPU validation. The current session remains on another host.
- Project Nix development-shell check started; result pending.

No backend crate, kernel, model download, performance result or completed implementation is claimed by these records.
