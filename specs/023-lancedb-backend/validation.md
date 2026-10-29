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

## Deployed LAN validation — 2026-10-29

The user deployed the storage service; packaged clients in the laptop jail exercised
`http://ai.lan.disasm.us:8091` without credentials. The live handshake reports protocol 1,
schema 1 and the expected jina Q8_0 model with 1536 dimensions. No jail configuration
or deployed service configuration was changed during validation.

| Feature | Live result |
| --- | --- |
| Local checkout ingestion | 1,454 code chunks from 191 files and 511 documentation chunks from 84 files |
| Incremental checkout ingestion | Zero new chunks; all 191 code files and 84 documentation files skipped |
| MCP package fetch | crates.io `itoa-1.0.15`, Hackage `split-0.2.5`, PyPI `six-1.17.0`; repeat fetch detected existing package |
| CHaP and Git sources | `cardano-binary-1.7.1.0` from CHaP; Git tag `dtolnay/itoa@1.0.15` including its README |
| Search relevance | Known identifiers, semantic package queries, docs, language/symbol/repository filters and negative scope checks exercised |
| Concurrent MCPs | Two independent stdio processes each passed the same 12 checks; concurrent package retags preserved both new project tags and the existing tag on all 16 rows |
| Incremental replacement/deletion | A separate temporary repo proved changed code, stale code deletion, updated docs and dry-run/actual pruning; fixture removed afterwards |
| Literate Haskell | A separate Bird-style `.lhs` fixture indexed and returned its expected Haskell symbol; fixture pruned afterwards |
| GitHub ingestion | `nixedge/agentix` and `dtolnay/itoa`: 26 issues, 45 PRs, 80 issue comments, 23 review comments |
| GitHub retrieval | MCP issue/PR searches with repo/state/entity filters; both comment collections queried through the storage API |
| GitHub incremental sync | Stable collection counts; persisted watermarks and cleared errors verified |
| Index maintenance | Explicit optimize completed; the 12 MCP parity checks passed afterwards |
| Optional reranking | Actual BGE cross-encoder ran locally; integer-to-string query ranked itoa's formatting function first |

Two GitHub client fixes were necessary and authorized during this exercise: supply
the required User-Agent header (the original requests returned HTTP 403), and record
successful empty streams so prior errors clear without advancing or regressing the
watermark. The rebuilt indexer exercised all four real GitHub streams. These changes
require the new ingest/MCP client builds, not a storage-server redeployment.

Two limitations remain visible. With reranking disabled, an exact
`ensure_embed_model` query returned the function first in BM25 and fifth in hybrid
search; the original top-three hybrid assertion failed and is not counted as a pass.
`get_file` successfully retrieves indexed symbols, but reconstruction omits unindexed
imports/declarations and intervening blank lines. This symbol-extraction behavior
also exists on master; it is not an exact source-file download. No ranking or
file-reconstruction behavior was changed during this validation.

Live corpus after reindexing the client fix and validation notes: 2,021 code chunks, 513 docs,
26 issues, 45 PRs, 80 issue comments and 23 review comments. This is functional
validation with real sources, not a sustained-load or comprehensive relevance benchmark.
Raw evidence, one-off Rust MCP probe and query expectations are ignored under
`data/lance-validation/deployed-*`; the probe uses the actual packaged stdio MCP binary.

Both updated client packages built with Nix. Two instances of the rebuilt MCP each
passed the same 12 live parity checks. The expanded sync-state regression passed
under the cached workspace release feature graph (one selected test, others filtered).
This was a focused regression run, not a repeat of the full 202-test suite. An initial
fresh-target build and then a package-only feature graph were stopped in favor of
reusing the existing workspace artifacts; neither reported an assertion failure.
Strict all-target indexer Clippy, workspace Rust formatting and diff whitespace checks
passed. No storage-server source change or redeployment was needed for either fix.

## Large ledger checkout — recovery implemented, inference blocked

The user requested all packages from GitHub `IntersectMBO/cardano-ledger` master
instead of individual CHaP packages. Revision `f4a3026c9bae` stored 2,486 code chunks
from 177 files before the model endpoint returned HTTP 502, 51 seconds after starting
the clone/ingestion command. This is an incomplete corpus, not a completed timing.

The authorized client fixes add bounded embedding retries for HTTP 429/5xx with
response diagnostics, and explicit Git completion/docs state. An incomplete revision
resumes file hashing/ingestion instead of being mistaken for a complete index. Forced
runs invalidate prior completion before work; pruning removes completion metadata.
Three actual HTTP fixture tests verify recovery through 502/429, six-attempt exhaustion
with error detail, and no retry on permanent 400. A disposable Git integration test
verifies partial-row recovery, adding docs after a code-only run, avoiding redundant
embedding on completed runs, invalidation on an actual forced failure, and cleanup.
All four regressions, strict all-target indexer Clippy, Rust formatting and canonical
Nix builds of both indexer and MCP pass. Initial fixture lints
were corrected by moving the test module and naming its state type.

The retry of the live ledger command exhausted its five retries after 76 seconds,
still at 2,486 chunks/177 files. The first error was `backend request failed: connection
closed before message completed`; subsequent errors were failure to reach the fallback
at `http://localhost:11434/api/embed`. A later small model probe timed out. Storage
remained healthy and persisted `git_ingest.complete=false`; a BM25 query for Coin
returned actual Haskell results in 164 ms including MCP CLI startup. This does not
establish hybrid latency for a completed ledger corpus. Operator inference-service
status/journal output was requested; no server configuration or inference source was
changed. Root cause is not yet established.

