# Feature Specification: llama.cpp Grammar Enforcement and Responses API

**Feature Branch**: `013-grammar-responses-api`
**Created**: 2026-08-20
**Status**: Draft
**Input**: User description: "agentix-llama grammar enforcement and responses API"

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Structured JSON Output from Chat Completions (Priority: P1)

When Ariadne sends a chat completion request asking for JSON output, the inference server must guarantee the response is valid JSON — not just instructed to produce JSON, but enforced at the output level. Both a generic "give me any JSON" request and a "give me JSON matching this exact schema" request must produce parseable, schema-conforming output every time.

**Why this priority**: Ariadne's core workflows depend on parsing structured data from model responses. Prompt-level JSON instructions are unreliable — models sometimes produce trailing text, markdown fences, or schema deviations. Token-level enforcement eliminates these failures entirely and is the foundation for all downstream data processing.

**Independent Test**: Send a chat completion request with `response_format: { type: "json_object" }` and verify the response body parses as valid JSON with zero additional text. Send a second request with a specific JSON schema and verify the response matches the schema exactly.

**Acceptance Scenarios**:

1. **Given** a chat completion request with `response_format: { type: "json_object" }`, **When** inference completes, **Then** the response content is valid JSON parseable without error and contains no extraneous text outside the JSON structure.
2. **Given** a chat completion request with `response_format: { type: "json_schema", "json_schema": { "schema": { ... } } }`, **When** inference completes, **Then** the response content is valid JSON that conforms to the provided schema — all required fields present, types correct, no additional top-level keys unless the schema allows them.
3. **Given** a chat completion request with no `response_format` field, **When** inference completes, **Then** the response is unchanged from current behavior — no grammar enforcement applied.
4. **Given** a request with `response_format` containing an invalid or unparseable JSON schema, **When** the server processes the request, **Then** the server returns a 400 error with a clear message identifying the schema problem before inference begins.

---

### User Story 2 — Responses API Endpoint for Ariadne (Priority: P2)

Ariadne uses the OpenAI Node.js SDK's `client.responses.create(...)` method, which targets the `/v1/responses` endpoint. This endpoint does not exist yet on the inference server, causing SDK calls to fail entirely. The endpoint must accept the Responses API request format — including the `input` array, `developer` role, and `text.format` for schema-constrained output — and return a Responses API-shaped reply.

**Why this priority**: Without this endpoint, one of Ariadne's two SDK call patterns is completely broken. P2 because P1 (json_object/json_schema enforcement in chat completions) delivers independent value; this story adds the second call pattern Ariadne requires.

**Independent Test**: Use the OpenAI Node.js SDK pointed at the local server to call `client.responses.create(...)` with a `developer` role message and a `text.format` json_schema constraint. Verify the response has `output[0].content[0].text` containing valid JSON matching the schema, and `output_text` equals that same text.

**Acceptance Scenarios**:

1. **Given** a POST to `/v1/responses` with `input: [{ role: "developer", content: [{ type: "input_text", text: "..." }] }]`, **When** inference completes, **Then** the response contains `object: "response"`, an `output` array with one message item whose `role` is `"assistant"`, and an `output_text` convenience field with the same text as `output[0].content[0].text`.
2. **Given** a `/v1/responses` request with `text.format: { type: "json_schema", schema: { ... } }`, **When** inference completes, **Then** `output[0].content[0].text` is valid JSON conforming to the schema — enforced at the output level, using the same enforcement mechanism as User Story 1.
3. **Given** a `/v1/responses` request with `max_output_tokens` set, **When** inference runs, **Then** the model output is limited to at most that many tokens.
4. **Given** a `/v1/responses` request with `store: true` or `store: false`, **When** the server processes it, **Then** the server accepts the field without error and ignores it (no server-side storage is implemented).
5. **Given** a `/v1/responses` request with `reasoning: { effort: "..." }`, **When** the server processes it, **Then** the field is accepted without error and ignored (no special reasoning mode is implemented).
6. **Given** a `/v1/responses` request where the model produces a refusal, **When** inference completes, **Then** the response contains a content item with `type: "refusal"` in `output[0].content`.

---

### Edge Cases

