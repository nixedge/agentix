# Research: Tool Calling for agentix-llama

**Feature**: 015-llama-tool-calling  
**Date**: 2026-08-21

## Decision 1: Template Rendering Approach

**Decision**: Use `minijinja` to render the GGUF's Jinja2 chat template directly, passing `{messages, tools, add_generation_prompt: true}` as context variables.

**Rationale**: The raw Jinja2 template string is accessible via `LlamaChatTemplate::to_str()` (confirmed in llama-cpp-2 0.1.154 at `/nix/store/.../src/model.rs:63`). minijinja is a pure-Rust Jinja2 implementation with no system dependencies, negligible runtime overhead, and supports all the Jinja2 features Qwen2.5's template uses (`tojson` filter, `namespace()`, `for`/`if`, `loop.last`).

**Alternatives considered**:
- Upgrade to `llama_chat_apply_template_ex` (C API with tools param): requires a llama-cpp-2 version bump that is not yet available for 0.1.x; rejected.
- Inject tools as a JSON string into the system message manually: bypasses the GGUF's native template and breaks for models with different tool injection conventions; rejected.
- Keep the existing `model.apply_chat_template()` binding: has no tools parameter; rejected.

## Decision 2: Template String Source

**Decision**: Call `model.chat_template(None)` (already done today) and then `.to_str()` on the returned `LlamaChatTemplate` to get the raw Jinja2 string for minijinja to render.

**Rationale**: `LlamaChatTemplate` exposes `to_str() -> Result<&str, Utf8Error>` (confirmed in binding source). No GGUF metadata parsing needed. The same code path that already fetches the template is reused.

## Decision 3: Tool Call Output Parsing

**Decision**: Post-process the full model output string to extract `<tool_call>...</tool_call>` XML-like markers. Regex: `<tool_call>\s*([\s\S]*?)\s*</tool_call>`. Parse the captured body as JSON. If any matches are found, the response uses `finish_reason: "tool_calls"` with a structured `tool_calls` array.

**Rationale**: Qwen2.5-Instruct generates tool calls as plain text markers in its output — not as special tokens. The markers are unambiguous and reliably parseable. This approach works regardless of streaming vs. non-streaming (we post-process the accumulated buffer).

**Alternatives considered**:
- Special-token-based detection (`<|tool_calls|>` etc.): Qwen2.5 does not use this format; rejected.
- Stream-time parsing (detect markers mid-generation and stop): adds complexity to the token loop; defer to a future streaming feature; rejected for MVP.

## Decision 4: Message Schema for Tool Turns

**Decision**: Extend `CompletionMessage` with two optional fields: `tool_calls: Option<serde_json::Value>` (for assistant turns that invoke tools) and `tool_call_id: Option<String>` (for `role: "tool"` result turns). minijinja receives messages as `serde_json::Value` objects constructed from these fields.

**Rationale**: The OpenAI protocol sends tool result turns as `{"role": "tool", "tool_call_id": "...", "content": "..."}` and prior assistant tool-call turns as `{"role": "assistant", "content": null, "tool_calls": [...]}`. These need to pass through the stack to reach the template renderer. Extending `CompletionMessage` keeps the change minimal and avoids a separate message type.

**Alternatives considered**:
- Pass `Vec<ChatMessage>` directly (skipping `CompletionMessage`): breaks the infer trait abstraction; rejected.
- Carry raw `serde_json::Value` as the full message object: loses the typed `role` field used elsewhere; rejected.

## Decision 5: minijinja Version and Features

**Decision**: `minijinja = { version = "2", features = ["json"] }`. The `json` feature enables the `tojson` Jinja2 filter, which Qwen2.5's template uses to serialize tool definitions.

**Rationale**: minijinja 2.x is the current stable series. The `json` feature is the only non-default feature required. All other Jinja2 constructs used by Qwen2.5 (`namespace`, `for`, `if`, `loop.last`, string concatenation) are supported in the default feature set.

## Qwen2.5 Tool Call Format (Confirmed)

Template injects tool schemas into the system preamble when `tools` is non-empty. Assistant tool-call output format:

```
<tool_call>
{"name": "todo_list", "arguments": {}}
</tool_call>
```

Multi-tool calls appear as consecutive `<tool_call>...</tool_call>` blocks before the EOG token. Tool result turns use `role: "tool"` with `tool_call_id` matching the call's `id`.
