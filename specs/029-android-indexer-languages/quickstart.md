# Android code search

Build the updated indexer with `nix build .#agentix-indexer`. Use the same storage/embedding environment as existing ingestion:

```sh
./result/bin/ingest code /path/to/repo --project agentix
```

This incremental command adds previously undiscovered Kotlin/Android files without re-embedding unchanged indexed files. Re-ingest completed Git imports with their original revision/branch/tag arguments plus `--force` instead.

Use MCP `bm25_search` for exact identifiers or `search_code` for semantic questions. Set `language` to `kotlin`, `java`, `xml`, `groovy`, or `properties`. Kotlin supports existing `symbol_kind` filters such as `function`, `class`, `method`, and `property`. XML/Gradle/properties have no symbol kind; omit that filter. Kotlin scripts without declarations use line chunks as well.

Android build output, `.gradle`, `.kotlin`, `.idea`, and ignored files are excluded. No Android rebuild or database migration is required. Deploy the updated indexer wherever ingestion runs and refresh the index; the existing search server can query the added records.
