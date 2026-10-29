# Build, use and migrate the LanceDB index

This branch replaces the PostgreSQL search/index runtime for every supported source. It does not switch an installed service or delete the old database. Work from `/home/sam/home/agentix/023-lancedb-backend`.

For the server/laptop deployment, use [Server storage with laptop MCP clients](#server-storage-with-laptop-mcp-clients). The fresh-install commands below describe local administrative use.

## Fresh installation

```bash
nix develop .#storage
export AGENTIX_LANCE_PATH="$HOME/.local/share/agentix/index"
export OLLAMA_HOST=http://127.0.0.1:11434
cargo run --release -p agentix-indexer --bin ingest -- code /absolute/repo --project my-project
cargo run --release -p agentix-indexer --bin ingest -- docs /absolute/repo --project my-project
cargo run --release -p agentix-indexer --bin ingest -- github OWNER/REPO
cargo run --release -p agentix-indexer --bin ingest -- stats
cargo run --release -p agentix-mcp-server --bin mcp-server -- repos
cargo run --release -p agentix-mcp-server --bin mcp-server -- search 'your query'
```

The regular Nix packages remain `.#agentix-indexer` and `.#agentix-mcp-server`. Both now include the shared Lance storage library. `nix run .#dev` starts the model/gateway services; no PostgreSQL process is required. Non-NixOS systems use the same local index directory and their existing Ollama installation.

`EMBED_MODEL` defaults to `hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0`; `EMBED_DIMENSIONS` defaults to 1536. Model identity and dimensions are persisted and checked on reopen. A model change requires a separate new index and re-embedding; renaming the environment variable does not convert existing vectors.

`PROJECTS` is a comma-separated visibility whitelist for code/docs/search/list/file operations. Empty means unscoped, matching the existing API. Package fetches tag the first configured project and merge tags into an already indexed package. GitHub search retains its existing repository/state/type contract; comments are retained in their respective collections, as before.

`LANCE_NPROBES` defaults to 20 (1..64), `LANCE_EF` to 100 (100..400). Vector indexes use unquantized cosine IVF_HNSW_FLAT, 64 partitions, m20, construction ef400 once there are at least 64 embedded records. New collections below that threshold search exhaustively. Initial ingestion builds a graph when needed; incremental writes remain visible before maintenance. After a large ingestion or import, run:

```bash
cargo run --release -p agentix-indexer --bin ingest -- optimize
```

The index directory must be writable for ingestion/package fetch/maintenance. Independent readers refresh committed versions. Writes are serialized within one Store handle; an overlapping writer in another process returns an explicit busy error and can be retried. Whole-file replacements commit only after their complete embeddings are prepared, and remove obsolete chunks in that same commit. Transactions spanning different files/collections are not promised.

For jailed MCP processes, expose the chosen index path through the jail's existing explicit writable bind option and set AGENTIX_LANCE_PATH to that path. This feature does not automatically expand any jail mounts or permissions. Put the same path in MCP configuration and ingestion commands; a host-only index path is not accessible through a jail without that explicit bind.

## Existing PostgreSQL data

1. Keep the old service and database unchanged. Supply its connection via SOURCE_DSN. Export into a new private local file using plain SELECT output, not COPY:

```bash
nix develop .#storage --command bash -c 'set -e; test ! -e export.jsonl; psql "$SOURCE_DSN" -X -qAt -v ON_ERROR_STOP=1 -f scripts/export-postgres.sql > export.jsonl'
```

The SQL uses one REPEATABLE READ, READ ONLY snapshot and exports code, docs, issues, PRs, both comment collections, repo_index and sync_state. It reads nullable stored vectors directly. The PostgreSQL client in the migration shell is an export utility, not a runtime backend.

2. Import into a new destination. Explicitly assert the historical model identity and dimension; the source schema cannot prove the model that generated old vectors. Use the same assertion as EMBED_MODEL/EMBED_DIMENSIONS when opening the imported index later.

```bash
nix develop .#storage --command cargo run --release -p agentix-indexer --bin ingest -- import export.jsonl --destination /absolute/new-index --model hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0 --dimensions 1536
```

Import validates the entire file before writing records, then streams batches of at most 128 rows, preserving metadata/project arrays/timestamps and nullable vectors. It refuses populated destinations, checks final per-collection counts and file checksum, and saves `import-manifest.json` with complete/incomplete status. Failed partial destinations are not automatically resumed or reused; retain them for inspection and choose a fresh path for an approved retry.

3. Inspect the manifest and counts, build indexes, and validate representative code/docs/GitHub queries and project restrictions using the new directory. Only then change the installed MCP configuration to AGENTIX_LANCE_PATH. Keep the old export/database until that separate switch is reviewed. Setting PG_DSN alone no longer connects to PostgreSQL and returns an explicit configuration error.

## Validation commands

```bash
nix develop .#storage --command cargo fmt --all --check
nix develop .#storage --command cargo clippy -p agentix-store -p agentix-search -p agentix-indexer -p agentix-mcp-server --all-targets -- -D warnings
nix develop .#storage --command cargo test --release -p agentix-store -p agentix-indexer -p agentix-search -p agentix-mcp-server
nix build .#agentix-store .#agentix-indexer .#agentix-mcp-server
```

Consult validation.md for the actual completed checks and any remaining merge gates. Fixture tests use actual local Lance tables and fixed vectors/a local embedding-protocol fixture; they do not require production models or write to GitHub.

## Server storage with laptop MCP clients

MCP stays on each laptop. Only the server owns the Lance directory. All MCPs and remote ingesters use the storage API; no shared filesystem or remote MCP transport is needed. On a trusted LAN, no token is required. The service shares one Store, allows concurrent reads and queues writes. It admits at most 64 active requests with a 32 MiB request-body limit; overload returns HTTP 429. Mutation failures are not automatically retried because a response can be lost after commit.

On the NixOS storage host (replace `inputs.agentix` with your flake input name):

```nix
{ inputs, pkgs, ... }: {
  imports = [ inputs.agentix.nixosModules.agentix ];
  services.agentix.storage = {
    enable = true;
    package = inputs.agentix.packages.${pkgs.stdenv.hostPlatform.system}.agentix-store-server;
    dataDir = "/var/lib/agentix-store/index";
    host = "0.0.0.0";
    port = 8091;
    openFirewall = true;
    # tokenFile defaults to null: no authentication for LAN access.
    # embedModel/embedDimensions must match the clients and existing index.
  };
}
```

The service creates its persistent directory and does not depend on the inference services being enabled. The example listens on all interfaces and opens port 8091. You can bind a specific LAN address or use your existing firewall rules instead.

The module's defaults remain loopback and a closed firewall; the example opts into LAN access. Clients reaching an unauthenticated service have full index access, including ingestion/deletion. PROJECTS retains its existing visibility-filter behavior.

Laptop MCP environment:

```bash
export AGENTIX_STORE_URL=http://index-host:8091
export EMBED_MODEL=hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0
export EMBED_DIMENSIONS=1536
unset AGENTIX_LANCE_PATH PG_DSN AGENTIX_STORE_TOKEN AGENTIX_STORE_TOKEN_FILE
nix run .#agentix-mcp-server -- repos
```

Use those same variables for the laptop's stdio MCP configuration and any `ingest` commands. Ensure the jail's existing network policy permits the API endpoint; no index mount or token provisioning is needed for LAN mode.

Authentication is optional. To enable it, set `services.agentix.storage.tokenFile = "/run/secrets/agentix-store-token";` on the server and `AGENTIX_STORE_TOKEN_FILE` to a file containing the same token on each laptop. Provision that runtime file through your existing secret mechanism; the token must contain at least 32 printable non-space ASCII characters. NixOS uses `LoadCredential` only when tokenFile is set. `AGENTIX_STORE_TOKEN` is an alternative to a client token file. Invalid configured credentials still fail explicitly. For jailed MCPs using authentication, the trusted launcher can inject the token through the environment without mounting a credential file.

Query embeddings and reranking still run from the MCP process using its existing model/service configuration (`OLLAMA_HOST` can point to the inference server). Index/model identity is checked at connection and on every operation. Unreachable server, bad credentials or mismatched models fail explicitly; there is no local-index fallback. AGENTIX_STORE_URL and AGENTIX_LANCE_PATH are mutually exclusive.

Fresh re-ingestion can run on any machine with access to the sources and embedding service:

```bash
nix run .#agentix-indexer -- code /absolute/repo --project my-project
nix run .#agentix-indexer -- github OWNER/REPO
nix run .#agentix-indexer -- optimize
```

For offline PostgreSQL import, stop `agentix-store`, import locally into a fresh destination as the service account, then configure dataDir to that destination and start the service. Routine ingestion and maintenance should use the API while the service runs. Back up the complete directory with writes paused or an appropriate filesystem snapshot. The metadata compare-and-swap protocol prevents lost project-tag/counter updates between remote clients; whole-file replacements retain last-committed replacement semantics rather than merging competing edits.

Build the server with `nix build .#agentix-store-server`. The service is named `agentix-store.service`. Laptop deployments update only their MCP binary/environment; no laptop Lance directory is required.
