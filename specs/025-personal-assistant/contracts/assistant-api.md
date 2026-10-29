# Assistant HTTP v1 and application contract

`POST /v1/assistant/tell`, on the existing agentix-agent listener. Opt-in configuration and `Authorization: Bearer <owner token>` are required. Maximum JSON request body 16KiB; UTF-8 instruction 1..4096 bytes. No logging of instructions/tokens. No model/provider selection from a phone request.

Request: `{request_id: UUID, conversation_id: UUID, text: string, timezone: IANA timezone}`. IDs must be nonnil. The immutable envelope binds the request ID globally. Server acceptance time is persisted, so retry cannot move relative-date context. A conversation has ordered append-only user/assistant messages and at most one unresolved turn.

Response: `{request_id, conversation_id, status, reply, outcomes: [CommandReceipt], records: [Record]}`. Status is saved, answer, clarification, failed or partial. Outcomes contain actual durable results; record data and revisions support correction/review. Save replies derive from receipts. Read-only responses are original snapshots on replay; send a new request ID to refresh. A partial multi-command outcome reports what committed and what failed.

Identical completed requests replay the original response. Reusing an ID for different input returns409. Concurrent admission or a different request in an unresolved conversation is retryable409/503; storage uncertainty and request timeout return503/504 with retry_same_request=true. Retain the complete original envelope and retry its ID; never generate a new ID to resolve an uncertain outcome. Inference or malformed proposed-action failure before writes returns a durable failed response with no outcomes. Authentication failures return401; invalid/oversized requests400/413. Internal errors expose stable codes, not database paths/model payloads.

Application ports are engine-free Rust traits. Inference consumes bounded recent messages, current/retrieved originals, accepted_at and timezone, then proposes one typed intent: add/show list, save note/task, edit list-item text, set task completion, delete by ID+revision, or clarification. Tool resolution checks exact named-list ambiguity and domain rules before producing a durable execution plan. Prepared plans are immutable and saved before mutation. Existing request receipts make crash/retry safe across sequential commands. The host owns transport/timeouts, local-model adapter and retry scheduling.

Phone integration follows server acceptance and reads its configured HTTPS base URL. Backend use is explicitly enabled and owner-token storage is private/excluded from backup. Android persists unresolved envelopes until a definitive response; cancellation cannot claim server rollback. WireGuard is external. No new public MCP listener, provider keys, VPN service or default-assistant change.

## Correction tools — owner approved

Replace the model-facing generic `update_record` action with `edit_list_item{id, expected_revision, text}` and narrowly scoped task-state updates. The item handler must load an original ListItem, validate its revision, and construct Command::Replace with the same list_id/completed fields and only the requested new text. A list/task/note ID cannot pass as an item edit. Task completion similarly preserves title/due_at; additional edits require explicit typed actions. Unchanged edits report no mutation rather than incrementing a revision and claiming success. Existing stored Command/PreparedPlan formats and the phone HTTP v1 envelope stay unchanged.

The owner approved this revision after reviewing the live Qwen failure. Generic replacement is no longer a model-facing action. Unsupported edits require clarification. Stored domain commands and prepared plans remain compatible.

## Capability answers

Internal `unsupported{capability:email|calendar|reminder|other}` generates a deterministic `answer` with empty outcomes/records. Email returns “I can't send email yet.” Calendar and reminder answers explicitly state their unavailable action. Providing contact details or confirming the action does not enable a capability. `clarify` is only for ambiguous supported operations. No new wire status or persisted-plan schema version is needed.

Android stores pending envelopes and the last definitive response in private backup-excluded preferences. Only a valid identity-matched v1 response clears a pending envelope. All HTTP/auth/transport/malformed/unknown results retain it; app cancellation stops waiting but does not imply rollback. Endpoint changes cannot redirect an unresolved request. Retry is an explicit user action; a new instruction is blocked while pending. App display acknowledges only validated server results.

## Two-step local inference

The local adapter first selects a typed operation from a small enum. Unsupported operations map directly to Intent::Unsupported and deterministic replies with one model call. Supported operations make a second bounded call whose schema permits only the selected action or clarification. The second schema offers clarification first; mismatched operations or unknown fields are rejected before plan preparation. Original IDs/current revisions are still validated by Rust. Both calls share the application's turn timeout; prepared/completed replays do not repeat either call. No wire envelope or stored journal schema changes.
