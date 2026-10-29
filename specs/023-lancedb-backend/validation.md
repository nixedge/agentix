# Validation record

Completed on 2026-10-28 in `/home/sam/home/agentix/023-lancedb-backend`, based on master b01b0e3. This implements the full replacement scope: code, documents, GitHub issues/PRs/comments, repository registry and sync state. No commit, push or installed-service switch was performed. Production PostgreSQL was never contacted or changed.

## Completed checks

- Final release workspace suite with optional LAN authentication: **202 passed, 0 failed, 3 ignored**, across 46 test suites. The ignored tests require a completion-capable GGUF model (two) or Whisper model (one). The existing pinned all-MiniLM GGUF was supplied for embedding tests.
- Real Lance storage tests cover atomic file replacement, stale deletion, project restrictions, model/dimension/schema guards, independent-reader visibility, nullable vectors, HNSW construction, incremental changes and competing-process writer exclusion. The parallel storage suite also passed ten consecutive runs.
- Actual subprocess ingestion and stdio MCP tests cover code/docs/GitHub retrieval, repository enumeration, ordered file retrieval, package retagging, incremental ingestion and preservation of old data when embedding fails. Unit tests cover sync watermarks/errors, migration validation, all legacy collections and NULL-project metadata across import batch boundaries.
- Strict Clippy passed for store/search/indexer/MCP with all targets and warnings denied. Workspace compilation, workspace Rust formatting, Nix formatting and diff whitespace checks passed.
- The initial local implementation passed canonical `nix build .#agentix-store .#agentix-indexer .#agentix-mcp-server --no-link`. That store derivation explicitly enabled checks: its actual build log records one unit test and five integration tests passing. Initial Nix-built CLI stats and lexical search succeeded against the imported fixture while PostgreSQL was stopped. Remote package validation is tracked below.
- The read-only SQL exporter ran against a new disposable local PostgreSQL fixture database, not production. Eight records covering all legacy collections/state exported as valid JSONL, including quotes, backslashes, newlines and Unicode. Import into a fresh Lance directory completed with matching per-collection counts and manifest checksum `f8aefd5eb1baf3386037c812ad2e451e5bae6dc5c50eb217b66e3991bf543007`. The fixture server was stopped afterwards.

Local raw logs and fixture data are ignored under `data/lance-validation/`: `workspace-tests2.log`, `clippy-final3.log`, `workspace-check-complete.log`, `lock-stress.log`, `nix-verified.log`, `nix-store-verified.log`, `export-fixture.log`, `import-fixture-final.log` and `nix-cli-{stats,search}.log`. Worktree 022 remains separate; only its ignored Cargo target directory was reused as a build cache.

## Corrections made during verification

- A lexical test initially used underscore-separated tokens with a legitimate shared FTS term. Distinct fixture tokens and a persisted-content assertion now verify deletion without changing search semantics.
- An OS error 11 was initially misidentified as worker exhaustion. It was actually a writer-lock lifecycle issue: fork-inherited descriptors could extend the old lock. Explicit unlocking before releasing the mutex fixes it; a retained-descriptor regression test and the original parallel suite pass. Worker limits and test concurrency were not reduced.
- The first Nix test derivation inherited `doCheck = false`; this was corrected to `true` and actual test execution verified in the final log.
- An existing home-agent test fixture referenced a removed `tts_engine` field. Updating it to the current HTTP client field enabled the full workspace suite. Six other existing files received formatting-only normalization from the workspace formatter.

## Practical limits

No production-sized migration, sustained concurrent workload, process-crash recovery exercise or installed-service deployment was performed. Export/import correctness is demonstrated with fixtures; production data still needs its own count/query review before switching configuration. Writes provide per-file/collection commits, not transactions across files or collections; overlapping independent writers return a retryable busy error. Relevance and performance results from benchmark 022 remain separate evidence, not a new production performance guarantee.

## Remote shared-store amendment

