# Contract: POST /v1/responses

**Crate**: `agentix-llama` (handler), `agentix-daemon` (forwarding)  
**Handler**: `responses_handler` in `agentix-llama/src/main.rs`  
**Daemon route**: forwarded to llama Unix socket via `proxy::forward()`

---

## Request

```json
POST /v1/responses
Content-Type: application/json

{
  "model": "string (required)",
  "input": [
    {
      "role": "developer | user | assistant",
      "content": "string OR [{ \"type\": \"input_text\", \"text\": \"...\" }]"
    }
  ],
  "max_output_tokens": 1024,
  "text": {
    "format": {
      "type": "json_schema",
      "schema": { ... },
      "name": "optional label"
    }
  },
  "store": false,
  "reasoning": { "effort": "low" }
}
```

**Role mapping**: `developer` → `system`. `user` and `assistant` pass through unchanged.

**Content normalization**: Both string and array forms are accepted. Array is normalized to the concatenated text of all `input_text` parts.

**Ignored fields**: `store` and `reasoning` are accepted without error and have no effect.

**Grammar enforcement**: Applied when `text.format.type == "json_schema"`. Same validation and error behavior as `/v1/chat/completions`.

## Response (success)

```json
HTTP 200 OK
Content-Type: application/json

{
  "id": "resp_<uuid>",
  "object": "response",
  "model": "string (echoed from request)",
  "output": [
    {
      "type": "message",
      "id": "msg_<uuid>",
      "role": "assistant",
      "status": "completed",
      "content": [
        {
          "type": "output_text",
          "text": "... model output ..."
        }
      ]
    }
  ],
  "output_text": "... same as output[0].content[0].text ..."
}
```

## Response (model refusal)

When the model produces a refusal:

```json
{
  "id": "resp_<uuid>",
  "object": "response",
  "model": "string",
  "output": [
    {
      "type": "message",
      "id": "msg_<uuid>",
      "role": "assistant",
      "status": "completed",
      "content": [
        {
          "type": "refusal",
          "refusal": "I can't help with that."
        }
      ]
    }
  ],
  "output_text": ""
}
```

## Response (invalid schema)

```json
HTTP 400 Bad Request
Content-Type: application/json

{
  "error": "text.format.schema could not be converted to a grammar: <reason>"
}
```

## Response (model not found)

```json
HTTP 404 Not Found
Content-Type: application/json

{
  "error": "model '<name>' not found — pull it first"
}
```

---

## OpenAI SDK Compatibility

The OpenAI Node.js SDK's `client.responses.create(...)` must work against this endpoint without modification. The SDK sends `POST /v1/responses` and expects the response shape above. The `output_text` convenience accessor on the response object maps to `response.output_text`.
