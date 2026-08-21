# Data Model: 015-llama-tool-calling

## Modified Types

### `agentix-api::ChatMessage` (agentix-api/src/lib.rs)

Extended with optional tool-protocol fields:

```
ChatMessage {
  role:         String                       // "system" | "user" | "assistant" | "tool"
  content:      serde_json::Value            // String or null (null for tool-call turns)
  name:         Option<String>               // unchanged
  tool_calls:   Option<serde_json::Value>    // NEW: array of tool call objects on assistant turns
  tool_call_id: Option<String>               // NEW: matching id on role="tool" turns
}
```

`tool_calls` structure (when present):
```json
[
  {
    "id": "call_abc123",
    "type": "function",
    "function": {
      "name": "todo_list",
      "arguments": "{}"
    }
  }
]
```

### `agentix-api::ChatCompletionRequest` (agentix-api/src/lib.rs)

New typed field (moved out of `extra`):

```
ChatCompletionRequest {
  ...existing fields...
  tools: Option<Vec<serde_json::Value>>   // NEW: array of tool definition objects
}
```

Tool definition structure:
```json
{
  "type": "function",
  "function": {
    "name": "todo_list",
    "description": "List todos.",
    "parameters": {
      "type": "object",
      "properties": {}
    }
  }
}
```

### `agentix-infer::CompletionMessage` (agentix-infer/src/lib.rs)

Extended to carry tool-protocol data through the stack:

```
CompletionMessage {
  role:         String
  content:      String
  tool_calls:   Option<serde_json::Value>   // NEW
  tool_call_id: Option<String>              // NEW
}
```

### `agentix-infer::CompletionRequest` (agentix-infer/src/lib.rs)

New field:

```
CompletionRequest {
  ...existing fields...
  tools: Option<Vec<serde_json::Value>>   // NEW: forwarded from ChatCompletionRequest
}
```

## New Types

### `ToolCallResult` (agentix-llama/src/main.rs, internal)

Parsed result of post-processing model output for tool call markers:

```
ToolCallResult {
  id:        String              // generated UUID
  name:      String              // tool name from JSON body
  arguments: String              // raw JSON arguments string
}
```

Used only internally to construct the HTTP response; not a public type.

## Response Shape Changes

When tool calls are detected in model output, the non-streaming chat completion response changes from:

```json
{
  "choices": [{
    "index": 0,
    "message": {"role": "assistant", "content": "..."},
    "finish_reason": "stop"
  }]
}
```

to:

```json
{
  "choices": [{
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
  }]
}
```

No changes to streaming response shape (streaming + tool calling is out of scope).
