# Persistent data model

Six searchable tables: code_chunks, documents, github_issues, github_prs, github_issue_comments, github_pr_comments. Each has stable key, payload JSON, common repository/file/chunk/language/symbol/doc-kind/state/project filter columns, content and nullable raw fixed-size-list embeddings. Payload preserves original source fields and timestamps; legacy embedding-null rows remain available to keyword search. Code/docs keys are (repo_path,file/source_path,chunk_index); issues/PRs use (repo,number); comments include repo/comment_id and review comment type where applicable.

Schema metadata carries agentix.schema=1, the configured embedding-model identity and dimensions. Open refuses incompatible tables. Writes validate key/content/project/vector shape before replacing existing rows. HNSW uses cosine and raw vectors; missing/small indexes use exhaustive retrieval, and incremental unindexed fragments remain visible.

The metadata table has namespace,key,payload columns and schema version. repo_index keys are repository paths. sync_state keys are source_name:scope_key and retain watermarks/error counters. Metadata updates that merge state run under the process writer lock. Watermarks advance monotonically after successful persistence; error recording preserves the previous watermark.

One writer lock file lives in the index directory. A Store handle also serializes its own concurrent writes. Independent readers refresh committed versions. A changed file is replaced by one merge with stale-key deletion after all embeddings are ready; empty/deleted files remove their previous rows. Project arrays merge with prior file bindings. There is no transaction spanning every collection or an entire repository.

Remote access does not change the persisted schema. HTTP v1 carries tagged storage operations
and protocol/model/dimension assertions. Metadata compare-and-swap compares the entire prior
JSON value (including absence) while holding the same local writer mutex/file lock used by
other mutations. A definitive conflict can rerun a pure client update closure; network errors
do not trigger automatic mutation retries. Concurrent file replacements retain last-commit
semantics rather than combining independently prepared file contents.
