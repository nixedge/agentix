# Quickstart: Grammar Enforcement and Responses API

**Feature**: 013-grammar-responses-api  
**Date**: 2026-08-20

---

## Scenario 1: Generic JSON Output (json_object)

**Goal**: Guarantee the model returns valid JSON without a specific schema.

```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "my-model",
    "messages": [
      { "role": "user", "content": "Give me a JSON object with name and age fields." }
    ],
    "response_format": { "type": "json_object" }
  }'
```

**Expected**: The `choices[0].message.content` field parses as a JSON object with no additional text.

---

## Scenario 2: Schema-Constrained JSON Output (json_schema)

**Goal**: Guarantee the model returns JSON matching a specific schema.

```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "my-model",
    "messages": [
      { "role": "user", "content": "What is the weather in London?" }
    ],
    "response_format": {
      "type": "json_schema",
      "json_schema": {
        "schema": {
          "type": "object",
          "properties": {
            "city": { "type": "string" },
            "temperature_c": { "type": "number" },
            "condition": { "type": "string" }
          },
          "required": ["city", "temperature_c", "condition"]
        }
      }
    }
  }'
```

**Expected**: The `choices[0].message.content` is valid JSON with `city`, `temperature_c`, and `condition` keys.

---

## Scenario 3: Invalid Schema (400 Error)

**Goal**: Confirm invalid schemas are rejected before inference.

```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "my-model",
    "messages": [{ "role": "user", "content": "test" }],
    "response_format": {
      "type": "json_schema",
      "json_schema": {
        "schema": { "$ref": "https://example.com/schema.json" }
      }
    }
  }'
```

**Expected**: HTTP 400, error message identifying the schema problem, no inference performed.

---

## Scenario 4: Responses API — Developer Role + Schema

**Goal**: Use the OpenAI Node.js SDK's `client.responses.create(...)` method.

```typescript
import OpenAI from "openai";

const client = new OpenAI({ baseURL: "http://localhost:8080/v1" });

const resp = await client.responses.create({
  model: "my-model",
  input: [
    {
      role: "developer",
      content: [{ type: "input_text", text: "Summarize this: ..." }],
    },
  ],
  text: {
    format: {
      type: "json_schema",
      schema: {
        type: "object",
        properties: {
          summary: { type: "string" },
          keywords: { type: "array", items: { type: "string" } },
        },
        required: ["summary", "keywords"],
      },
    },
  },
  max_output_tokens: 256,
});

console.log(resp.output_text);         // valid JSON string
const data = JSON.parse(resp.output_text);
console.log(data.summary, data.keywords);
```

**Expected**: `resp.output_text` is valid JSON with `summary` (string) and `keywords` (array).

---

## Scenario 5: Responses API — Plain String Input

**Goal**: Confirm both content formats are accepted.

```bash
curl -X POST http://localhost:8080/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "my-model",
    "input": [
      { "role": "user", "content": "Hello!" }
    ]
  }'
```

**Expected**: HTTP 200, `output[0].content[0].type == "output_text"`, `output_text` is non-empty.

---

## Scenario 6: No response_format (Regression Check)

**Goal**: Confirm existing behavior is unchanged.

```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "my-model",
    "messages": [{ "role": "user", "content": "Tell me a joke." }]
  }'
```

**Expected**: Works identically to pre-feature behavior. No grammar enforcement, no format requirement.

---

## Integration Test Approach

For automated testing against the running server, use a small quantized fixture model (< 50MB) pinned in the Nix flake. Tests should:

1. Load the fixture model via `InferEngine`
2. Send requests with `response_format` variants
3. Parse responses and assert JSON validity / schema conformance
4. Assert 400 for bad schema inputs
5. Assert the pre-feature path still works (no regression)
