# agentic-nix

Hybrid code search for Claude. Index your codebases, documentation, and GitHub issues into
PostgreSQL (ParadeDB BM25 + pgvector HNSW). A Rust MCP server exposes the index to Claude Code
so it can answer questions about your code without reading every file from scratch.

**What this replaces:** Claude reading 50 files one by one to find something. Instead, Claude
queries the index with a single tool call and gets the 10 most relevant chunks in under a second.

---

## How it works

```
your repos ──► ingest ──► PostgreSQL (ParadeDB)
                               │
                          BM25 + vector          Ollama
                          hybrid search    ◄──  embeddings
                               │
                          MCP server ──► Claude Code
```

1. The `ingest` binary walks your repos, extracts named symbols via tree-sitter, embeds each
   chunk via Ollama, and stores everything in PostgreSQL.
2. The `mcp-server` binary sits between Claude and the database, exposing search tools over
   the MCP protocol (stdio).
3. Claude Code connects to the MCP server automatically and calls the tools when it needs
   to understand code.

---

## Prerequisites

- [Nix](https://nixos.org/download/) with flakes enabled
- Git

That's it. PostgreSQL, Ollama, Rust, and all other tools are managed by Nix.

**Enable flakes** if you haven't already — add to `~/.config/nix/nix.conf`:
```
experimental-features = nix-command flakes
```

> **On Ubuntu or other non-NixOS systems?** `nix run .#dev` won't use your GPU — CUDA library
> paths don't resolve correctly for Nix-packaged Ollama outside NixOS. See
> [Non-NixOS / Ubuntu setup](#non-nixos--ubuntu-setup) below for the system-services alternative.

---

## Non-NixOS / Ubuntu setup

On Ubuntu, use system-managed PostgreSQL and Ollama. The Nix dev shell still provides the Rust
toolchain, `just`, and all build tools — only the services differ.

### 1. Install PostgreSQL 17 with ParadeDB and pgvector

Add the PostgreSQL apt repo if you haven't already:

```bash
sudo apt install -y postgresql-common
sudo /usr/share/postgresql-common/pgdg/apt.postgresql.org.sh
```

Install [pg_search](https://docs.paradedb.com/documentation/getting-started/self-hosted) (ParadeDB BM25) and pgvector:

```bash
# ParadeDB repo
curl -fsSL https://apt.fury.io/paradedb/gpg.key \
  | sudo gpg --dearmor -o /usr/share/keyrings/paradedb.gpg
echo "deb [signed-by=/usr/share/keyrings/paradedb.gpg] https://apt.fury.io/paradedb/ stable main" \
  | sudo tee /etc/apt/sources.list.d/paradedb.list

sudo apt update
sudo apt install -y postgresql-17-pg-search postgresql-17-pgvector
```

Enable `pg_search` in PostgreSQL — add to `/etc/postgresql/17/main/postgresql.conf`:

```
shared_preload_libraries = 'pg_search'
pg_search.enable_telemetry = off
```

Restart PostgreSQL:

```bash
sudo systemctl restart postgresql
```

Create the database and apply the schema:

```bash
sudo -u postgres createdb codebase
sudo -u postgres psql codebase -c \
  "CREATE EXTENSION IF NOT EXISTS pg_search; CREATE EXTENSION IF NOT EXISTS vector;"

# Allow your user to connect (adjust if you use password auth instead)
sudo -u postgres psql -c "GRANT ALL PRIVILEGES ON DATABASE codebase TO $USER;"
sudo -u postgres psql codebase -c "GRANT ALL ON SCHEMA public TO $USER;"

# Enter the Nix dev shell, then apply the schema
nix develop
just migrate
```

### 2. Install Ollama

The official installer detects CUDA automatically:

```bash
curl -fsSL https://ollama.com/install.sh | sh
```

Pull the embedding model:

```bash
ollama pull hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0
```

Ollama runs as a systemd service and starts on boot. Verify GPU usage with `ollama ps` after
pulling a model — it should show `GPU` in the processor column.

### 3. Continue with the normal setup

Skip the `nix run .#dev` step — your services are already running. Jump straight to
[Build the Rust binaries](#2-build-the-rust-binaries) and continue from there. Everything
else (indexing, MCP registration, Claude integration) is identical.

---

## First-time setup

### 1. Clone and enter the dev shell

```bash
git clone <this-repo> ~/agentic-nix
cd ~/agentic-nix
nix develop
```

The first `nix develop` downloads Rust, PostgreSQL, Ollama, and all dependencies. This takes
a few minutes once; subsequent shells are instant.

### 2. Build the Rust binaries

Use `nix develop` for the pinned Rust 1.94.1 toolchain. Agentix uses edition 2021,
while the Codex libraries and extracted patch core use edition 2024; older Rust
versions are not validated. The dev shell provides tools without building ax on
entry. Build or run the launcher explicitly with `nix build .#ax-jail` or
`nix run .#ax-jail`. Upstream pins and licenses are recorded in [licenses](licenses/README.md).

Feature 020's API/daemon 0.2 changes are described in the
[migration guide](docs/api-0.2-migration.md), including implementation status.
The [coding checkpoint guide](specs/020-ax-coding-agent/quickstart.md#coding-checkpoint-validation--2026-10-11-continuation)
records tested behavior, remaining issues and commands for running the matching
worker, daemon and jailed terminal agent with an installed local model.

Set `AGENTIX_GATEWAY_URL` to your agentix server (for example,
`http://ai.lan.disasm.us:11434`) and launch `ax-jail` from the project directory.
Without an explicit `--model` or `AGENTIX_MODEL`, the installed-model picker opens
automatically; `/model` reopens it between turns. Type to filter, use arrows and
Enter to load/select, and Esc to close. This requires the matching 0.2 daemon and
worker. `ax --list-models` lists the catalog without launching coding tools.

```bash
just build
```

This produces `target/release/mcp-server` and `target/release/ingest`. You only need to
rebuild when the Rust source changes.

### 3. Start the services

In a dedicated terminal (keep it running):

```bash
nix run .#dev
```

This starts:
- **PostgreSQL 17** on `localhost:5432` with `pg_search` (BM25) and `pgvector` (HNSW) loaded.
  The `codebase` database is created automatically with the full schema.
- **Ollama** on `localhost:11434` with the `jina-code-embeddings-1.5b` model pulled.
  First start pulls the model (~1.5 GB); subsequent starts are instant.

Data is stored under `./data/` relative to where you run the command, so always run it from
the repo root.

### 4. Verify everything is running

```bash
# Should return a connection
just psql

# Should return empty tables (schema applied automatically)
just stats
```

---

## Indexing your code

### Index a codebase

```bash
just index /path/to/your/repo
```

This walks the repo, extracts symbols for TypeScript, JavaScript, Python, Rust, and Haskell
files using tree-sitter (functions, classes, structs, etc.), and falls back to overlapping
line windows for other file types. Files that haven't changed since the last run are skipped.

Re-index everything from scratch:
```bash
just reindex /path/to/your/repo
```

Index multiple repos — just run the command once per repo:
```bash
just index ~/work/frontend
just index ~/work/backend
just index ~/work/infrastructure
```

### Index documentation

Discovers `AGENTS.md`, `CLAUDE.md`, `README.md`, and any `.agent/workflows/`, `.agent/skills/`,
`.agent/plans/`, `.agent/SOPs/` markdown files:

```bash
just index-docs /path/to/your/repo
```

### Index GitHub issues and pull requests

```bash
export GITHUB_TOKEN=ghp_...   # optional but recommended (5000 req/hr vs 60)
just index-github anthropics/claude-code
```

This fetches all issues, PRs, and their comments. Subsequent runs are incremental — only items
updated since the last sync are fetched.

Index a specific stream only:
```bash
just index-github-issues anthropics/claude-code   # issues only
just index-github-prs    anthropics/claude-code   # PRs only
```

### Check what's indexed

```bash
just stats          # row counts per table
just sync-status    # GitHub watermarks (last sync times)
```

---

## Connect to Claude Code

The MCP server communicates with Claude Code over stdio. You need to register it once.

### Add the MCP server

Run this from the `~/agentic-nix` directory:

```bash
claude mcp add agentic-nix \
  --command "$(pwd)/target/release/mcp-server" \
  --env PG_DSN=postgresql://127.0.0.1:5432/codebase \
  --env OLLAMA_HOST=http://127.0.0.1:11434
```

Or add it manually to your Claude Code config (`~/.claude.json` or project `.claude/mcp.json`):

```json
{
  "mcpServers": {
    "agentic-nix": {
      "command": "/home/you/agentic-nix/target/release/mcp-server",
      "env": {
        "PG_DSN": "postgresql://127.0.0.1:5432/codebase",
        "OLLAMA_HOST": "http://127.0.0.1:11434"
      }
    }
  }
}
```

### Verify the connection

Start a new Claude Code session and ask:

```
List all indexed repositories.
```

Claude should call `list_repos` and return the repos you've indexed. If it can't connect,
check that the services are running (`nix run .#dev`) and the binary path is correct.

---

## codex-jail

`codex-jail` runs Codex with the same Nix, worktree and GitHub proxy
integration as the Claude launcher. The host runs the real GitHub CLI; the
jail's `gh` is the socket client, with no host GitHub credentials mounted.

```bash
nix run .#codex-jail -- --dangerous
nix run .#codex-jail -- --repo owner/another-repo --write
nix run .#codex-jail -- --no-github-auth -- exec "Inspect this project"
```

GitHub remotes supply the default repository list; `--repo OWNER/REPO`
adds entries, and `--all-repos` explicitly removes that restriction.
Without GitHub remotes or explicit repository selection, the proxy is disabled.
`--write` enables the shared proxy's mutating API/release operations.
`--no-github-auth` skips the proxy. The packaged client remains installed,
but cannot operate without its socket.

`--allow-ssh` explicitly forwards the host SSH agent; it is off by default.
`--debug` reports launcher arguments while redacting environment values and
omitting the Codex prompt. `--ro`, `--rw`, and `--all-worktrees` control
additional mounts. Codex arguments follow a separate `--`.

The jail uses an isolated global Git config with the host's name/email,
signing disabled, and GitHub SSH remotes rewritten to HTTPS. It does not
import host credential helpers. Private Git-over-HTTPS authentication is
not provided: the shared proxy blocks `gh auth`, so it cannot serve as
a Git credential helper. Explicit SSH URLs can use `--allow-ssh`.
The shared proxy's existing policy is unchanged by the Codex integration.

Launcher regression tests use the real proxy server and client with fixture
executables for bubblewrap and upstream GitHub access:

```bash
nix develop --command cargo test -p agentix-jails --bin codex-jail --test codex_jail
nix build .#codex-jail
```

## claude-jail

`claude-jail` runs Claude Code inside a [bubblewrap](https://github.com/containers/bubblewrap)
sandbox, giving it read-write access to your project while blocking access to the rest of your
home directory, SSH keys, and host credentials.

### Usage

```bash
nix run .#claude-jail [options]
```

Flags for `claude` itself go after `--`:

```bash
nix run .#claude-jail -- --dangerously-skip-permissions
nix run .#claude-jail --dangerous   # shorthand for the above
```

### Flags

| Flag | Description |
|---|---|
| `--dangerous` | Pass `--dangerously-skip-permissions` to Claude |
| `--write` | Allow mutating GitHub operations via the gh proxy (releases, `api POST/PATCH/DELETE`) |
| `--repo OWNER/REPO` | Add an extra GitHub repo the proxy may access. Repeatable. |
| `--no-github-auth` | Skip the gh proxy; no `gh` available inside the jail |
| `--allow-ssh` | Forward the host SSH agent socket into the jail. **Warning:** Claude can sign arbitrary SSH operations with your keys. Never combine with `--dangerous`. |
| `--ro PATH` | Bind an extra path read-only at its real path. Repeatable. |
| `--rw PATH` | Bind an extra path read-write at its real path. Repeatable. |
| `--debug` | Print every host command and the full bwrap argument list before launching |

### What gets mounted

Network access is shared with the host. Everything else is constructed from scratch:

| Path inside jail | Source | Access |
|---|---|---|
| `/nix` | host | read-only |
| `/proc`, `/dev`, `/tmp` | synthetic | — |
| `~/bin` | `$CLAUDE_JAIL_BIN_DIR` | read-only |
| `~/.claude` | host `~/.claude` | read-write |
| `~/.claude.json` | host `~/.claude.json` | read-write |
| `~/.ssh/known_hosts` | host | read-only |
| SSH agent socket | host (`--allow-ssh` only) | read-write |
| `/tmp/gitconfig` | synthetic (host identity) | read-only |
| project worktree | host worktree root | read-write |
| bare git common dir | host `.bare/` or `.git/` | read-write |
| gh proxy socket dir | host tmpdir | read-write |
| `/etc/{ssl,nix,resolv.conf,…}` | host | read-only |

**Git worktree detection.** The jail automatically mounts the full worktree root, not just the
current subdirectory. For bare+worktree layouts (e.g. `git worktree add`), the common git dir is
mounted separately so commits and refs work correctly. The `hooks/` directory is masked with a
tmpfs so Claude cannot plant code that runs on the host.

**Synthetic gitconfig.** A minimal gitconfig is generated from the host's `git config --global
user.name/email`. It sets `gpgsign = false`, rewrites `git@github.com:` URLs to HTTPS, and
registers `gh auth git-credential` as the credential helper. Your real gitconfig is not visible
inside the jail.

### Tool set

| Tool | Notes |
|---|---|
| `claude` | Claude Code |
| `nix`, `git` | standard |
| `gh` | proxied — see below |
| `curl`, `bash`, `python3`, `direnv` | standard |
| `coreutils`, `findutils`, `jq` | standard |
| `grep` (GNU), `ripgrep`, `sed` (GNU) | standard |
| `ssh`, `ssh-keyscan`, `scp` | openssh client; agent available only with `--allow-ssh` |
| `ingest`, `mcp-server` | from this repo |

### GitHub access (gh proxy)

By default, `claude-jail` starts a `gh-jail-server` process on the host before entering the
sandbox. A thin `gh-jail-client` binary, installed as `gh` inside the jail, forwards every `gh`
invocation to the server over a Unix domain socket. The server runs the real `gh` with the host's
existing authentication — **no token is ever passed into the jail**.

**Policy enforced by the server:**

| Operation | Read-only (default) | With `--write` |
|---|---|---|
| `gh issue`, `gh pr`, `gh run`, `gh workflow` | allowed | allowed |
| `gh repo view / list / clone / sync` | allowed | allowed |
| `gh repo create / delete / rename / …` | blocked | blocked |
| `gh release view / list / download` | allowed | allowed |
| `gh release create / upload / delete / edit` | blocked | allowed |
| `gh api` — `GET` | allowed | allowed |
| `gh api` — `POST / PATCH / DELETE` | blocked | allowed |
| `gh auth`, `gh config`, `gh ssh-key`, `gh gpg-key`, `gh extension`, `gh alias` | blocked | blocked |

**Repo restriction.** The cwd's GitHub remote is allowed automatically. Pass `--repo OWNER/REPO`
(repeatable) to add more. Any `gh` call with an explicit `-R`/`--repo` flag or an
`/api/repos/OWNER/REPO/…` path not in the allowed list is rejected by the server.

If the server fails to start, `gh` is simply unavailable inside the jail (a warning is printed;
the jail still launches). Pass `--no-github-auth` to skip the proxy entirely.

### Security model

| Concern | Mitigation |
|---|---|
| Host filesystem | tmpfs home; only explicit bind mounts visible |
| SSH private keys | not mounted; agent socket excluded by default (`--allow-ssh` opts in with a warning) |
| Git hooks | `hooks/` masked with tmpfs; Claude cannot plant host-side hooks |
| Git identity | synthetic gitconfig; real `~/.gitconfig` not visible |
| GitHub credentials | not injected; all `gh` calls go through the policy-checking proxy |
| Nix daemon | socket forwarded so `nix build` / `nix develop` work inside the jail |

### Troubleshooting

**`gh: GH_PROXY_SOCKET not set`**
The server didn't start (or `--no-github-auth` was passed). Check the startup output for a
warning line from `claude-jail`.

**`gh: cannot connect to proxy at …`**
The server crashed after creating the socket. Run with `--debug` to see the full server command
and reproduce the error outside the jail.

**`gh-jail-server did not create socket within 5 s`**
The server binary is missing or `CLAUDE_JAIL_GH_SERVER` is wrong. This env var is set
automatically by the Nix wrapper; when running the raw binary, set it manually.

**Commits fail with GPG errors**
The synthetic gitconfig sets `gpgsign = false`. If a system-level gitconfig overrides it, check
`GIT_CONFIG_SYSTEM` and ensure it isn't re-enabling signing.

**Claude can't see files outside the project**
Only the worktree root and explicitly bound paths are visible. Use `--ro /path` or `--rw /path`
to expose additional directories.

**direnv environment not applied**
`claude-jail` runs `direnv export json` on the host before entering bwrap. If the `.envrc` isn't
trusted, run `direnv allow` in the project directory first.

---

## Using the index with Claude

### How Claude uses the tools automatically

Once connected, Claude will automatically call the search tools when it needs to understand
your code. You don't have to do anything special — just work normally. For example:

- *"How does the authentication middleware work?"* → Claude searches for auth-related code
- *"Why was this API endpoint added?"* → Claude searches GitHub issues for context
- *"What does the `UserService` class do?"* → Claude fetches chunks for that symbol

### Prompts that get the most out of the index

Being explicit helps Claude know to search rather than guess:

```
Search the codebase for how we handle database connection pooling.

Look through the indexed GitHub issues for any discussion of rate limiting.

Find all the Rust functions related to embedding and explain how they fit together.

Search the docs for our deployment workflow.
```

### Available tools

| Tool | When to use it |
|---|---|
| `search_code` | Natural language or code questions about the codebase |
| `bm25_search` | Exact identifier or symbol lookups (faster, no embeddings) |
| `search_docs` | Questions about workflows, SOPs, or agent instructions |
| `search_github` | "Was there an issue about X?" or "How was Y implemented?" |
| `list_repos` | See what's indexed |
| `get_file` | Read a complete file by path |

You can guide Claude toward a specific tool:

```
Use bm25_search to find every place we call `send_email`.

Search GitHub PRs for anything related to the login refactor.

Search only Haskell files for the `parseConfig` function.
```

### Filters

The search tools accept optional filters you can mention in your prompt:

- **Language**: `search in Rust files`, `TypeScript only`
- **Symbol kind**: `only functions`, `find all classes`, `interfaces only`
- **Doc kind**: `search workflows`, `look in SOPs`
- **GitHub**: `open issues only`, `only PRs`, `in the anthropics/claude-code repo`

---

## Keeping the index current

### After pulling new code

Re-index is incremental — only changed files are processed:

```bash
just index /path/to/your/repo
```

### Scheduled re-indexing (optional)

Add to your crontab or a systemd timer:

```bash
# Re-index at 2am every night
0 2 * * * cd ~/agentic-nix && nix develop --command just index ~/work/myrepo
```

### After large refactors

If you've moved many files around, a full re-index is cleaner:

```bash
just reindex /path/to/your/repo
```

---

## Environment variables

All variables have sensible defaults; override as needed.

| Variable | Default | Description |
|---|---|---|
| `PG_DSN` | `postgresql://127.0.0.1:5432/codebase` | PostgreSQL connection string |
| `OLLAMA_HOST` | `http://127.0.0.1:11434` | Ollama API base URL |
| `EMBED_MODEL` | `hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0` | Embedding model |
| `RERANK_MODEL` | *(empty — disabled)* | Set to a fastembed cross-encoder to enable reranking |
| `GITHUB_TOKEN` | *(empty)* | GitHub personal access token (raises rate limit to 5000/hr) |

---

## Quick reference

```bash
# Services
nix run .#dev                          # start PostgreSQL + Ollama

# Binaries
just build                             # build mcp-server + ingest

# Indexing
just index /path/to/repo               # code (incremental)
just reindex /path/to/repo             # code (force full re-index)
just index-docs /path/to/repo          # markdown docs
just index-github OWNER/REPO           # GitHub issues + PRs
just reindex-github OWNER/REPO         # GitHub (ignore watermarks)

# Inspection
just stats                             # row counts
just sync-status                       # GitHub watermarks
just psql                              # open a psql session

# Schema
just migrate                           # apply schema.sql (safe to re-run)
```

---

## Troubleshooting

**"Embedding failed (is Ollama running?)"**
The MCP server can't reach Ollama. Make sure `nix run .#dev` is running, or check that
`OLLAMA_HOST` points to the right address.

**"Database error: connection refused"**
PostgreSQL isn't running. Start it with `nix run .#dev`. Data lives in `./data/pg/` —
you must run from the repo root so the path resolves correctly.

**Claude doesn't call the search tools**
Claude doesn't always reach for MCP tools unprompted. Be explicit: *"Search the indexed
codebase for..."* or *"Use the search tools to find..."*.

**Slow first query after starting**
The embedding model loads lazily on the first request. Subsequent queries are fast.

**GitHub rate limit hit**
Set `GITHUB_TOKEN` with a personal access token. The unauth limit is 60 requests/hour;
authenticated is 5000/hour.

**Ollama uses CPU instead of GPU on Ubuntu**
Nix-packaged Ollama can't resolve CUDA libraries outside NixOS. Use the system Ollama installer
instead — see [Non-NixOS / Ubuntu setup](#non-nixos--ubuntu-setup). Confirm GPU is active with
`ollama ps` while a model is loaded.