Evidence: `data/lance-validation/ledger-*`, including first/retry timings, diagnostic
responses, regression logs and canonical client-build outputs. T019 remains open until
the model service is diagnosed and complete ingestion/search timing succeeds.

## Confirmed native assertion fixed — same worktree

The operator supplied `llama.log`. Both ledger failures align with
`GGML_ASSERT(n_tokens_all <= cparams.n_batch)` and SIGABRT/core dumps in
agentix-llama. The active backend capped embedding context/decode batch at 4,096
tokens and passed arbitrarily tokenized inputs directly to native decode. The
indexer's 8,000-byte bound did not guarantee that smaller token budget. This is a
confirmed input-bound failure, not evidence of GPU out-of-memory.

The backend now checks actual token counts against context and the configured batch
limit before encode/decode. Oversized inputs return ContextExceeded and structured
HTTP 400 on both embedding routes; a later valid request still succeeds. The
embedding cap is now 8,192 tokens, bounded by model context, accommodating intact
8,000-byte code chunks without truncation or averaging partial embeddings. Ingest
embedding errors also identify the repository/collection/files for diagnosis. No
storage schema, model identity, vector dimensions or NixOS option changed.

Final serial workspace suite: **207 passed, 0 failed, 3 ignored**, across 46 suites,
with the pinned all-MiniLM GGUF actually supplied. This includes a real-model backend
overflow/recovery test and both HTTP embedding handlers rejecting oversized inputs
then accepting a normal request. The fixture tests ran on CPU; live CUDA/Jina
validation and complete ledger ingestion require updating the remote inference
service. Existing completion-loop counter and three test unwrap lint findings were
cleaned up without changing their assertions. Strict all-target Clippy for inference
and indexer, Rust formatting and diff checks passed. Canonical Nix builds of the CUDA
inference package, indexer and MCP passed on the final source.

Evidence: `ledger-inference-workspace-tests.log`, `ledger-native-{embed,route}-test.log`
and `ledger-inference-*-build.{json,log}` in the ignored validation directory. The
deployment/resume commands are in quickstart.md. The user's raw llama.log remains
untracked and is not part of this change.

## Completed live ledger validation after operator deployment

The operator deployed the inference fix. A 9,001-token Jina request returned HTTP
400 with context_window=8192; an immediately following valid request returned a
1,536-dimensional embedding. The service remained usable throughout the large run.

GitHub master revision `f4a3026c9baed7845709befc8d66989ac9ecb765` finished with
**18,867 code chunks from 1,499 files and 505 documentation chunks**. The successful
resume command took **422 seconds (7m02s)** including clone and code/docs ingestion;
it reused the 2,486 chunks from the failed first attempt. This is not a clean full
ingestion timing. It embedded 16,381 new code chunks and 505 doc chunks, skipped 177
unchanged code files, logged no embedding retries, and persisted complete=true and
docs_indexed=true. No code chunks have missing embeddings. Comparing the untruncated
GitHub tree with indexed Cabal paths found all **29 package manifests**, with no
missing/extra manifests; every package directory also contains indexed Haskell code.
A subsequent completed-revision check skipped ingestion and took 4.237s including
the fresh Git clone.

Explicit index maintenance across the six collections took **3.835s**. Measurements
use the real packaged stdio MCP, project/repo filters, warm remote Jina embeddings,
no reranking, and six small queries repeated three times. These are observed samples,
not a throughput or percentile SLO; latency includes the MCP tool round trip and
remote inference where applicable.

| Tool | Samples | Median | Range |
| --- | ---: | ---: | ---: |
| BM25 code | 6 | 17ms | 14–20ms |
| Hybrid code | 9 | 90ms | 83–163ms |
| Hybrid docs | 3 | 33ms | 29–65ms |

Before maintenance, two BM25 requests took 74–91ms and three hybrid code requests
took 167–247ms. Two simultaneous MCP processes completed their six-query sequences
in about 0.41s each; their hybrid requests took 98–178ms. There were no protocol or
storage errors, but each repeated the same known relevance miss described below.

**Relevance finding retained, not counted as an all-pass result:** the query
“calculate the minimum lovelace required for a transaction output” returned
`babbageMinUTxOValue` second before maintenance, but it fell out of the top five
after maintenance, reproducibly across all three repeats and both concurrent clients.
Exact identifier/BM25 retrieval still finds the stored function. Requesting 100
hybrid results finds the function at rank 63. This changes candidate breadth (the
store normally uses max(60, requested_limit)), so it does not by itself isolate ANN
recall from fusion effects. Maintenance rebuilt both lexical and vector indexes;
do not attribute the difference solely to HNSW without an exact-vs-ANN comparison.

The existing optional BGE reranker took 2.619–4.826s for the three semantic queries
and did not recover that minimum-ADA function in the top five. Its normal pool is
only 20 results, excluding the target in this case. The original loose substring
checks passed with reranking, but manual target inspection shows the semantic miss
remains. No ranker, candidate-pool default, embedding representation or service
configuration was changed for this experiment. A useful next step is a curated
query/target set and controlled retrieval/fusion/pool comparisons before enabling
reranking globally. The pre-existing get_file reconstruction limitation also remains.

Evidence under the ignored validation directory: `ledger-github-resume*`,
`ledger-source-tree.json`, `ledger-package-{coverage.json,code-counts.jsonl}`,
`ledger-{completion-state,final-repositories,final-doc-count,final-missing-vectors}.json`,
`ledger-search-{before,after}-optimize*`, `ledger-search-timing-summary.json`,
`ledger-{concurrent-a,concurrent-b,rerank,min-ada-candidates}*`, and
`ledger-{oversized,recovery}-probe*`. Source and client packages remain unchanged;
only this validation record and task completion were updated.
