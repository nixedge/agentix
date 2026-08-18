# Tasks: Fix gh Jail to Allow All GitHub Remotes

**Input**: Design documents from `/specs/012-gh-jail-all-remotes/`
**Prerequisites**: plan.md ✅, spec.md ✅, research.md ✅

**Organization**: Tasks are grouped by user story. All changes are confined to a single file: `agentix-jails/src/jail/main.rs`.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1, US2, US3)

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: No new project structure is required — the change is confined to an existing file. This phase covers any prerequisite adjustments.

- [x] T001 Add `use std::collections::HashSet;` import to `agentix-jails/src/jail/main.rs`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Extract the URL-parsing logic from the existing `github_repo_from_remote` function into a standalone `parse_github_slug` helper, so it can be reused and unit-tested independently of process I/O.

**⚠️ CRITICAL**: US1, US2, and US3 all depend on `parse_github_slug` existing before their respective work can proceed.

- [x] T002 Extract `parse_github_slug(url: &str) -> Option<String>` from the body of `github_repo_from_remote` in `agentix-jails/src/jail/main.rs`; the new function parses `git@github.com:owner/repo[.git]` and `https://github.com/owner/repo[.git]` patterns and returns the `owner/repo` slug, or `None` for non-GitHub URLs

**Checkpoint**: `parse_github_slug` exists as a standalone function; `github_repo_from_remote` now delegates to it. `cargo test -p agentix-jails` compiles cleanly.

---

## Phase 3: User Story 1 — Fork + Upstream Workflow (Priority: P1) 🎯 MVP

**Goal**: claude-jail enumerates all git remotes and adds every GitHub repo to the gh proxy allowlist, so a developer with both `origin` (fork) and `upstream` (canonical) can run `gh` commands against either inside the jail.

**Independent Test**: In a repo with two GitHub remotes, start claude-jail and verify that `gh` commands targeting either remote succeed without allowlist errors.

### Implementation for User Story 1

- [x] T003 [US1] Implement `github_repos_from_all_remotes(cwd: &Path) -> Vec<String>` in `agentix-jails/src/jail/main.rs`: run `git -C <cwd> remote` to list remote names; for each name run `git -C <cwd> remote get-url <name>`; call `parse_github_slug` on each URL; collect into a `HashSet<String>` (dedup); return as `Vec<String>` in insertion order; on any git failure return empty `Vec`
- [x] T004 [US1] Replace the `github_repo_from_remote` call site in `main()` in `agentix-jails/src/jail/main.rs`: iterate `github_repos_from_all_remotes(&cwd)` and push each slug into `allowed_repos` if not already present; preserve the existing `args.allowed_repos` entries (explicit `--repo` flags) by merging them with deduplication; remove the old `github_repo_from_remote` function
- [x] T005 [US1] Add unit tests for `parse_github_slug` in `agentix-jails/src/jail/main.rs` covering: SSH standard (`git@github.com:user/repo.git` → `user/repo`), SSH without `.git` suffix, HTTPS standard (`https://github.com/user/repo.git` → `user/repo`), HTTPS without `.git` suffix

**Checkpoint**: `cargo test -p agentix-jails` passes. `github_repos_from_all_remotes` correctly discovers multiple GitHub remotes.

---

## Phase 4: User Story 2 — Non-GitHub Remote Ignored (Priority: P2)

**Goal**: Remote URLs that are not GitHub (GitLab, Gitea, etc.) are silently skipped; jail startup emits no errors for them.

**Independent Test**: Start claude-jail in a repo with one GitHub remote and one non-GitHub remote; confirm jail starts cleanly and only the GitHub repo appears in the allowlist.

### Implementation for User Story 2

- [x] T006 [US2] Add unit tests for `parse_github_slug` in `agentix-jails/src/jail/main.rs` covering: non-GitHub SSH (`git@gitlab.com:user/repo.git` → `None`), non-GitHub HTTPS (`https://example.com/repo.git` → `None`), malformed URL → `None`; confirm no panic on any input

