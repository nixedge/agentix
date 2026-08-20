# agentix Development Guidelines

Auto-generated from feature plans and updated manually. Last updated: 2026-08-20

## Active Technologies
- Rust edition 2021, Rust 1.80+ (via fenix stable toolchain in Nix)
- llama-cpp-2 (C++, isolated in `agentix-llama`)
- whisper-rs / whisper.cpp (C++, isolated in `agentix-whisper`); audio decoding via symphonia + rubato
- tree-sitter-* (C++, in `agentix-indexer`)
- fastembed/onnxruntime (C++, in `agentix-search`)
- tokio, axum, sqlx, rmcp, ratatui
- PostgreSQL 17 + pg_search (BM25) + pgvector (HNSW)
- Nix flakes (build + services)
- Rust 2021, Rust 1.80+ (fenix stable via Nix) + `llama-cpp-2 = "0.1"` (add `common` feature), `axum 0.8`, `agentix-api`, `agentix-infer` (013-grammar-responses-api)
- N/A (stateless per request) (013-grammar-responses-api)

## Project Structure

```text
agentix-api/         # OpenAI-compatible request/response types (no deps)
agentix-router/      # Backend-selection routing (RouteTarget enum); NO C++ deps
agentix-infer/       # Pure-Rust inference traits, types, and ModelStore
agentix-llama/       # llama-cpp-2 GGUF backend (C++; isolated here)
agentix-whisper/     # whisper.cpp speech-to-text backend (C++; isolated here)
agentix-daemon/      # HTTP gateway (Axum); assembles api + router + infer + llama + whisper
agentix-harness/     # Agent loop library (state machine, tool dispatch)
agentix-ax/          # TUI agent binary (Ratatui, links harness)
agentix-search/      # PostgreSQL search library (BM25 + vector + reranking)
agentix-indexer/     # Repo ingestion pipeline (tree-sitter + embed); bin: ingest
agentix-mcp-server/  # MCP stdio server exposing search tools; bin: mcp-server
src/                 # Jail binaries (claude-jail, ax-jail, gh-jail-*)
```

Dependency flow: `agentix-api` → `agentix-router` → `agentix-daemon`. C++ is isolated:
`agentix-llama` (llama-cpp-2), `agentix-whisper` (whisper.cpp + symphonia), `agentix-indexer` (tree-sitter), `agentix-search` (fastembed).
Neither `agentix-router` nor `agentix-infer` have C++ transitive deps. No circular deps.
The daemon is the only crate that binds a port. The daemon's `build.rs` adds
`--allow-multiple-definition` because llama.cpp and whisper.cpp both bundle ggml.

## Commands

```bash
# Build everything
nix develop --command cargo build --workspace

# Test a specific crate
nix develop --command cargo test -p agentix-infer

# Run all tests
nix develop --command cargo test --workspace

# Format check
nix develop --command cargo fmt --all --check

# Clippy (fails on warnings; unwrap_used + expect_used are enabled)
nix develop --command cargo clippy -- -D warnings

# Start dev services (PostgreSQL + Ollama)
nix run .#dev

# Run gateway
nix develop --command agentix-daemon
```

## Code Style

- Conventional Commits: `feat:`, `fix:`, `feat!:`, `chore:`, `docs:`, `test:`
- `clippy::unwrap_used` and `clippy::expect_used` are denied workspace-wide — use `?` or explicit error handling
- `unsafe` blocks require a `// SAFETY:` comment explaining the invariant
- No blocking calls on the Tokio runtime — use `tokio::task::spawn_blocking` for C FFI (llama.cpp)
- Comments only when the WHY is non-obvious; no docstring blocks for obvious functions

## Active Feature: 001-agentix-infer

**Branch**: `001-agentix-infer`
**Plan**: `specs/001-agentix-infer/plan.md`

Building `agentix-infer`: in-process GGUF inference replacing the Ollama HTTP proxy.

Key constraints:
- Pure library crate — no network surface
- Integration tests MUST use a small (<50MB) fixture GGUF pinned in Nix, not jina-code (1.5GB)
- All llama.cpp C FFI via `spawn_blocking`
- Ollama-compatible blob layout so existing model dirs are usable

<!-- MANUAL ADDITIONS START -->
<!-- MANUAL ADDITIONS END -->

## Recent Changes
- 013-grammar-responses-api: Added Rust 2021, Rust 1.80+ (fenix stable via Nix) + `llama-cpp-2 = "0.1"` (add `common` feature), `axum 0.8`, `agentix-api`, `agentix-infer`
- 011-source-filters: Per-package scoped source trees (mkWorkspaceSrc); jail binaries moved to `agentix-jails` workspace member; root Cargo.toml is now workspace-only
- 007-cargo-cleanup: Decomposed workspace — llama-cpp-2 isolated in `agentix-llama`; new crates: `agentix-search`, `agentix-indexer`, `agentix-mcp-server`; root crate renamed to `agentix-jails`; pure-Rust GGUF metadata parser added to `agentix-infer`
