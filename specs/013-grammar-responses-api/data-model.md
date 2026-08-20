# Data Model: Grammar Enforcement and Responses API

**Feature**: 013-grammar-responses-api  
**Date**: 2026-08-20

---

## Entity: ResponseFormat (agentix-api)

Represents the output format constraint on a chat completion request.

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub format_type: ResponseFormatType,
    pub json_schema: Option<JsonSchemaSpec>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormatType {
    JsonObject,
    JsonSchema,
    Text,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JsonSchemaSpec {
    pub name: Option<String>,
    pub schema: serde_json::Value,
    pub strict: Option<bool>,
}
```

**Validation rules**:
- When `format_type == JsonSchema`, `json_schema` MUST be present
- When `json_schema.schema` is present, it MUST not contain `$ref` pointing to external URIs (http/https)
- `format_type == Text` and `Unknown` → no grammar enforcement

**Added to**: `ChatCompletionRequest.response_format: Option<ResponseFormat>`

---

## Entity: GrammarConstraint (agentix-infer)

Internal representation of a grammar constraint passed to the inference backend. Opaque to agentix-infer; only agentix-llama interprets the GBNF string.

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum GrammarConstraint {
    /// A raw GBNF grammar string. Root rule is always named "root".
    Gbnf(String),
}
```

**Added to**: `CompletionRequest.grammar: Option<GrammarConstraint>`

**State transitions**:
- `response_format.type == "json_object"` → `GrammarConstraint::Gbnf(JSON_GBNF_CONST)`
- `response_format.type == "json_schema"` → `GrammarConstraint::Gbnf(json_schema_to_grammar(schema))`
- No `response_format` or `type == "text"` → `None`

---

## Entity: ResponsesRequest (agentix-api)

The OpenAI Responses API request body (`POST /v1/responses`).

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ResponsesRequest {
    pub model: String,
    pub input: Vec<ResponseInputItem>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    pub text: Option<ResponseTextConfig>,
    /// Silently ignored (no server-side storage)
    #[serde(default)]
    pub store: Option<bool>,
    /// Silently ignored (no special reasoning mode)
    #[serde(default)]
    pub reasoning: Option<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ResponseInputItem {
    pub role: String,
    pub content: ResponseInputContent,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
pub enum ResponseInputContent {
    /// Plain string shorthand
    Text(String),
    /// Array of content items
    Parts(Vec<ResponseInputPart>),
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ResponseInputPart {
    #[serde(rename = "type")]
    pub part_type: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ResponseTextConfig {
    pub format: Option<ResponseTextFormat>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ResponseTextFormat {
    #[serde(rename = "type")]
    pub format_type: String,
    pub schema: Option<serde_json::Value>,
    pub name: Option<String>,
}
```

**Validation rules**:
- `model` MUST be a non-empty string
- `input` MUST be a non-empty array
- `developer` role in `input` items is mapped to `system` for inference
- `text.format.type == "json_schema"` without `schema` → treat as no grammar constraint
- Content string shorthand and content-array form are both valid

---

## Entity: ResponsesResponse (agentix-api)

The OpenAI Responses API response body.

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResponsesResponse {
    pub id: String,
    pub object: String,           // always "response"
    pub model: String,
    pub output: Vec<ResponseOutputMessage>,
    pub output_text: String,      // convenience: output[0].content[0].text
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ResponseOutputMessage {
    #[serde(rename = "type")]
    pub msg_type: String,          // "message"
    pub id: String,                // "msg_<uuid>"
    pub role: String,              // "assistant"
    pub status: String,            // "completed"
    pub content: Vec<ResponseOutputContent>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseOutputContent {
    OutputText { text: String },
    Refusal { refusal: String },
}
```

**State transitions**:
- Normal completion → `ResponseOutputContent::OutputText { text: <generated> }`
- Model refusal → `ResponseOutputContent::Refusal { refusal: <refusal text> }`
- `output_text` = text from first `OutputText` content item, or empty string on refusal

---

## Relationships

```
ChatCompletionRequest
  └── response_format: Option<ResponseFormat>
        └── json_schema: Option<JsonSchemaSpec>
              └── schema: serde_json::Value  ──converts to──►  GrammarConstraint::Gbnf(String)

CompletionRequest  (agentix-infer)
  └── grammar: Option<GrammarConstraint>
        └── GrammarConstraint::Gbnf(String)  ──applied by──►  LlamaSampler::grammar(model, str, "root")

ResponsesRequest  ──translated to──►  CompletionRequest
  └── input[*] { role, content }
  └── text.format.schema                ──converted to──►  GrammarConstraint
  └── max_output_tokens                 ──mapped to──►  CompletionRequest.max_tokens

CompletionResult  ──wrapped into──►  ResponsesResponse
  └── output[0].content[0].text = output_text
```