**Note**: The silent-skip behaviour is already implemented by `parse_github_slug` returning `None` (T002/T003). This phase adds explicit test coverage to lock in that behaviour.

**Checkpoint**: All non-GitHub URL test cases pass. `cargo clippy -p agentix-jails -- -D warnings` is clean.

---

## Phase 5: User Story 3 — Explicit --repo Still Works (Priority: P3)

**Goal**: Repos added via `--repo owner/repo` on the command line are still included in the allowlist alongside auto-discovered remotes, with deduplication.

**Independent Test**: Start claude-jail with `--repo other-org/other-repo` where that repo is not a git remote; confirm `gh repo view other-org/other-repo` succeeds inside the jail.

### Implementation for User Story 3

- [x] T007 [US3] Add unit test in `agentix-jails/src/jail/main.rs` (or a helper test function) that verifies the deduplication logic: when the same slug appears both as a discovered remote and as an explicit `--repo` flag, it appears exactly once in the final `allowed_repos` list
- [x] T008 [US3] Verify the updated call site in `main()` (from T004) correctly handles the case where `args.allowed_repos` contains a slug also returned by `github_repos_from_all_remotes`; adjust if any duplicate can slip through

**Checkpoint**: All three user story paths (multi-remote, non-GitHub skipping, explicit --repo) are covered by passing unit tests.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Quality gates and documentation.

- [x] T009 Run `nix develop --command cargo fmt --all --check` and fix any formatting issues in `agentix-jails/src/jail/main.rs`
- [x] T010 Run `nix develop --command cargo clippy -- -D warnings` and fix any warnings in `agentix-jails/src/jail/main.rs`
- [x] T011 Run `nix develop --command cargo test --workspace` and confirm all tests pass
- [x] T012 Add a one-line comment in `ARCHITECTURE.md` under the `claude-jail` security note (Constitution IV) documenting that the gh proxy allowlist is now auto-populated from all GitHub remotes, not just `origin`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: No dependencies — start immediately
- **Phase 2 (Foundational)**: Depends on Phase 1 — blocks all user story phases
- **Phase 3 (US1)**: Depends on Phase 2
- **Phase 4 (US2)**: Depends on Phase 2 (the `parse_github_slug` function from T002); can proceed in parallel with Phase 3 once Phase 2 is done
- **Phase 5 (US3)**: Depends on Phase 3 (T004 — call site must exist before T007/T008 can validate it)
- **Phase 6 (Polish)**: Depends on all prior phases complete

### User Story Dependencies

- **US1 (P1)**: Depends on Foundational (T002)
- **US2 (P2)**: Depends on Foundational (T002); independent of US1
- **US3 (P3)**: Depends on US1 (T004) for the call-site dedup logic

### Within Each Phase

- T003 must precede T004 (function must exist before call site is updated)
- T005 and T006 can run in parallel (different test cases, same file section)
- T007 depends on T004 (tests the call site)

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Add import (T001)
2. Complete Phase 2: Extract `parse_github_slug` (T002)
3. Complete Phase 3: Implement `github_repos_from_all_remotes` + update call site + tests (T003–T005)
4. **STOP and VALIDATE**: confirm `cargo test -p agentix-jails` passes and multi-remote discovery works
5. Continue with US2/US3 test coverage (T006–T008) and polish (T009–T012)

### Parallel Opportunities

- T005 (US1 URL-parse tests) and T006 (US2 non-GitHub tests) can be written in parallel since they target different test cases in the same `#[cfg(test)]` module.

---

## Notes

- All changes are in `agentix-jails/src/jail/main.rs` — no other file needs modification except `ARCHITECTURE.md` (T012)
- `ax-jail` (`agentix-jails/src/ax_jail/main.rs`) is explicitly out of scope; do not touch it
- `gh-jail-server` (`agentix-jails/src/gh_proxy/server.rs`) is unchanged; its allowlist enforcement logic is already correct
- Commit message when done: `fix(jails): enumerate all GitHub remotes for gh proxy allowlist`
