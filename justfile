# Agentic RAG stack

# List available commands
default:
    @just --list

# Start Ollama and the gateway
dev:
    nix run .#dev

# Import an explicitly exported legacy index into a fresh Lance directory
import-postgres export destination model:
    cargo run --release -p agentix-indexer --bin ingest -- import "{{export}}" --destination "{{destination}}" --model "{{model}}"

# ── Code + doc indexing ───────────────────────────────────────────────────────

# Index a codebase and its markdown docs (usage: just index /path/to/repo)
# Incremental: unchanged files are skipped automatically.
index path:
    cargo run --release -p agentix-indexer --bin ingest -- code "{{path}}"

# Re-index, clearing existing chunks and documents first
reindex path:
    cargo run --release -p agentix-indexer --bin ingest -- code "{{path}}" --force

# Index code only, skipping markdown docs
index-code path:
    cargo run --release -p agentix-indexer --bin ingest -- code "{{path}}" --no-docs

# Index markdown docs only (standalone, does not touch code_chunks)
index-docs path:
    cargo run --release -p agentix-indexer --bin ingest -- docs "{{path}}"

# Re-index docs only
reindex-docs path:
    cargo run --release -p agentix-indexer --bin ingest -- docs "{{path}}" --force

# ── Haskell package indexing ──────────────────────────────────────────────────

# Fetch and index a Haskell package from CHaP or Hackage (usage: just index-hackage serialise 0.2.6.1)
# Checks CHaP first, falls back to Hackage. Skips if already indexed.
index-hackage package version:
    cargo run --release -p agentix-indexer --bin ingest -- hackage "{{package}}" "{{version}}"

# Re-index a Haskell package, clearing existing chunks first
reindex-hackage package version:
    cargo run --release -p agentix-indexer --bin ingest -- hackage "{{package}}" "{{version}}" --force

# ── Rust crate indexing ───────────────────────────────────────────────────────

# Fetch and index a Rust crate from crates.io (usage: just index-crate tokio 1.0.0)
# Skips if already indexed.
index-crate package version:
    cargo run --release -p agentix-indexer --bin ingest -- crate "{{package}}" "{{version}}"

# Re-index a Rust crate, clearing existing chunks first
reindex-crate package version:
    cargo run --release -p agentix-indexer --bin ingest -- crate "{{package}}" "{{version}}" --force

# ── Git repo indexing ─────────────────────────────────────────────────────────

# Clone and index a git repo at HEAD (usage: just index-git https://github.com/owner/repo)
index-git url:
    cargo run --release -p agentix-indexer --bin ingest -- git "{{url}}"

# Clone and index a git repo at a specific branch (usage: just index-git-branch <url> <branch>)
index-git-branch url branch:
    cargo run --release -p agentix-indexer --bin ingest -- git "{{url}}" --branch "{{branch}}"

# Clone and index a git repo at a specific tag (usage: just index-git-tag <url> <tag>)
index-git-tag url tag:
    cargo run --release -p agentix-indexer --bin ingest -- git "{{url}}" --tag "{{tag}}"

# Clone and index a git repo at a specific commit hash (full clone)
index-git-rev url rev:
    cargo run --release -p agentix-indexer --bin ingest -- git "{{url}}" --rev "{{rev}}"

# ── GitHub indexing ───────────────────────────────────────────────────────────

# Index GitHub issues + PRs for a repo (usage: just index-github OWNER/REPO)
# Incremental: only fetches items updated since the last sync watermark.
# Set GITHUB_TOKEN env var for authenticated requests (5000 req/hr).
index-github repo:
    cargo run --release -p agentix-indexer --bin ingest -- github "{{repo}}"

# Re-fetch all GitHub items, ignoring watermarks
reindex-github repo:
    cargo run --release -p agentix-indexer --bin ingest -- github "{{repo}}" --force

# Index only issues (skips PRs and comments)
index-github-issues repo:
    cargo run --release -p agentix-indexer --bin ingest -- github "{{repo}}" --stream issues

# Index only PRs
index-github-prs repo:
    cargo run --release -p agentix-indexer --bin ingest -- github "{{repo}}" --stream prs

# ── Repo index ────────────────────────────────────────────────────────────────

# List all indexed repos
list:
    cargo run --release -p agentix-indexer --bin ingest -- list

# List only local clone repos
list-local:
    cargo run --release -p agentix-indexer --bin ingest -- list --local

# Prune all local clone repos (shows what would be deleted first — use prune-local to actually delete)
prune-local-dry:
    cargo run --release -p agentix-indexer --bin ingest -- prune --dry-run local

# Delete all local clone repos from the index
prune-local:
    cargo run --release -p agentix-indexer --bin ingest -- prune local

# For each package, delete all but the most recently indexed version (dry run)
prune-old-dry:
    cargo run --release -p agentix-indexer --bin ingest -- prune --dry-run old-versions

# For each package, delete all but the most recently indexed version
prune-old:
    cargo run --release -p agentix-indexer --bin ingest -- prune old-versions

# Delete a specific repo from the index (usage: just prune-repo 'hackage::serialise-0.2.6.1')
prune-repo repo:
    cargo run --release -p agentix-indexer --bin ingest -- prune repo "{{repo}}"

# ── MCP server + CLI ──────────────────────────────────────────────────────────

# Build both binaries
build:
    cargo build --release -p agentix-indexer -p agentix-mcp-server

# Start the MCP server (Claude Code launches this automatically)
mcp:
    cargo run --release -p agentix-mcp-server --bin mcp-server

# Hybrid search over indexed code (usage: just search "query")
search query:
    cargo run --release -p agentix-mcp-server --bin mcp-server -- search "{{query}}"

# BM25-only search (usage: just bm25 "MyFunctionName")
bm25 query:
    cargo run --release -p agentix-mcp-server --bin mcp-server -- bm25 "{{query}}"

# ── Local search index ──────────────────────────────────────────────────────

# Show indexed counts across every source collection
stats:
    cargo run --release -p agentix-indexer --bin ingest -- stats

# Show persisted GitHub watermarks and errors
sync-status:
    cargo run --release -p agentix-indexer --bin ingest -- sync-status

# Rebuild FTS, scalar and HNSW indexes after a large incremental ingest or import
optimize:
    cargo run --release -p agentix-indexer --bin ingest -- optimize
