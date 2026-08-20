# Tasks: Grammar Enforcement and Responses API

**Input**: Design documents from `/specs/013-grammar-responses-api/`
**Branch**: `013-grammar-responses-api`
**Spec**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md)

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel with other [P] tasks in the same phase (different files, no unmet deps)
- **[Story]**: User story this task belongs to (US1 = grammar in chat completions, US2 = /v1/responses endpoint)
- All file paths are relative to the workspace root

---

## Phase 1: Setup

**Purpose**: Enable the `common` feature in `llama-cpp-2` which unlocks `LlamaSampler::grammar()` and `json_schema_to_grammar()`. Nothing else compiles correctly without this.

- [X] T001 Enable `common` feature for `llama-cpp-2` in `agentix-llama/Cargo.toml`: change `llama-cpp-2 = "0.1"` to `llama-cpp-2 = { version = "0.1", features = ["common"] }`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Add shared types to `agentix-api` and `agentix-infer` that both user stories depend on. US1 and US2 cannot be implemented until these types exist.

**⚠️ CRITICAL**: All Phase 3 and 4 tasks depend on Phase 2 completing first.

- [X] T002 Add `ResponseFormat`, `ResponseFormatType` (with `JsonObject`, `JsonSchema`, `Text`, `#[serde(other)] Unknown` variants), and `JsonSchemaSpec { name: Option<String>, schema: serde_json::Value, strict: Option<bool> }` types; add `response_format: Option<ResponseFormat>` field to `ChatCompletionRequest` in `agentix-api/src/lib.rs`
- [X] T003 Add `ResponsesRequest`, `ResponseInputItem`, `ResponseInputContent` (untagged enum: `Text(String)` and `Parts(Vec<ResponseInputPart>)`), `ResponseInputPart { part_type: String, text: Option<String> }`, `ResponseTextConfig`, `ResponseTextFormat { format_type: String, schema: Option<serde_json::Value>, name: Option<String> }` types to `agentix-api/src/lib.rs`
- [X] T004 Add `ResponsesResponse { id: String, object: String, model: String, output: Vec<ResponseOutputMessage>, output_text: String }`, `ResponseOutputMessage { msg_type: String, id: String, role: String, status: String, content: Vec<ResponseOutputContent> }`, and `ResponseOutputContent` (`#[serde(tag = "type", rename_all = "snake_case")]` enum with `OutputText { text: String }` and `Refusal { refusal: String }`) to `agentix-api/src/lib.rs`
- [X] T005 [P] Add `GrammarConstraint` enum (`Gbnf(String)` variant, derive `Debug Clone Serialize Deserialize`) and `grammar: Option<GrammarConstraint>` field to `CompletionRequest` in `agentix-infer/src/lib.rs`

**Checkpoint**: `cargo check -p agentix-api -p agentix-infer` must pass before Phase 3/4 begins.

---

## Phase 3: User Story 1 — Grammar Enforcement in /v1/chat/completions (Priority: P1) 🎯 MVP

**Goal**: When a caller sends `response_format: { type: "json_object" }` or `response_format: { type: "json_schema", json_schema: { schema: {...} } }`, the model output is guaranteed to be valid JSON (or schema-conforming JSON) via token-level grammar enforcement. Requests without `response_format` are unchanged.

**Independent Test**: Send `POST /v1/chat/completions` with `response_format: { type: "json_object" }` to the running `agentix-llama` binary, parse the `choices[0].message.content` as JSON — must succeed without error.

- [X] T006 [US1] Add `const JSON_GBNF: &str` containing the inline JSON GBNF grammar (root = object rule; value, object, array, string, number rules), and add `fn validate_and_convert_schema(schema: &serde_json::Value) -> Result<String, String>` that: (1) rejects schemas containing `"$ref"` whose value starts with `http` or `https` with an Err, (2) serializes the schema to a JSON string, (3) calls `llama_cpp_2::json_schema_to_grammar(&schema_str).map_err(|e| e.to_string())` and returns the GBNF string on success in `agentix-llama/src/main.rs`
- [X] T007 [P] [US1] Modify `complete_sync` in `agentix-llama/src/lib.rs` to conditionally rebuild the sampler chain with grammar first: if `req.grammar` is `Some(GrammarConstraint::Gbnf(gbnf))`, build `LlamaSampler::grammar(&model, &gbnf, "root").map_err(|e| InferError::Backend(format!("grammar init: {e:?}")))` and then call `LlamaSampler::chain_simple([grammar_sampler, LlamaSampler::top_k(40), LlamaSampler::top_p(...), LlamaSampler::temp(...), LlamaSampler::dist(...)])` — build a new chain from scratch with grammar as the first element (NOT appended to an existing chain); if `req.grammar` is `None`, build the existing chain unchanged
- [X] T008 [US1] In `chat_completions_handler` in `agentix-llama/src/main.rs`, after parsing `api_req`: extract `api_req.response_format`, match on `ResponseFormatType`: `JsonObject` → `Some(GrammarConstraint::Gbnf(JSON_GBNF.to_string()))`, `JsonSchema` → call `validate_and_convert_schema(schema)` and if Err return HTTP 400 `Json(json!({"error": msg}))`, else `Some(GrammarConstraint::Gbnf(gbnf))`, `Text | Unknown | None` → `None`; set `grammar` field on the `CompletionRequest` struct literal
- [X] T009 [US1] Add `#[cfg(test)] mod tests` in `agentix-llama/src/main.rs` (or extend existing) with four unit tests: (1) `json_object_format_produces_json_gbnf` — builds ResponseFormat with JsonObject, calls extraction logic, asserts GrammarConstraint::Gbnf containing "root ::= object"; (2) `valid_json_schema_converts_to_gbnf` — passes a simple schema `{"type":"object","properties":{"x":{"type":"string"}},"required":["x"]}`, asserts Ok with gbnf containing "root ::="; (3) `external_ref_schema_rejected` — passes schema `{"$ref":"https://example.com/s.json"}`, asserts Err; (4) `text_format_type_produces_no_grammar` — ResponseFormat with Text → None

