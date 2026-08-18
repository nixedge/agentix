# Implementation Plan: Fix gh Jail to Allow All GitHub Remotes

**Branch**: `012-gh-jail-all-remotes` | **Date**: 2026-08-18 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/012-gh-jail-all-remotes/spec.md`

## Summary

Replace the single-remote `github_repo_from_remote` helper in `agentix-jails/src/jail/main.rs` with a `github_repos_from_all_remotes` function that enumerates every git remote and collects all GitHub slugs, so the gh proxy allowlist covers any remote the repo has — not just `origin`.

## Technical Context

**Language/Version**: Rust 1.80+ (fenix stable toolchain via Nix)
**Primary Dependencies**: `std::process::Command` (already in use), no new crate deps
**Storage**: N/A
**Testing**: `cargo test -p agentix-jails` (unit tests for the parsing function)
**Target Platform**: Linux (bubblewrap sandbox host)
**Project Type**: CLI binary (`claude-jail` bin in `agentix-jails`)
**Performance Goals**: Sub-second jail startup; git subprocess overhead is negligible
**Constraints**: No new crate dependencies; change confined to `agentix-jails/src/jail/main.rs`
**Scale/Scope**: Single function replacement + updated call site; ~30 lines changed

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | ✅ Pass | Change is in a binary crate (`agentix-jails`), not a library. Appropriate. |
| II. Local-First | ✅ N/A | This is sandbox tooling, not inference routing. |
| III. Reproducible Environments | ✅ Pass | No new deps; Nix build unaffected. `nix build .#claude-jail` must still succeed. |
| IV. Isolation by Default | ✅ Pass | Expands the gh proxy allowlist scope — does not reduce sandbox isolation, expose credentials, or add new mounts. The gh proxy already enforces allowlist policy; this change only widens which repos are auto-discovered. This is a security-neutral change that must be noted in code review per constitution. |
| V. Layered API | ✅ N/A | No API contract changes. |
| VI. Comprehensive Testing | ✅ Pass | New parsing function must have unit tests covering SSH URLs, HTTPS URLs, non-GitHub URLs, and deduplication. |
| VII. Formal Agent State Machine | ✅ N/A | No agent loop changes. |
| VIII. Code Quality Gates | ✅ Pass | `cargo fmt`, `cargo clippy -D warnings`, `cargo test --workspace` must all pass. |

**Post-design re-check**: No new violations introduced.

## Project Structure

### Documentation (this feature)

```text
specs/012-gh-jail-all-remotes/
├── plan.md              # This file
├── research.md          # Phase 0 output
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (affected files only)

```text
agentix-jails/src/jail/main.rs   # Only file changed
```

No new files, no new crates, no schema changes.

## Phase 0: Research

### Decision Log

**How to enumerate all remotes**

- **Decision**: Run `git -C <cwd> remote` to get newline-separated remote names, then for each name run `git -C <cwd> remote get-url <name>` to get the fetch URL.
- **Rationale**: `git remote get-url` returns exactly one URL per remote (the fetch URL), which is what we want. `git remote -v` would also work but requires parsing the extra `(fetch)`/`(push)` suffix and gives duplicate lines.
- **Alternatives considered**: `git remote -v` (more subprocess output to parse); reading `.git/config` directly (fragile, bypasses git's own resolution). The `get-url` approach matches the pattern already used in the existing `github_repo_from_remote`.

**Deduplication strategy**

- **Decision**: Use a `HashSet<String>` during collection, then convert to `Vec<String>` preserving insertion order (i.e., first-seen wins).
- **Rationale**: Simple, idiomatic Rust. Two remotes pointing at the same GitHub repo (e.g., one SSH, one HTTPS) normalise to the same slug and collapse to one entry.
- **Alternatives considered**: Post-collection dedup with `Vec::dedup` (requires sorting, destroys ordering); checking `contains` in a loop (O(n²), fine for small lists but less idiomatic).

**Error handling**

- **Decision**: If `git remote` fails (not a git repo, git not available), return an empty `Vec` and continue. Per FR-007.
- **Rationale**: Jail startup must not be aborted by a missing git repo. The `--repo` explicit flag is the fallback.

**Return type change**

- **Decision**: Replace `github_repo_from_remote(cwd: &Path) -> Option<String>` with `github_repos_from_all_remotes(cwd: &Path) -> Vec<String>`.
- **Rationale**: Returns zero or more repos; `Vec` is the natural type. The call site iterates and deduplicates against the explicit `--repo` list.

## Phase 1: Design

### Function Signature & Logic

```
github_repos_from_all_remotes(cwd: &Path) -> Vec<String>
  1. run: git -C <cwd> remote
     → on failure: return []
  2. split stdout on newlines → list of remote names (filter empty lines)
  3. for each name:
       run: git -C <cwd> remote get-url <name>
       → on failure: skip this remote (continue)
       url = stdout.trim()
       slug = parse_github_slug(url)  // extract from SSH or HTTPS pattern
       if slug is Some(s): insert into HashSet
  4. return HashSet in insertion order as Vec<String>
```

`parse_github_slug(url: &str) -> Option<String>` — extracted from the existing `github_repo_from_remote` body, no logic changes.

### Updated Call Site

```
// Build the allowed-repos list: all GitHub remotes + any --repo flags.
let mut seen: HashSet<String> = HashSet::new();
let mut allowed_repos: Vec<String> = Vec::new();

// Discovered remotes first (preserves existing "origin first" spirit)
for repo in github_repos_from_all_remotes(&cwd) {
    if seen.insert(repo.clone()) {
        allowed_repos.push(repo);
    }
}
// Explicit --repo flags appended, deduped
for repo in &args.allowed_repos {
    if seen.insert(repo.clone()) {
        allowed_repos.push(repo.clone());
    }
}
```

### Tests Required

New unit tests in `agentix-jails/src/jail/main.rs` (or a test module therein):

| Test | Input URL | Expected slug |
|------|-----------|---------------|
| SSH standard | `git@github.com:user/repo.git` | `user/repo` |
| SSH no .git suffix | `git@github.com:user/repo` | `user/repo` |
| HTTPS standard | `https://github.com/user/repo.git` | `user/repo` |
| HTTPS no .git suffix | `https://github.com/user/repo` | `user/repo` |
| Non-GitHub SSH | `git@gitlab.com:user/repo.git` | `None` |
| Non-GitHub HTTPS | `https://example.com/repo.git` | `None` |
| Dedup | Two URLs → same slug | Appears once |

### No Contracts or Data Model

This feature has no external API surface and no data entities. `contracts/` and `data-model.md` are not applicable.

## Complexity Tracking

*No constitution violations — table omitted.*
