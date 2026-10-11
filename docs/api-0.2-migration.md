# API and daemon 0.2 migration

Feature 020 introduces API/daemon version 0.2.0 and schema-version-1 DTOs under `/v1/agentix`. Local OpenAI-compatible request and response shapes remain available. The coding checkpoint implements local routing, structured cloud escalation, readiness/tokenization and headless event contracts. Model installation and orchestration routes remain later work.

The 0.2 daemon rejects legacy cloud targets on `/v1/chat/completions`, `/v1/responses`, `/v1/messages`, embeddings and Ollama-compatible inference routes with HTTP 403 `structured_escalation_required`. Registered local model IDs/aliases are resolved first. There is no compatibility switch restoring unstructured cloud forwarding. Escalation is disabled unless the daemon sets `AGENTIX_ALLOW_CLOUD_ESCALATION=1`; when `AGENTIX_API_KEY` is configured, callers must also authenticate. Provider executors require a router-issued `ValidatedEscalation` value that callers cannot construct or deserialize.

Cloud callers must use `POST /v1/agentix/escalations` with the following shape, subject to daemon authorization:

```json
{
  "schema_version": 1,
  "knowledge": "Relevant evidence already collected locally",
  "question": "The specific unresolved question",
  "local_insufficiency": "Why local sources cannot answer it",
  "target": {"provider": "open_ai", "model": "configured-cloud-model"},
  "context": {
    "session_id": "00000000-0000-4000-8000-000000000001",
    "turn_id": "00000000-0000-4000-8000-000000000002",
    "invocation_id": "call-1"
  }
}
```

Provider values are `open_ai`, `anthropic`, `open_router`. No API keys, arbitrary upstream URLs or caller-supplied approval flags belong in this body. A typed request is necessary but does not itself grant permission.

New DTOs reject unknown fields, unsupported schema versions, empty/oversized rationale, malformed UUIDs, floating model revisions, invalid SHA-256 digests and zero assignment generations. The shared request decoder caps JSON at 1 MiB and avoids echoing rejected values. HTTP handlers must enforce transport body limits and use the decoder; semantic graph, filesystem, permission and capacity checks remain server responsibilities.

Worker relay requests carry `assignment: {assignment_id, generation}` and no raw capability or role. Only the host supervisor receives capability issuance responses. Tokens are redacted in Debug output; they still require explicit secret handling when constructing authenticated host-to-daemon headers.

Headless event envelopes retain `schema_version`, `session_id`, `turn_id`, `sequence`, `type` and `payload`. New model management and orchestration state enums distinguish preparation, revocation and review from completion. Consumers must not treat a submitted worker result as an approved completed task.
