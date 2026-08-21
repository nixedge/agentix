# Quickstart: Testing Tool Calling

## Prerequisites

- `agentix-daemon` running with the Qwen2.5-7B-Instruct GGUF model loaded
- `agentix-agent daemon` running on port 8765 (for end-to-end agent test)

## Test 1: Basic Tool Call

Send a request that should trigger a tool call:

```bash
curl -s -X POST http://ai.lan.disasm.us:11434/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Qwen2.5-7B-Instruct-Q8_0.gguf",
    "messages": [{"role": "user", "content": "What is on my todo list?"}],
    "tools": [{
      "type": "function",
      "function": {
        "name": "todo_list",
        "description": "List todos.",
        "parameters": {"type": "object", "properties": {}}
      }
    }]
  }' | python3 -m json.tool
```

**Expected**: `finish_reason: "tool_calls"`, `message.tool_calls[0].function.name == "todo_list"`, `message.content == null`.

## Test 2: No Tool Call When Not Needed

```bash
curl -s -X POST http://ai.lan.disasm.us:11434/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Qwen2.5-7B-Instruct-Q8_0.gguf",
    "messages": [{"role": "user", "content": "What is 2 + 2?"}],
    "tools": [{
      "type": "function",
      "function": {"name": "todo_list", "description": "List todos.", "parameters": {"type": "object", "properties": {}}}
    }]
  }' | python3 -m json.tool
```

**Expected**: `finish_reason: "stop"`, `message.content` is a string (the answer), no `tool_calls` field.

## Test 3: No Tools — Regression Check

```bash
curl -s -X POST http://ai.lan.disasm.us:11434/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Qwen2.5-7B-Instruct-Q8_0.gguf",
    "messages": [{"role": "user", "content": "Say hello."}]
  }' | python3 -m json.tool
```

**Expected**: same shape as before this feature — `finish_reason: "stop"`, `message.content` is a greeting string.

## Test 4: Multi-Turn with Tool Result

Run Test 1, copy the `tool_calls[0].id` from the response, then:

```bash
curl -s -X POST http://ai.lan.disasm.us:11434/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Qwen2.5-7B-Instruct-Q8_0.gguf",
    "messages": [
      {"role": "user", "content": "What is on my todo list?"},
      {"role": "assistant", "content": null, "tool_calls": [{"id": "PASTE_ID_HERE", "type": "function", "function": {"name": "todo_list", "arguments": "{}"}}]},
      {"role": "tool", "tool_call_id": "PASTE_ID_HERE", "content": "[{\"title\": \"Review PR\", \"due_at\": \"2026-08-21T17:00:00Z\"}]"}
    ],
    "tools": [{"type": "function", "function": {"name": "todo_list", "description": "List todos.", "parameters": {"type": "object", "properties": {}}}}]
  }' | python3 -m json.tool
```

**Expected**: `finish_reason: "stop"`, `message.content` references the todo item from the tool result.

## Test 5: End-to-End via agentix-agent

```bash
curl -s -X POST http://127.0.0.1:8765/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "What do I have on my todo list?"}' | python3 -m json.tool
```

**Expected**: `reply` contains the actual todos from the sled store, not hallucinated items.