- Focused release suite: 100 passed, including real HTTP/Lance tests with twelve concurrent writers across independent clients, atomic metadata counters, authorization-before-body parsing, oversized/invalid requests and protocol/model guards. Remote and local ingestion and stdio MCP tests pass; the remote MCP scenario runs two live processes concurrently and verifies a package retag from one is visible to the other.
- Full workspace release suite: 198 passed, 3 model-dependent ignored. This includes a standalone storage-server process loading a runtime token file and serving actual remote mutations. Final MCP-only rerun after changing the no-fallback fixture to XDG_DATA_HOME passed both tests.
- Strict Clippy for all four storage/search/indexer/MCP crates and all targets passed again, as did Rust/Nix formatting and diff whitespace checks.
- NixOS module evaluation passed with only storage enabled: loopback listener, closed default firewall, dedicated state and LoadCredential environment. The generated unit text confirms the credential path, LoadCredential, StateDirectory and writable index directory without including the secret value. A custom address/port/path/model configuration and explicit firewall opening evaluated correctly; a Nix-store token path produces the intended failed assertion. One custom evaluation expression initially had a lambda-spacing typo; correcting the diagnostic expression required no module change.
- Final canonical `nix build .#agentix-store-server .#agentix-store .#agentix-indexer .#agentix-mcp-server --no-link --json` passed. The actual Nix storage check ran all nine tests successfully (one unit, two HTTP, one server-process and five storage tests). The earlier source-snapshot build was deliberately cancelled after its shared dependencies were cached; the final-source build completed successfully.
- Packaged runtime smoke passed: the Nix-built server opened the imported legacy fixture using a runtime token file; two Nix-built MCP clients concurrently performed lexical search and repository listing over HTTP, and the Nix-built indexer reported one row in each of all six entity collections. The temporary server was stopped by cleanup; PostgreSQL and model services were not used.

Remote logs: `remote-tests.log`, `remote-workspace-tests.log`, `remote-mcp-final.log`, `remote-clippy-final.log`, `remote-format-final.log`, `remote-nixos-eval.json`, `remote-nixos-custom.json`, `remote-generated-unit.txt`, `remote-nix-final.{json,log}`, `remote-nix-store-check.log` and `remote-smoke-{server,search,repos,stats}.log`. The first compile check began before new HTTP test dependencies were added and reported missing axum in test targets; subsequent all-target Clippy and actual tests include those dependencies and pass.

These tests do not establish a production throughput SLO or validate deployment behind the operator's actual TLS/private-network setup. The API trusts authenticated clients with the entire index. Large atomic file replacements must fit the documented 32 MiB request limit. No installed configuration, runtime credential or firewall was changed on the user's hosts.

## Optional authentication for LAN access

The user explicitly chose no-token access on their trusted LAN. Server and client token
configuration is now optional; configured credentials still enforce authentication.
The final release workspace suite passed 202 tests with the same 3 model-dependent
ignores, including no-token HTTP mutation/search, two simultaneous no-token stdio MCPs
and standalone server startup without a credential. Empty, short and non-UTF8 configured
tokens fail before opening an index. Strict all-target Clippy and formatting passed.
NixOS evaluation and generated-unit checks confirm that the default tokenFile=null emits
neither LoadCredential nor a token environment variable; setting tokenFile retains both.
Final canonical builds of store-server/store-check/indexer/MCP passed, including all 12
actual Nix storage tests (one unit, three HTTP, three server-process and five storage).
Packaged no-token smoke passed: with both token variables unset, the packaged server
served two concurrent packaged MCP clients (lexical search/repositories) and packaged
indexer stats confirmed one row in all six entity collections. Cleanup stopped the server.

Evidence: `lan-workspace-final.log`, `lan-format-clippy-final.log`, `lan-nixos-eval.json`,
`lan-nix-complete.{json,log}`, `lan-nix-store-check.log` and `lan-smoke-*.log`. Storage now
has a dedicated dependency artifact matching its own feature graph; after that initial
cache build, the final server and storage-check source builds took 30 and 46 seconds.
Earlier redundant focused Rust builds were consolidated into the final full-workspace
run, which includes the configured-token regression cases and passed all tests above.
