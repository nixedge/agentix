# Model-management contract v1

All new public paths are under `/v1/agentix`. DTOs live in agentix-api. Existing `/v1/models`, `/api/tags`, `/api/pull` retain their documented behavior. Legacy inference APIs preserve local request/response shapes; cloud targets require the breaking 0.2 migration to typed escalation described in [agent.md](agent.md). the new UI does not infer residency from these legacy lists.

## Endpoints

| Method/path | Input | Output/behavior |
|---|---|---|
| GET `/models` | optional cursor, limit (1–100) | items: ModelRecord[], next_cursor; fresh installed/resident/capability snapshot |
| GET `/catalog/huggingface/models` | q, cursor, limit (1–50) | repository IDs, display metadata, next_cursor; no auto-download |
| GET `/catalog/huggingface/files` | repo_id, revision, cursor | exact supported/unsupported file entries, full commit, size, SHA-256 when available, license/gating metadata |
| POST `/model-operations` | idempotency_key; warm(model_id) OR install_and_warm(repo_id, revision_commit, filename, expected_sha256, expected_bytes) | 202 {operation_id, status_url}; validates confirmed metadata and resource limits |
| GET `/model-operations/{id}` | none | current ModelOperation including terminal result/error |
| GET `/model-operations/{id}/events` | Last-Event-ID optional | ordered SSE progress snapshots; if history expired send reset snapshot |
| POST `/model-operations/{id}/cancel` | none | current snapshot; idempotent; terminal ready is not undone |
| POST `/models/tokenize` | model_id, bounded text | count and context_limit from selected backend tokenizer |

Model selection is session-local and journaled by ax; no endpoint changes a global default for unrelated users/sessions. When model readiness succeeds, ax atomically commits the selection at an idle boundary, after context compatibility checks. Concurrent UI requests use a selection generation so a late response cannot override a newer selection.

## Errors and lifecycle

Envelope: `{error:{code,message,retryable,operation_id?,retry_after_seconds?}}`. HTTP 400 invalid request; 403 management disabled/gated access; 404 missing model/file/job; 409 conflict/busy; 413 limit exceeded; 422 unsupported format/template or missing integrity metadata; 429 quota/rate limit; 503 service unavailable. Never include tokens, signed URLs, raw upstream authorization headers or arbitrary local paths in messages.

Progress states follow data-model.md. SSE is observational: disconnect does not cancel a download; explicit cancel does. Metadata polling supports reconnect. One idempotency key maps to one request body; conflicting reuse returns 409. Keep job snapshots for 24 hours after terminal state; stale IDs return 404. Jobs persist minimal state under the daemon model directory with private permissions. On daemon restart, nonterminal jobs become interrupted and need explicit retry; installed verified blobs remain reusable.

Download progress is bytes/total, with stage text when byte totals do not apply. Size and SHA-256 must be known before confirmation; otherwise explain that verified installation is unsupported for that file. Never interpret an ETag as SHA-256 without validated upstream semantics. Do not execute repository scripts or download all files implicitly. Split files are shown as unsupported in v1.

## Daemon and worker boundary

Hub access and tokens remain daemon-owned. Store writes use agentix-infer library code under bounded blocking work or async I/O; model FFI remains on llama workers. The existing private UDS service adds list_residency, ensure_loaded, tokenize and cancellation support using shared contract types. No new worker TCP listener. A completion's guard protects its model from eviction until generation is actually released; UI disconnect alone does not establish FFI cancellation.

Only administrator-enabled loopback/trusted-LAN management is supported initially. Document actual bind/access controls and test forbidden-source rejection. Do not mistake absence of user credentials in a jail for authentication of arbitrary remote clients. Legacy control endpoints must receive the same management access policy where they mutate shared model state.
