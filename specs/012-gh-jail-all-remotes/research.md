# Research: Fix gh Jail to Allow All GitHub Remotes

**Branch**: `012-gh-jail-all-remotes` | **Date**: 2026-08-18

## Decisions

### Remote Enumeration Method

- **Decision**: `git remote` → names, then `git remote get-url <name>` per name
- **Rationale**: Matches the existing pattern in the codebase; returns the canonical fetch URL without extra parsing overhead
- **Alternatives considered**: `git remote -v` (noisier output); direct `.git/config` parsing (fragile)

### Deduplication

- **Decision**: `HashSet<String>` during collection, `Vec<String>` for output
- **Rationale**: Idiomatic; handles SSH+HTTPS aliases pointing to the same repo cleanly
- **Alternatives considered**: `Vec::dedup` after sort (loses insertion order)

### Error Handling

- **Decision**: Return empty `Vec` on any git failure; never abort jail startup
- **Rationale**: FR-007; the jail is usable without git remotes (explicit `--repo` still works)

### Scope

- **Decision**: Change confined to `github_repo_from_remote` → `github_repos_from_all_remotes` in `agentix-jails/src/jail/main.rs`
- **Rationale**: ax-jail has no gh proxy; gh-jail-server allowlist logic is already correct; only the discovery step needs updating