- What happens when the JSON schema provided in `response_format` or `text.format` uses `$ref` to external URIs? → Return a 400 error; only self-contained inline schemas are supported.
- What happens if token budget is exhausted mid-object during grammar-constrained generation? → Return whatever partial content was produced; finish reason indicates truncation.
- What happens when `response_format.type` is an unknown value (e.g., `"text"`)? → Treat as no grammar enforcement — pass through unchanged, no error.
- What happens when `input` in `/v1/responses` contains mixed `developer`, `user`, and `assistant` turns? → Map `developer` to system, pass `user` and `assistant` through unchanged.
- What happens when `content` in a `/v1/responses` input item is a plain string rather than an array? → Accept both forms; normalize internally.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The server MUST apply grammar-based output enforcement when `response_format.type` is `"json_object"`, guaranteeing the response content is valid JSON.
- **FR-002**: The server MUST apply grammar-based output enforcement when `response_format.type` is `"json_schema"`, guaranteeing the response content conforms to the provided JSON schema.
- **FR-003**: The server MUST leave existing behavior completely unchanged when `response_format` is absent or `response_format.type` is `"text"`.
- **FR-004**: The server MUST return a 400 error with a descriptive message when a provided JSON schema cannot be converted to a grammar constraint, and MUST NOT begin inference in that case.
- **FR-005**: The server MUST implement `POST /v1/responses` accepting the OpenAI Responses API request format.
- **FR-006**: The server MUST map the `developer` role in `/v1/responses` `input` arrays to a system message for inference.
- **FR-007**: The server MUST apply grammar-based output enforcement in `/v1/responses` when `text.format.type` is `"json_schema"`, using the same enforcement mechanism as FR-002.
- **FR-008**: The server MUST respond to `/v1/responses` with the Responses API response shape: `id` (generated), `object: "response"`, `model` (echoed), `output` array with one assistant message item, and `output_text` convenience field.
- **FR-009**: The server MUST respect `max_output_tokens` in `/v1/responses` requests as a token generation limit.
- **FR-010**: The server MUST silently accept and ignore `store` and `reasoning` fields in `/v1/responses` requests without returning an error.
- **FR-011**: When the model produces a refusal, the server MUST include a refusal content item (`type: "refusal"`) in `output[0].content` in the `/v1/responses` response.

### Key Entities

- **GrammarConstraint**: A specification of how model output is constrained during generation — either a generic JSON constraint (any valid JSON object) or a specific schema constraint derived from a provided JSON schema definition.
- **ResponsesRequest**: An OpenAI Responses API request payload containing `input` (array of role-content message items), optional `text.format` schema constraint, optional `max_output_tokens`, and ignorable `store`/`reasoning` fields.
- **ResponsesResponse**: An OpenAI Responses API response containing a generated `id`, `object: "response"`, the echoed `model`, an `output` array of message items, and an `output_text` convenience string.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of chat completion responses with `response_format: { type: "json_object" }` parse as valid JSON — zero parse failures across a 50-request test run.
- **SC-002**: 100% of chat completion responses with a `response_format` JSON schema conform to that schema — zero schema violations across a 50-request test run.
- **SC-003**: Grammar enforcement adds no more than 10% to median inference latency compared to equivalent unconstrained requests.
- **SC-004**: The OpenAI Node.js SDK `client.responses.create(...)` method succeeds end-to-end against this server without any client-side workaround or modification.
- **SC-005**: All existing `/v1/chat/completions` requests without `response_format` continue to work identically — zero regressions in a full regression run.
- **SC-006**: Invalid schema inputs are rejected with a 400 response within 50ms — inference is never started for invalid inputs.

## Assumptions

- The inference server already handles `/v1/chat/completions` correctly; this feature extends the existing handler without replacing it.
- Grammar enforcement operates at the token-sampling level during inference — not by post-processing the output after generation.
- The generic JSON grammar (for `json_object`) is the standard JSON GBNF grammar included with llama.cpp and requires no custom authoring.
- JSON schema to grammar conversion uses the utility bundled with llama.cpp; schemas using `$ref` to external resources or advanced features unsupported by that utility are out of scope.
- The `reasoning` field is accepted and ignored; no chain-of-thought or extended reasoning inference mode is implemented in this feature.
- Server-side conversation storage (the `store` field in Responses API) is out of scope; the server remains stateless per request.
- Ariadne is the primary caller; the OpenAI Node.js SDK is the reference client for compatibility testing.
- `developer` role is functionally equivalent to `system` for inference; no behavioral distinction is needed.
