# Tasks: Tool Calling for agentix-llama

**Feature Branch**: `015-llama-tool-calling`
**Spec**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md)
**Total Tasks**: 18 | **Parallelizable**: T002–T005, T012–T013, T015

---

## Phase 1: Setup

- [ ] T001 Add `minijinja = { version = "2", features = ["json"] }` to `agentix-llama/Cargo.toml`

---

## Phase 2: Foundational — Type Changes

These four tasks touch different files and can run concurrently. Must all complete before Phase 3.

- [ ] T002 [P] Add `tool_calls: Option<serde_json::Value>` and `tool_call_id: Option<String>` fields (with `#[serde(skip_serializing_if = "Option::is_none")]`) to `ChatMessage` in `agentix-api/src/lib.rs`, before the `#[serde(flatten)]` field
- [ ] T003 [P] Add `tools: Option<Vec<serde_json::Value>>` field (with `#[serde(skip_serializing_if = "Option::is_none")]`) to `ChatCompletionRequest` in `agentix-api/src/lib.rs`, before the `#[serde(flatten)]` field
- [ ] T004 [P] Add `tool_calls: Option<serde_json::Value>` and `tool_call_id: Option<String>` fields (with `#[serde(skip_serializing_if = "Option::is_none")]`) to `CompletionMessage` in `agentix-infer/src/lib.rs`
- [ ] T005 [P] Add `tools: Option<Vec<serde_json::Value>>` field (with `#[serde(skip_serializing_if = "Option::is_none")]`) to `CompletionRequest` in `agentix-infer/src/lib.rs`; leave `CompletionRequest::new()` setting `tools: None`

---

## Phase 3: User Story 1 — Structured Tool Call Responses

**Story Goal**: A POST `/v1/chat/completions` request with a `tools` array returns `finish_reason: "tool_calls"` and a structured `tool_calls` array when the model decides to invoke a tool.

**Independent Test**: Send the curl command from `quickstart.md` Test 1. Verify `finish_reason == "tool_calls"` and `tool_calls[0].function.name == "todo_list"`.

### Implementation

- [ ] T006 [US1] Add `ToolCallResult` struct (fields: `id: String`, `name: String`, `arguments: String`) to `agentix-llama/src/lib.rs` — internal only, no `pub`
- [ ] T007 [US1] Implement `parse_tool_calls(output: &str) -> Result<Vec<ToolCallResult>, String>` in `agentix-llama/src/lib.rs`: scan for `<tool_call>` / `</tool_call>` markers, parse each body as `serde_json::Value`, extract `name` as string, re-serialize `arguments` object as JSON string (or use string as-is), generate `uuid::Uuid::new_v4()` formatted as `"call_{uuid}"` for each `id`; return `Err(String)` if a marker is found but the body is not valid JSON or lacks a `name` field
- [ ] T008 [US1] Modify `apply_chat_template` in `agentix-llama/src/lib.rs` to accept `tools: Option<&[serde_json::Value]>` as a third parameter; when `tools` is `None` or empty, use the existing `model.apply_chat_template()` path unchanged; when tools are present: call `model.chat_template(None)?.to_str()?.to_string()` to get the Jinja2 string, build a `minijinja::Environment` with the `json` feature (tojson filter available by default), construct the messages context as a `serde_json::Value` array from `messages` (each with `role`, `content`, and optionally `tool_calls`/`tool_call_id`), render with `{messages, tools, add_generation_prompt: true}`, return rendered string or map error to `String`
- [ ] T009 [US1] Update `complete_sync` in `agentix-llama/src/lib.rs` to pass `req.tools.as_deref()` as the third argument to `apply_chat_template`
- [ ] T010 [US1] In `chat_completions_handler` in `agentix-llama/src/main.rs`: (a) extract `let tools = api_req.tools.clone()` from the typed field; (b) when building each `CompletionMessage` from `ChatMessage`, also copy `tool_calls: msg.tool_calls.clone()` and `tool_call_id: msg.tool_call_id.clone()`; (c) set `req.tools = tools` on the `CompletionRequest` after construction
- [ ] T011 [US1] In the non-streaming response path of `chat_completions_handler` in `agentix-llama/src/main.rs`: after accumulating `full_content`, call `parse_tool_calls(&full_content)`; if `Ok(calls)` and calls is non-empty, serialize the response with `"content": null`, `"tool_calls": [...]`, `"finish_reason": "tool_calls"`; if `Err(e)`, return HTTP 500 with `{"error": format!("tool call parse error: {e}")}`; otherwise keep existing response construction unchanged; add `tracing::warn!` when `tools` is present and `stream: true` noting streaming tool calling is not supported

### Tests

