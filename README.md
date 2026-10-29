# agentic-nix

Hybrid code search for Claude. Index your codebases, documentation, and GitHub issues into
local LanceDB (BM25 + unquantized HNSW). A Rust MCP server exposes the index to Claude Code
so it can answer questions about your code without reading every file from scratch.

**What this replaces:** Claude reading 50 files one by one to find something. Instead, Claude
queries the index with a single tool call and gets the 10 most relevant chunks in under a second.

---

## How it works

```
your repos ──► ingest ──► local LanceDB
                               │
                          BM25 + vector          Ollama
                          hybrid search    ◄──  embeddings
                               │
                          MCP server ──► Claude Code
```

1. The `ingest` binary walks your repos, extracts named symbols via tree-sitter, embeds each
   chunk via Ollama, and stores everything in LanceDB.
2. The `mcp-server` binary sits between Claude and the database, exposing search tools over
   the MCP protocol (stdio).
3. Claude Code connects to the MCP server automatically and calls the tools when it needs
   to understand code.

---

## Prerequisites

- [Nix](https://nixos.org/download/) with flakes enabled
- Git

That's it. LanceDB, Ollama, Rust, and all other tools are managed by Nix.

The native assistant client lives in [agentix-android](agentix-android/README.md).
Build its APK with `nix build .#agentix-android`; use `nix develop .#android` for
the dedicated Kotlin/Android environment.

**Enable flakes** if you haven't already — add to `~/.config/nix/nix.conf`:
```
experimental-features = nix-command flakes
```

> **On Ubuntu or other non-NixOS systems?** `nix run .#dev` won't use your GPU — CUDA library
> paths don't resolve correctly for Nix-packaged Ollama outside NixOS. See
> [Non-NixOS / Ubuntu setup](#non-nixos--ubuntu-setup) below for the system-services alternative.

---

## Non-NixOS / Ubuntu setup

On Ubuntu, use a persistent local LanceDB directory and system-managed Ollama. The Nix dev shell still provides the Rust
toolchain, `just`, and all build tools — only the services differ.

### 1. Configure index access

Code, documentation, GitHub issues/PRs/comments, repository metadata and sync state live in
LanceDB. PostgreSQL and its extensions are no longer required. For server storage with laptop
MCPs, use the [shared-storage deployment guide](specs/023-lancedb-backend/quickstart.md#server-storage-with-laptop-mcp-clients)
and configure `AGENTIX_STORE_URL` with an optional runtime credential. For same-host local use, both
`ingest` and `mcp-server` use the same `AGENTIX_LANCE_PATH`:

```bash
export AGENTIX_LANCE_PATH="$HOME/.local/share/agentix/index"
```

The default is `$XDG_DATA_HOME/agentix/index`, or `$HOME/.local/share/agentix/index` when
XDG_DATA_HOME is unset. Keep this directory persistent and writable by the indexing user.
For an existing PostgreSQL index, follow the [migration guide](specs/023-lancedb-backend/quickstart.md)
before changing the installed MCP configuration. Old PostgreSQL data is not deleted or
converted automatically. A leftover `PG_DSN` without an explicit Lance path returns an error
so an existing installation cannot silently start searching a new empty index.

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

The first `nix develop` downloads Rust, LanceDB, Ollama, and all dependencies. This takes
a few minutes once; subsequent shells are instant.

### 2. Build the Rust binaries

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
- **Ollama** on `localhost:11434` with the `jina-code-embeddings-1.5b` model pulled.
  First start pulls the model (~1.5 GB); subsequent starts are instant.

Service data is stored under `./data/`. The search index uses AGENTIX_LANCE_PATH independently; configure the same path for ingestion and MCP.

### 4. Verify everything is running

```bash
# Shows zero counts for a new local index
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
  --env AGENTIX_LANCE_PATH=/absolute/path/to/agentix-index \
  --env OLLAMA_HOST=http://127.0.0.1:11434
```

Or add it manually to your Claude Code config (`~/.claude.json` or project `.claude/mcp.json`):

```json
{
  "mcpServers": {
    "agentic-nix": {
      "command": "/home/you/agentic-nix/target/release/mcp-server",
      "env": {
        "AGENTIX_LANCE_PATH": "/absolute/path/to/agentix-index",
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
| `AGENTIX_LANCE_PATH` | XDG data directory + `agentix/index` | Persistent local LanceDB directory |
| `EMBED_DIMENSIONS` | `1536` | Stored/query vector dimensions; must match the model |
| `LANCE_NPROBES` | `20` | IVF partitions searched (1..64) |
| `LANCE_EF` | `100` | HNSW query effort (100..400) |
| `OLLAMA_HOST` | `http://127.0.0.1:11434` | Ollama API base URL |
| `EMBED_MODEL` | `hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0` | Embedding model |
| `RERANK_MODEL` | *(empty — disabled)* | Set to a fastembed cross-encoder to enable reranking |
| `GITHUB_TOKEN` | *(empty)* | GitHub personal access token (raises rate limit to 5000/hr) |

---

## Quick reference

```bash
# Services
nix run .#dev                          # start Ollama + gateway

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

# Maintenance
just optimize                          # rebuild local search indexes
```

---

## Troubleshooting

**"Embedding failed (is Ollama running?)"**
The MCP server can't reach Ollama. Make sure `nix run .#dev` is running, or check that
`OLLAMA_HOST` points to the right address.

**"index is busy: another process is writing"**
Wait for the other indexing/maintenance process to finish and retry. Readers can continue
searching committed data. Ensure ingest and MCP use the same persistent AGENTIX_LANCE_PATH.

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

### Ingestion and maintenance through MCP

The MCP exposes every ingest CLI operation. Use `start_ingestion` for background
work, then poll `ingestion_status` with the returned `id` as `job_id`:

```json
{"request":{"source":"git","url":"https://github.com/IntersectMBO/cardano-ledger","branch":"master"}}
```

Keep the originating MCP process running until the job finishes. Other MCPs using
the same store can inspect or cancel it; `retry_ingestion` starts a new attempt
after failure, cancellation or interruption. There are two background workers per
MCP. Jobs heartbeat every five seconds; a missing heartbeat becomes interrupted
after 30 seconds when status is checked. Committed data survives cancellation.
Git and registry packages have explicit completion markers so partial ingestion
is retried. Package versions indexed before these markers revalidate once.

| CLI capability | MCP interface |
| --- | --- |
| `code`, `docs` | `start_ingestion`, source `code` or `docs`, `repo_path` |
| `git` | source `git`, `url`, optional one of `rev`/`branch`/`tag` |
| `hackage`, `crate`, `pypi` | source `package`, `package`, `version`, `ecosystem` (`haskell`/`rust`/`python`); synchronous `fetch_package` also remains available |
| `github` | source `github`, `repo` (`OWNER/REPO`), optional `streams` |
| `import` | source `import`, `input`, `destination`, `model`, `dimensions` |
| `stats`, `sync-status` | `index_status` |
| `list` | `list_sources`; `list_repos` provides project-filtered search summaries |
| `optimize` | `optimize_index`, optional `collections` (empty means all six) |
| `prune repo/local/old-versions` | `prune_index`, `selection.target` (`repo`/`local`/`old_versions`), optional `repo_path`/`package` |

Local paths and offline import destinations refer to the MCP host, including its
jail. Import creates a fresh local store; it does not merge into a remote store.
Pruning defaults to `dry_run: true`. Administration is global: actual pruning
also removes other projects' tags on the selected repository. `PROJECTS` filters
search visibility, not administrative authority.

Successful ingestion automatically tracks changes and maintains affected search
indexes. Shared metadata leases coordinate concurrent MCPs. First use optimizes;
later changes accumulate until row-count drift reaches 10% (bounded to 64–512 rows)
or pending changes are at least five minutes old on a subsequent call. There is
no idle scheduler: use `optimize_index` to flush deferred maintenance immediately.
`index_status` shows pending generations, leases, errors and completion markers.
Cancelled/crashed optimization leases expire after 15 minutes. A maintenance
failure reports that ingestion committed and remains pending; it does not undo
indexed files.

### Shared storage for laptop MCPs

For separate storage and MCP hosts, deploy `agentix-store-server` on the index host and
keep each MCP on its laptop. For LAN access, configure `AGENTIX_STORE_URL=http://index-host:8091`
instead of `AGENTIX_LANCE_PATH`; no token is required by default. Set a server `tokenFile`
and client `AGENTIX_STORE_TOKEN_FILE` only when enabling authentication.
The storage API supports concurrent clients, serializes
writes and checks model/dimension compatibility; it never silently opens a local fallback.
Query embedding and reranking retain the existing MCP model-service configuration.

The NixOS module now exposes `services.agentix.storage` for persistent storage, runtime
credentials and listener configuration. See the [LAN server/laptop deployment guide](specs/023-lancedb-backend/quickstart.md#server-storage-with-laptop-mcp-clients)
for the module and MCP configuration. No LanceDB directory mount is required on laptops.
