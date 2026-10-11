# agentix development guidance

Read [CLAUDE.md](CLAUDE.md) for existing repository conventions; current instructions and this file take precedence over historical architecture/toolchain statements there.

- Use the pinned Nix dev shell for every Cargo command. Verified compiler: Rust 1.94.1. Agentix uses edition 2021; the vendored patch core and pinned Codex dependencies use edition 2024. Older compilers are not validated.
- Use Spec Kit and continue [020 tasks](specs/020-ax-coding-agent/tasks.md) in dependency order. Record test evidence in the feature quickstart; unchecked tasks remain incomplete.
- Keep inference/queue mechanics in libraries and public network listeners in the daemon. No SQLite or C++ dependencies in the harness/router/infer.
- Codex PTY, file-search and execpolicy are Git dependencies pinned to 092d3acd6bec3e3a14bdc7e7a2810ab628ab759d. Patch parser/text transforms are extracted in vendor/codex-apply-patch; retain its provenance, notices and upstream tests.
- The user explicitly chose unchanged upstream filename-search symlink behavior. Do not claim its starting root provides containment; jail mounts determine accessibility. This does not relax read/edit approval or path checks.
- Use agentic-nix first for code investigation. Never scan /nix/store.
- Use conventional commits only when committing is authorized; no automatic pushes, merges or commits to master.
- Keep workspace Clippy panic lints enabled. Upstream test-only allowances must remain scoped to fixture assertions.

Validation:

```bash
nix develop --command cargo test -p agentix-harness -p agentix-codex-patch
nix develop --command cargo test -p agentix-infer --test generation_fixture
nix develop --command cargo clippy -p agentix-harness -p agentix-codex-patch --all-targets -- -D warnings
nix build .#ax --no-link
```

The dev shell provides build tools without building ax on entry. Run/build ax-jail explicitly. The generation fixture uses AGENTIX_TEST_GENERATION_MODEL_PATH; AGENTIX_TEST_MODEL_PATH remains the existing embedding fixture.
