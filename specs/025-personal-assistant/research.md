# Research and decisions

User selected Sled authority + LanceDB retrieval after workload analysis. Maintainability/modularity are explicit requirements. Android voice transcription is owner-tested; default-assistant activation remains deferred until backend utility is real. User is merging worktree023 concurrently; isolate025 and review the merged storage API before Lance integration.

Existing home-agent stores have useful behaviour but separate read/modify/write paths and default Sled durability do not yet establish the desired command acknowledgement contract. Session arrays are replaced as a whole. New namespaces/API avoid silently reinterpreting legacy data; migration is explicit later.

Pinned dependencies at base: Sled0.34.7; LanceDB0.40.0/Lance13. New foundation reuses locked serde/chrono/uuid/async-trait/tokio/thiserror versions. Sled's documented atomic batches, serializable transactions and explicit flush support the selected owned-DB/mutex/atomic-batch design. The single-process ownership constraint matches a private backend serving many clients. All Sled operations are offloaded to blocking workers.

Source audit through agentic-nix after the jail restart: `crates.io::sled-0.34.7`, `src/tree.rs::apply_batch_inner` seals a recovery guard for all-or-none recovery. `src/pagecache/logger.rs::flush` and `src/tree.rs::Drop` also confirm that both acknowledgement and final destruction can perform blocking I/O. The adapter therefore offloads destruction as well as read/write/flush and exposes an awaited close for immediate reopen. These source guarantees are distinct from physical power-loss qualification.

Merged code-index review: `agentix-store/src/lib.rs` owns code/GitHub collection keys and a required embedding model/dimension profile. Its Arrow 58 row batches, typed FTS queries and single-row merge patterns are reused in the separate personal adapter without importing the code-index record schema or defaults. The initial personal profile is `personal-lexical-v1` (zero vector dimensions); semantic model quality/evaluation remains unperformed T011 work.

Sources:
- https://docs.rs/sled/0.34.7/sled/struct.Tree.html (batch, flush, range and CAS semantics)
- https://github.com/spacejam/sled#known-issues-warnings (beta/disk-format warnings require recovery qualification)
- Existing023 storage contract and validation: per-collection commits, not a general multi-collection application transaction.
- https://docs.lancedb.com/tables/consistency and /tables/update (freshness and mutable projections)

Inference distinction: jina-code-embeddings-1.5b is an embedding model based on Qwen2.5-Coder; it is not the assistant conversation model. Existing agentix-agent/dev.toml configures Qwen2.5-7B-Instruct Q8_0. Keep as baseline, measure actual command correctness before replacing it. Qwen weights8.10GB and Jina weights1.65GB; runtime/context budgets are additional. Owner approved agentix-llama now, possible agentix-qwen later, and noted spare48GB unified-memory Mac mini capacity. No model-serving rewrite or Mac deployment is part of A.
