# Contract: POST /v1/chat/completions (extended with response_format)

**Crate**: `agentix-llama`  
**Handler**: `chat_completions_handler` in `agentix-llama/src/main.rs`

---

## Request

```json
POST /v1/chat/completions
Content-Type: application/json

{
  "model": "string (required)",
  "messages": [
    { "role": "system|user|assistant", "content": "string or content-array" }
  ],
  "max_tokens": 512,
  "temperature": 0.7,
  "response_format": {
    "type": "json_object | json_schema | text",
    "json_schema": {
      "name": "optional label",
      "schema": { ... },
      "strict": true
    }
  }
}
```

**response_format rules**:
- `type: "json_object"` → enforce generic JSON grammar; `json_schema` field ignored
- `type: "json_schema"` → `json_schema.schema` REQUIRED; enforce schema-derived grammar
- `type: "text"` or absent → no grammar enforcement (unchanged behavior)
- `json_schema.schema` containing `$ref` to external URI → 400 before inference

## Response (success)

```json
HTTP 200 OK
Content-Type: application/json

{
  "choices": [{
    "finish_reason": "stop | length",
    "message": {
      "role": "assistant",
      "content": "... guaranteed-valid JSON when grammar enforced ..."
    }
  }]
}
```

## Response (invalid schema)

```json
HTTP 400 Bad Request
Content-Type: application/json

{
  "error": "response_format.json_schema.schema could not be converted to a grammar: <reason>"
}
```

**Guarantee**: When `response_format.type` is `json_object` or `json_schema`, the response `content` is valid JSON. When `json_schema` is provided, the content conforms to the schema. This is enforced at the token-sampling level — not post-processed.

**Unchanged contracts**: All requests without `response_format` behave identically to pre-feature behavior. Streaming (`"stream": true`) is not affected.
