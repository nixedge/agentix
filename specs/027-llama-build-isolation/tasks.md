# Tasks: Isolated workspace builds

- [x] T001 Audit current build graph, allocate027/verify host worktree, specify scope and plan before code.
- [x] T002 Implement scoped workspace/lock/patch projection with explicit dependency identities and dependency-only sources.
- [x] T003 Refactor every package/check to scoped sources/artifacts/native inputs; expose all workspace members and retain public wrappers.
- [ ] T004 Add canonical all-member cache-invalidation regression and locked/offline Cargo projection checks.
- [ ] T005 Run regression checks, representative canonical builds, formatting and CUDA evaluation; document evidence/limitations and commit without pushing.

Owner requested committing the current checkpoint before resuming026. All20 real-member locked/offline/all-features metadata checks pass, as do canonical agentix-api, agentix-jails, assistant-core (26 tests), and agentix-llama-cpu builds. Formatting and whitespace checked. Cache-invalidation mutation regressions, CUDA derivation evaluation and the broader workspace/GPU suite remain open; the checkpoint does not claim these are complete.