**Checkpoint**: `cargo test -p agentix-llama` must pass. Send a manual request with `json_object` and parse the response as JSON.

---

## Phase 4: User Story 2 — /v1/responses Endpoint (Priority: P2)

**Goal**: `POST /v1/responses` accepts the OpenAI Responses API format, maps `developer` role to `system`, applies grammar enforcement when `text.format.type == "json_schema"`, and returns a response shaped as `{ id, object: "response", model, output: [{ type: "message", role: "assistant", content: [{ type: "output_text", text: "..." }] }], output_text }`.

**Independent Test**: Use the OpenAI Node.js SDK pointed at the local server: `await client.responses.create({ model: "...", input: [{ role: "developer", content: [{ type: "input_text", text: "Say hi" }] }] })` — must succeed and return a non-empty `output_text`.

- [X] T010 [US2] Implement `async fn responses_handler(State(state): State<AppState>, body: axum::body::Bytes) -> Response` in `agentix-llama/src/main.rs`: (1) parse `ResponsesRequest` from body or return 400; (2) resolve model or return 404; (3) map `input` to `Vec<CompletionMessage>` mapping `developer` → `system`, `user`/`assistant` → pass through; (4) normalize `ResponseInputContent`: `Text(s)` → use s directly, `Parts(parts)` → concatenate text of all `input_text` parts; (5) extract grammar from `req.text.as_ref().and_then(|t| t.format.as_ref())` using same logic as US1 (json_schema only); (6) build `CompletionRequest { messages, max_tokens: req.max_output_tokens, temperature: None, top_p: None, stop: vec![], grammar }`; (7) run via `state.engine.complete(&resolved, comp_req).await`, collect all chunks; (8) **FR-011 MVP scope**: always wrap output in `ResponseOutputContent::OutputText { text: full_text }` — add a `// FR-011: refusal detection requires a model-level signal not yet available in agentix-llama; all non-error completions are treated as OutputText` comment; (9) build `ResponsesResponse` with `id: format!("resp_{}", Uuid::new_v4())`, `object: "response"`, `model: req.model`, `output: [ResponseOutputMessage { msg_type: "message", id: format!("msg_{}", Uuid::new_v4()), role: "assistant", status: "completed", content: [ResponseOutputContent::OutputText { text: full_text }] }]`, `output_text: full_text`; return `Json(response)`
- [X] T011 [US2] Register the `/v1/responses` route in the `Router` in `agentix-llama/src/main.rs`: add `.route("/v1/responses", post(responses_handler))` to the existing router builder (add `uuid` to imports: `use uuid::Uuid;` — verify `uuid` is in `agentix-llama/Cargo.toml` and add it if missing with `features = ["v4"]`)
- [X] T012 [P] [US2] Add a `POST /v1/responses` forwarding route in `agentix-daemon/src/gateway/mod.rs`: add `.route("/v1/responses", post(responses_proxy_handler))` to the router builder; implement `responses_proxy_handler` that reads the body bytes and calls `proxy::forward(&state.llama_socket, Method::POST, "/v1/responses", headers, Body::from(body)).await` — identical pattern to the existing `chat_completions_handler` proxy, no routing logic needed at the handler level
- [X] T013 [US2] Add unit tests in `agentix-llama/src/main.rs` for the responses handler helpers: (1) `developer_role_maps_to_system` — build a `ResponseInputItem` with role "developer", assert mapping produces CompletionMessage with role "system"; (2) `string_content_normalizes_to_text` — `ResponseInputContent::Text("hello".into())` → content "hello"; (3) `array_content_concatenates_input_text_parts` — Parts with two input_text parts → concatenated text; (4) `store_and_reasoning_fields_ignored` — parse a JSON body with `store: true, reasoning: {"effort":"high"}` as ResponsesRequest, assert it deserializes without error

**Checkpoint**: `cargo test -p agentix-llama -p agentix-daemon` must pass. The `/v1/responses` route must be reachable via the daemon's Unix socket proxy.