- [ ] T012 [P] [US1] Write unit tests for `parse_tool_calls` in `agentix-llama/src/lib.rs`: `single_tool_call` (one marker, assert name and arguments correct), `multiple_tool_calls` (two consecutive markers, assert two results), `no_markers` (plain text, assert empty vec returned as Ok), `malformed_json_body` (valid markers but invalid JSON inside, assert Err returned)
- [ ] T013 [P] [US1] Write unit tests for deserialization in `agentix-api/src/lib.rs`: `chat_message_with_tool_calls_round_trips` (assistant message with `tool_calls` array deserializes and re-serializes correctly), `chat_message_with_tool_call_id_round_trips` (tool result message with `tool_call_id`), `chat_completion_request_with_tools_round_trips` (request with `tools` array)

---

## Phase 4: User Story 2 — Argument Well-Formedness

**Story Goal**: `function.arguments` in every returned tool call is a valid JSON string and the response handles unparseable tool call bodies with an explicit 500 error rather than silently corrupting the response.

**Independent Test**: Send a request where the model is likely to produce a tool call with arguments (e.g. a `todo_create` tool with a required `title` param and prompt "add a todo called Buy milk"). Verify `JSON.parse(tool_calls[0].function.arguments)` succeeds and has a `title` field.

### Implementation

- [ ] T014 [US2] In `parse_tool_calls` in `agentix-llama/src/lib.rs`: after extracting the JSON body, if `arguments` is a JSON object re-serialize it via `serde_json::to_string` (ensures compact, valid JSON string); if `arguments` is already a string in the JSON body use it as-is; if the body has no `arguments` key use `"{}"` as the default; this guarantees `function.arguments` is always a valid JSON-encoded string in the output

### Tests

- [ ] T015 [P] [US2] Extend unit tests for `parse_tool_calls` in `agentix-llama/src/lib.rs`: `arguments_as_object_serialized_to_string` (body has `"arguments": {"key": "val"}`, assert result `arguments == r#"{"key":"val"}"#`), `arguments_absent_defaults_to_empty_object` (body has only `"name"`, assert `arguments == "{}"`), `arguments_as_string_preserved` (body has `"arguments": "{\"key\":\"val\"}"`, assert string preserved as-is)

---

## Phase 5: Polish

- [ ] T016 Run `nix develop --command cargo fmt --all --check` from `agentix-llama/`, `agentix-infer/`, `agentix-api/`; fix any formatting issues
- [ ] T017 Run `nix develop --command cargo clippy -- -D warnings` across affected crates; fix all warnings (pay attention to unused field warnings on new optional fields)
- [ ] T018 Run `nix develop --command cargo test -p agentix-api -p agentix-infer -p agentix-llama` and confirm all tests pass

---

## Dependencies

```
T001 → T002, T003, T004, T005
T002 → T010, T013
T003 → T010, T013
T004 → T008, T009, T012
T005 → T009
T006 → T007
T007 → T011, T012, T014
T008 → T009
T009 → T010
T010 → T011
T011 → T016, T017
T012 → T018
T013 → T018
T014 → T015
T015 → T018
T016, T017, T018 → done
```

## Parallel Execution Examples

**Phase 2** (all four run concurrently — different files):
```
Task T002: agentix-api/src/lib.rs (ChatMessage fields)
Task T003: agentix-api/src/lib.rs (ChatCompletionRequest field)  ← same file as T002, run after
Task T004: agentix-infer/src/lib.rs (CompletionMessage fields)
Task T005: agentix-infer/src/lib.rs (CompletionRequest field)   ← same file as T004, run after
```

Note: T002+T003 touch the same file; run T002 first, then T003. T004+T005 touch the same file; run T004 first, then T005. T002+T003 can run in parallel with T004+T005.

**Phase 3 tests** (T012 and T013 touch different files):
```
Task T012: agentix-llama/src/lib.rs (parse_tool_calls unit tests)
Task T013: agentix-api/src/lib.rs (deserialization unit tests)
```

## Implementation Strategy

### MVP (US1 only — Phases 1–3)

1. Complete Phase 1: Add minijinja dep (T001)
2. Complete Phase 2: Type changes (T002–T005)
3. Complete Phase 3: Core tool calling mechanics (T006–T013)
4. **STOP and VALIDATE**: hit the gateway directly with the curl from quickstart.md Test 1 — model should return `finish_reason: "tool_calls"` with a `todo_list` call
5. VALIDATE Test 3 (regression): tool-free request still returns `finish_reason: "stop"` with normal content

### Incremental Delivery

- **After Phase 4** (US2): Argument serialization is consistent and error paths are robust — safe to wire up `agentix-agent` and test end-to-end (quickstart.md Test 5)
- **After Phase 5**: Ship-ready — all quality gates pass
