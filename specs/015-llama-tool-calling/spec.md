# Feature Specification: Tool Calling Support for Local Inference

**Feature Branch**: `015-llama-tool-calling`
**Created**: 2026-08-21
**Status**: Draft

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Structured Tool Calls Returned from Chat Completions (Priority: P1)

When a client sends a chat completion request that includes a `tools` array, the inference server must return a structured tool call response — not raw text that mentions tool names, but a machine-readable `tool_calls` array with `finish_reason: "tool_calls"` — identical in shape to the OpenAI specification. The model must use the tool definitions injected into its prompt via its native chat template to decide when and how to call tools.

**Why this priority**: The entire agent loop depends on structured tool dispatch. Without this, the harness that drives the home assistant agent cannot execute any tool — the model produces plain text and the agent cannot act on it. This is the foundational capability that makes every downstream tool (todo list, calendar, Gmail) functional.

**Independent Test**: Send a POST `/v1/chat/completions` with a `tools` array containing a `todo_list` function definition and a user message asking "what's on my todo list?". Verify the response has `finish_reason: "tool_calls"` and `message.tool_calls` is a non-empty array where `tool_calls[0].function.name` equals `"todo_list"` and `tool_calls[0].function.arguments` is a valid JSON string.

**Acceptance Scenarios**:

1. **Given** a chat completion request with one or more tool definitions and a user message that clearly requires a tool call, **When** inference completes, **Then** the response has `finish_reason: "tool_calls"`, `message.tool_calls` is a non-empty array, each entry has `id`, `type: "function"`, and `function.name` matching a defined tool, and `function.arguments` is a parseable JSON string.
2. **Given** a chat completion request with tool definitions and a user message that does not require any tool (e.g. "what is 2+2?"), **When** inference completes, **Then** the response has `finish_reason: "stop"`, `message.content` contains the answer, and `message.tool_calls` is absent or empty.
3. **Given** a chat completion request with no `tools` field, **When** inference completes, **Then** the response is identical to current behavior — no change in shape or content.
4. **Given** a multi-turn conversation where a prior assistant turn contains `tool_calls` and a subsequent `role: "tool"` turn carries the result, **When** inference runs the next turn, **Then** the model receives the full context including the tool result and responds accordingly (either with another tool call or a final answer).

---

### User Story 2 — Tool Arguments Are Well-Formed and Match Declared Schema (Priority: P2)

When the model calls a tool, the arguments it produces must match the parameter schema declared in the tool definition. Callers should not need to defensively validate or clean up arguments before passing them to the tool implementation.

**Why this priority**: Malformed tool arguments cause silent failures downstream — tools receive unexpected types or missing required fields and either crash or return wrong results. This is P2 rather than P1 because P1 (returning a tool_calls response at all) is prerequisite.

**Independent Test**: Send a request with a tool whose schema requires a string `title` and an optional ISO 8601 `due_at`. Ask the model to create a todo. Verify `function.arguments` is valid JSON with a `title` field that is a string, and `due_at` (if present) is a string in ISO 8601 format.

**Acceptance Scenarios**:

1. **Given** a tool definition with required and optional parameters and a user message that provides values for all required parameters, **When** the model calls the tool, **Then** `function.arguments` contains all required parameters with the correct types.
2. **Given** a tool definition where a parameter is optional, **When** the model calls the tool without a value for that parameter, **Then** `function.arguments` omits the optional parameter rather than passing null or an invalid value.

---

### Edge Cases

- What if the model produces a partial or malformed `<tool_call>` XML block? → Return a 500 with a parse error; do not attempt to forward a half-formed tool call.
- What if the model names a tool that was not in the `tools` array? → Include it in `tool_calls` as-is; callers are responsible for handling unknown tool names.
- What if the `tools` array is present but empty? → Treat as if `tools` was absent — no tool-calling behavior, standard plain-text response.
- What if a tool parameter schema uses `$ref` or complex JSON Schema features? → The server passes tool definitions to the model as-is; schema validation of tool arguments is out of scope.
- What if the model calls multiple tools in a single response? → Return all calls in the `tool_calls` array; callers may execute them in any order.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The server MUST accept a `tools` array in POST `/v1/chat/completions` requests, where each entry follows the OpenAI function tool format (`type: "function"`, `function.name`, `function.description`, `function.parameters`).
- **FR-002**: When `tools` is provided, the server MUST render the model's native GGUF chat template with the tool definitions included, such that the model receives tool schemas in the format its training expects.
- **FR-003**: When the model output contains one or more tool call markers, the server MUST parse them into a structured `tool_calls` array and return `finish_reason: "tool_calls"`.
- **FR-004**: Each entry in the returned `tool_calls` array MUST contain a unique `id` string, `type: "function"`, and a `function` object with `name` (string) and `arguments` (JSON-encoded string).
- **FR-005**: The server MUST accept `role: "tool"` messages in the `messages` array (carrying `tool_call_id` and `content`), passing them through the chat template so the model can continue the conversation after tool results are provided.
- **FR-006**: When no tool call markers are present in the model output, the server MUST return a plain content response with `finish_reason: "stop"` — identical to current behavior.
- **FR-007**: When `tools` is absent or empty, the server MUST behave exactly as it does today — no change to prompt construction or response shape.
- **FR-008**: The server MUST return a 500 error if a tool call marker is detected in model output but cannot be parsed into a valid tool call structure.

### Key Entities

- **ToolDefinition**: A function tool as declared by the caller — `type: "function"`, `function.name`, `function.description`, `function.parameters` (JSON Schema object).
- **ToolCall**: A structured tool invocation returned by the model — `id` (unique string), `type: "function"`, `function.name`, `function.arguments` (JSON-encoded string).
- **ToolResult**: A caller-supplied tool execution result — `role: "tool"`, `tool_call_id` matching a prior `ToolCall.id`, `content` (string result).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of chat completion requests with a `tools` array and a question clearly requiring a tool result in a `tool_calls` response — zero plain-text fallbacks — across a 20-request test run with the Qwen2.5-7B model.
- **SC-002**: 100% of returned `tool_calls` entries have `function.arguments` that parse as valid JSON — zero malformed argument strings.
- **SC-003**: Chat completion requests without a `tools` field produce responses identical in shape and content to pre-feature behavior — zero regressions across a full regression run.
- **SC-004**: The tool-calling path adds no more than 5% to median inference latency compared to equivalent tool-free requests (template rendering is CPU-bound and negligible relative to GPU inference time).
- **SC-005**: Multi-turn conversations with interleaved `tool_calls` and `role: "tool"` result turns complete correctly end-to-end — the model produces a coherent final answer after receiving tool results.

## Assumptions

- The GGUF model file (`Qwen2.5-7B-Instruct-Q8_0.gguf`) contains a valid Jinja2 chat template in its metadata that has a native `{% if tools %}` branch compatible with the OpenAI function-calling format.
- The model has been instruction-tuned to produce tool calls using `<tool_call>...</tool_call>` XML markers when tools are present in the prompt — no additional fine-tuning is required.
- The existing llama-cpp-2 Rust binding version is not upgraded; template rendering is handled in Rust without modifying the C++ library dependency.
- Tool argument schema validation (ensuring model output conforms to the declared parameter schema) is out of scope — callers validate arguments themselves.
- Streaming responses (`stream: true`) with tool calling are out of scope for this feature; only non-streaming responses are supported.
- The `tool_choice` field (forcing or preventing specific tool selection) is accepted but not enforced — model selection is driven entirely by the prompt and model behavior.
- Server-side tool execution is out of scope; the server returns tool call descriptions and callers execute them.