---

## Phase 5: Polish & Cross-Cutting Concerns

**Purpose**: Verify quality gates, fix any clippy warnings, and confirm Nix build succeeds.

- [X] T014 Run `nix develop --command cargo test -p agentix-api` and fix any compilation or serde issues with the new types
- [X] T015 Run `nix develop --command cargo test -p agentix-infer` and fix any issues with GrammarConstraint in CompletionRequest
- [X] T016 Run `nix develop --command cargo test -p agentix-llama` and ensure all grammar + responses unit tests pass
- [X] T017 Run `nix develop --command cargo test -p agentix-daemon` and ensure forwarding route compiles and tests pass
- [X] T018 Run `nix develop --command cargo fmt --all --check` and fix any formatting issues (constitution Principle VIII gate 1)
- [X] T019 Run `nix develop --command cargo clippy --workspace -- -D warnings` and fix all warnings (pay special attention to unused imports from `llama_cpp_2` when `common` feature is enabled)
- [X] T020 Add integration test in `agentix-llama/tests/grammar_integration.rs` using the fixture GGUF model already pinned in the Nix flake: (1) `json_object_produces_valid_json` — spin up InferEngine with fixture model via `LlamaCppBackend::new()`, build a CompletionRequest with `grammar: Some(GrammarConstraint::Gbnf(JSON_GBNF))`, call `complete()`, collect output, assert `serde_json::from_str(&output).is_ok()`; (2) `no_grammar_output_unchanged` — same request with `grammar: None`, assert completion returns without error (regression guard); add `#[ignore]` attribute if the fixture model is not available in CI and document the skip reason
- [X] T021 Run `nix build .#agentix-llama` to verify the Nix build succeeds with the `common` feature enabled (constitution Principle VIII gate 5)
- [X] T022 Run `nix build .#agentix-daemon` to verify daemon changes compile and the Nix build succeeds (constitution Principle VIII gate 5 — all changed deliverables must have a passing nix build)

---

## Dependencies

```
T001 (Cargo.toml common feature)
  └── T002 (agentix-api ResponseFormat types)
  └── T003 (agentix-api ResponsesRequest types) [depends on T002]
  └── T004 (agentix-api ResponsesResponse types) [depends on T003]
  └── T005 [P] (agentix-infer GrammarConstraint) [parallel with T002-T004]

T002, T003, T004, T005
  └── T006 (US1: JSON_GBNF constant + validate_and_convert_schema helper)
  └── T007 [P] (US1: grammar sampler in lib.rs complete_sync)
  └── T008 (US1: extract response_format in chat_completions_handler) [depends on T006]
  └── T009 (US1: unit tests) [depends on T008]

T002-T005
  └── T010 (US2: responses_handler impl)
  └── T011 (US2: register /v1/responses route) [depends on T010]
  └── T012 [P] (US2: daemon forwarding) [parallel with T010-T011]
  └── T013 (US2: responses handler unit tests) [depends on T010]

T009, T013
  └── T014-T022 (Polish)
```

## Parallel Execution Examples

**After Phase 2 (T001-T005 complete), Phase 3 and start of Phase 4 can overlap**:
- T006 + T007 in parallel (different files: main.rs vs lib.rs)
- T012 in parallel with T010-T011 (different files: daemon vs llama)

**Within Phase 3**:
- T006 (main.rs helpers) and T007 (lib.rs sampler) can run in parallel
- T008 depends on T006 (uses validate_and_convert_schema)
- T009 (tests) depends on T008

**Within Phase 4**:
- T010, T011 sequential (same file)
- T012 parallel (different crate)
- T013 parallel with T012 (unit tests for helpers added in T010)

## Implementation Strategy

**MVP = Phase 1 + Phase 2 + Phase 3** (US1 only):
Grammar enforcement in `/v1/chat/completions` delivers independent value. Ariadne can use `response_format` in chat completions without the Responses API.

**Full delivery = MVP + Phase 4**:
US2 adds the `/v1/responses` endpoint, completing Ariadne's second call pattern.

**Test approach**: Unit tests (no model needed) for parsing and conversion logic. Integration tests against a running `agentix-llama` with a fixture model for end-to-end grammar enforcement verification.

## Task Summary

| Phase | Tasks | Files |
|-------|-------|-------|
| Setup | T001 | agentix-llama/Cargo.toml |
| Foundational | T002-T005 | agentix-api/src/lib.rs, agentix-infer/src/lib.rs |
| US1 (Grammar) | T006-T009 | agentix-llama/src/main.rs, agentix-llama/src/lib.rs |
| US2 (Responses API) | T010-T013 | agentix-llama/src/main.rs, agentix-daemon/src/gateway/mod.rs |
| Polish | T014-T022 | Verification runs, integration test, nix builds |

**Total tasks**: 22  
**Parallel opportunities**: T005‖T002-T004, T006‖T007, T012‖T010-T011, T013‖T012  
**MVP scope**: T001-T009 (US1 complete, grammar enforcement in chat completions)
