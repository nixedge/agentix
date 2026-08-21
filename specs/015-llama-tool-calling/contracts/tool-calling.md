# Contract: Tool Calling Extension to POST /v1/chat/completions

## Endpoint

`POST /v1/chat/completions` (existing endpoint, extended)

## Request Changes

New optional field `tools` alongside existing fields:

```json
{
  "model": "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Qwen2.5-7B-Instruct-Q8_0.gguf",
  "messages": [
    {"role": "user", "content": "What's on my todo list?"}
  ],
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "todo_list",
        "description": "List todos. Optionally filter by completion status.",
        "parameters": {
          "type": "object",
          "properties": {
            "include_completed": {
              "type": "boolean",
              "description": "Whether to include completed todos (default false)"
            }
          }
        }
      }
    }
  ]
}
```

`tools` absent or `[]` → existing behavior unchanged.

## Multi-Turn Tool Call Conversation

Turn 1 (above) → server returns tool call response (see below)  
Turn 2 (client executes tool, sends result):

```json
{
  "model": "...",
  "messages": [
    {"role": "user", "content": "What's on my todo list?"},
    {
      "role": "assistant",
      "content": null,
      "tool_calls": [
        {
          "id": "call_a1b2c3",
          "type": "function",
          "function": {"name": "todo_list", "arguments": "{}"}
        }
      ]
    },
    {
      "role": "tool",
      "tool_call_id": "call_a1b2c3",
      "content": "[{\"id\": \"uuid1\", \"title\": \"Review PR\", \"due_at\": \"2026-08-21T17:00:00Z\"}]"
    }
  ],
  "tools": [...]
}
```

Turn 2 response is a plain content response (`finish_reason: "stop"`) with the model's answer.

## Response: Tool Call

When the model decides to call one or more tools:

```json
{
  "id": "chatcmpl-<uuid>",
  "object": "chat.completion",
  "model": "hf.co/bartowski/...",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": null,
        "tool_calls": [
          {
            "id": "call_<uuid>",
            "type": "function",
            "function": {
              "name": "todo_list",
              "arguments": "{}"
            }
          }
        ]
      },
      "finish_reason": "tool_calls"
    }
  ]
}
```

## Response: No Tool Call

When the model answers directly (question doesn't require a tool, or no tools provided):

```json
{
  "choices": [
    {
      "index": 0,
      "message": {"role": "assistant", "content": "The answer is 42."},
      "finish_reason": "stop"
    }
  ]
}
```

Identical to current behavior — no change.

## Error Responses

| Condition | HTTP Status | Body |
|-----------|-------------|------|
| Tool call marker found but JSON body unparseable | 500 | `{"error": "tool call parse error: <detail>"}` |
| All other errors | unchanged | unchanged |

## Constraints

- `tool_choice` field: accepted without error, ignored (model chooses freely).
- Streaming (`stream: true`) with `tools`: `tools` is ignored in streaming mode; plain streaming response returned.
- `tools` with `response_format`: both fields are accepted; `response_format` grammar enforcement is applied in addition to tool rendering. This combination is unusual but not rejected.
