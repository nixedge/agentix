# Feature Specification: Fix gh Jail to Allow All GitHub Remotes

**Feature Branch**: `012-gh-jail-all-remotes`
**Created**: 2026-08-18
**Status**: Draft
**Input**: User description: "Fix gh-jail-server to allow gh CLI access for any GitHub remote, not just origin. Currently, claude-jail only adds the origin remote's repo to the gh proxy allowlist. Any other remotes (forks, upstreams, etc.) are blocked. The fix should enumerate all git remotes in the cwd, extract any that are GitHub repos (git@github.com: or https://github.com/), and add them all to the allowed-repos list passed to gh-jail-server. The --repo flag should still work for explicitly adding repos not present as remotes. ax-jail does not use gh proxy so no changes needed there."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Fork + Upstream Workflow (Priority: P1)

A developer working in a repository with both an `origin` remote (their fork) and an `upstream` remote (the canonical repo) needs to run `gh` commands against both repos inside the jail. Currently only `origin` is in the allowlist, so commands targeting `upstream` are rejected.

**Why this priority**: This is the most common multi-remote setup and the primary pain point — it blocks normal open-source contribution workflows.

**Independent Test**: Start claude-jail in a repo with two GitHub remotes (`origin` = fork, `upstream` = canonical). Run `gh pr list --repo upstream-owner/upstream-repo` inside the jail — it should succeed without an allowlist error.

**Acceptance Scenarios**:

1. **Given** a repo with `origin = git@github.com:user/fork.git` and `upstream = https://github.com/org/canonical.git`, **When** claude-jail starts, **Then** both `user/fork` and `org/canonical` are included in the gh proxy allowlist.
2. **Given** the allowlist includes both repos, **When** `gh pr list --repo org/canonical` runs inside the jail, **Then** it executes successfully and returns results.
3. **Given** the allowlist includes both repos, **When** `gh issue view 123` (no explicit --repo, defaults to origin) runs inside the jail, **Then** it executes successfully.

---

### User Story 2 - Non-GitHub Remote Ignored (Priority: P2)

A developer has a mix of GitHub and non-GitHub remotes (e.g., a self-hosted GitLab or Gitea instance). Non-GitHub remotes should not cause errors or appear in the allowlist.

**Why this priority**: Required for correctness but not the primary user need; non-GitHub remotes are common in enterprise environments.

**Independent Test**: Start claude-jail in a repo where one remote is a GitHub URL and another is a non-GitHub URL. Verify jail starts cleanly and only the GitHub repo is in the allowlist.

**Acceptance Scenarios**:

1. **Given** a repo with `origin = git@github.com:user/repo.git` and `mirror = git@gitlab.example.com:user/repo.git`, **When** claude-jail starts, **Then** only `user/repo` is in the allowlist and no error is emitted for the GitLab remote.
2. **Given** a repo with no GitHub remotes at all, **When** claude-jail starts, **Then** the allowlist contains only entries from `--repo` flags (or is unrestricted if none given) and no error is emitted.

---

### User Story 3 - Explicit --repo Still Works (Priority: P3)

A developer needs to access a GitHub repo that is not listed as a remote (e.g., a dependency repo they want to inspect). The explicit `--repo` flag must still add it to the allowlist on top of auto-discovered remotes.

**Why this priority**: Preserves existing functionality; users may rely on `--repo` for repos they haven't added as git remotes.

**Independent Test**: Start claude-jail with `--repo other-org/other-repo` in a repo that does not have that repo as a remote. Confirm `gh repo view other-org/other-repo` succeeds inside the jail.

**Acceptance Scenarios**:

1. **Given** `--repo other-org/other-repo` is passed to claude-jail, **When** `gh repo view other-org/other-repo` runs inside the jail, **Then** it succeeds regardless of whether that repo appears as a git remote.
2. **Given** both auto-discovered remotes and explicit `--repo` flags, **When** the allowlist is built, **Then** all entries are deduplicated (no duplicate owner/repo entries).

---

### Edge Cases

- What happens when a remote URL is malformed or uses an unsupported scheme? → Silently skip it; do not abort jail startup.
- What happens when the cwd is not a git repository? → No repos auto-added; allowlist contains only explicit `--repo` entries (or is unrestricted if none given).
- What happens when the `git remote` command fails? → Treat as empty remote list; continue jail startup normally.
- What if two remotes point to the same GitHub repo (e.g., one via SSH and one via HTTPS)? → Deduplicate; add each slug only once.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: claude-jail MUST enumerate all configured git remotes in the working directory at startup.
- **FR-002**: claude-jail MUST parse each remote URL and extract the `owner/repo` slug for any URL matching the GitHub SSH pattern (`git@github.com:owner/repo[.git]`) or GitHub HTTPS pattern (`https://github.com/owner/repo[.git]`).
- **FR-003**: claude-jail MUST add every discovered GitHub slug to the gh proxy allowed-repos list.
- **FR-004**: The allowed-repos list MUST be deduplicated so each `owner/repo` slug appears at most once, regardless of how many remotes resolve to it.
- **FR-005**: Explicit `--repo owner/repo` flags passed to claude-jail MUST continue to supplement the auto-discovered repos in the allowlist.
- **FR-006**: Remote URLs that do not match a recognised GitHub pattern MUST be silently skipped (no warning, no abort).
- **FR-007**: Failure to list remotes (e.g., not in a git repo, git command error) MUST NOT abort jail startup; it MUST be treated as an empty remote list.
- **FR-008**: ax-jail MUST NOT be modified; it does not use a gh proxy.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A developer with two GitHub remotes can run `gh` commands targeting either repo inside the jail without allowlist errors.
- **SC-002**: Adding a new GitHub remote to a repo and restarting the jail is sufficient to grant gh access to that remote's repo — no `--repo` flag required.
- **SC-003**: Jails started in repos with non-GitHub remotes start cleanly with no error output related to remote parsing.
- **SC-004**: All existing `--repo` flag behaviour is preserved; no regressions in the explicit-allowlist path.

## Assumptions

- GitHub remote URLs follow exactly two patterns: SSH (`git@github.com:owner/repo[.git]`) and HTTPS (`https://github.com/owner/repo[.git]`). Other GitHub URL variants (e.g., `ssh://git@github.com/`) are out of scope.
- `git remote -v` (or equivalent) is the canonical mechanism for listing remotes; no other discovery mechanism is needed.
- ax-jail has no gh proxy and is entirely out of scope for this feature.
- The change is confined to `agentix-jails/src/jail/main.rs`; no other crates require modification.
