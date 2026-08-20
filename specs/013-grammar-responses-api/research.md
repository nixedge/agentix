# Research: Grammar Enforcement and Responses API

**Feature**: 013-grammar-responses-api  
**Date**: 2026-08-20

---

## Finding 1: llama-cpp-2 Grammar Sampler API

**Decision**: Use `LlamaSampler::grammar()` from `llama-cpp-2` with the `common` feature enabled.

**Rationale**: The `llama-cpp-2` 0.1.154 crate (already in the dependency tree) exposes grammar sampling in its safe Rust API when the `common` feature is enabled. No new crate is needed.

**API surface**:
```rust
// Convert JSON schema string → GBNF grammar string
llama_cpp_2::json_schema_to_grammar(schema_json: &str) -> Result<String>

// Build a grammar sampler from a GBNF string
LlamaSampler::grammar(model: &LlamaModel, grammar_str: &str, grammar_root: &str)
    -> Result<LlamaSampler, GrammarError>
```

Both functions require `features = ["common"]` in the dependency declaration.

**Alternatives considered**:
- Raw `llama-cpp-sys-2` FFI — rejected because the safe wrapper now covers the needed surface
- External grammar crate — rejected because llama.cpp's bundled converter handles the schema format we need

---

## Finding 2: Generic JSON Grammar

**Decision**: Use the GBNF string bundled in `llama-cpp-2` (available as `src/grammar/json.gbnf`) by hardcoding it as a constant in `agentix-llama`. The grammar is stable and short.

**GBNF content** (the full `json.gbnf` from llama-cpp-2 0.1.154):
```gbnf
root   ::= object
value  ::= object | array | string | number | ("true" | "false" | "null")
object ::=
  "{" (
            string ":" value
    ("," string ":" value)*
  )? "}"
array  ::=
  "[" (
            value
    ("," value)*
  )? "]"
string ::=
  "\"" (
    [^"\\] |
    "\\" (["\\/bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F])
  )* "\""
number ::= ("-"? ([0-9] | [1-9] [0-9]*)) ("." [0-9]+)? ([eE] [-+]? [0-9]+)?
```

The root rule produces a JSON object (not array or scalar). This matches `json_object` semantics.

---

## Finding 3: Schema Validation and Error Handling

**Decision**: Validate the schema by calling `json_schema_to_grammar()` before inference starts. If it returns an error, return HTTP 400 immediately.

**Rationale**: The spec (FR-004) requires 400 errors for invalid schemas before inference begins. The `json_schema_to_grammar` FFI call is fast (< 5ms) and can be done synchronously in the request handler before dispatching to the inference thread.

**External `$ref` rejection**: The GBNF conversion function does not resolve external URIs. Schemas containing `$ref` pointing to external resources will produce a malformed grammar; we detect this by rejecting any schema whose serialized form contains `"$ref"` that starts with `http` or has a non-`#` fragment before calling the converter.

---

## Finding 4: Code Path for Grammar Enforcement

**Decision**: Grammar enforcement happens inside `agentix-llama`, not in `agentix-daemon` or `agentix-infer`.

**Rationale**: 
- `agentix-infer` defines the traits but has no `llama-cpp-2` dependency. Grammar constraint is passed as a `GrammarConstraint::Gbnf(String)` field on `CompletionRequest` — the GBNF string is opaque to `agentix-infer`.
- `agentix-llama/src/lib.rs` builds the sampler chain; it has `llama-cpp-2` and can call `LlamaSampler::grammar()`.
- The daemon proxies all local requests via Unix socket without inspecting the body — it does not need to know about grammar.
- GBNF string is computed in `agentix-llama/src/main.rs` (from `response_format`), stored in `CompletionRequest.grammar`, and applied in `agentix-llama/src/lib.rs`'s `complete_sync()`.

**Sampler chain with grammar** (new chain when grammar is present):
```rust
// Grammar sampler MUST come first in the chain — it masks invalid tokens before scoring
LlamaSampler::grammar(model, grammar_str, "root")?
// Followed by normal scoring/selection samplers
LlamaSampler::top_k(40)
LlamaSampler::top_p(top_p, 1)
LlamaSampler::temp(temperature)
LlamaSampler::dist(0xDEAD_BEEF)
```

---

## Finding 5: Responses API Endpoint Architecture

**Decision**: Implement `/v1/responses` directly in `agentix-llama/src/main.rs` as a new route handler. The daemon adds a forwarding rule for this path.

**Request translation**:
- `input` array → convert to `CompletionMessage` list, mapping `developer` role to `system`
- `text.format.type == "json_schema"` → extract `text.format.schema`, call `json_schema_to_grammar`, set `CompletionRequest.grammar`
- `max_output_tokens` → `CompletionRequest.max_tokens`
- `store`, `reasoning` → silently ignored

**Response construction**:
- Generate UUID for `id` field (using `uuid` crate, already in workspace)
- `object`: `"response"`
- `output`: one-element array with `type: "message"`, `role: "assistant"`, `content` array
- `output_text`: same string as `output[0].content[0].text`

**Streaming**: Not required by the spec; `/v1/responses` returns a complete JSON response (no SSE).

---

## Finding 6: `LlamaModel` Availability for Grammar Sampler

**Decision**: Pass `&model` from the inference thread's `LlamaCppLoadedModel` into `complete_sync()`. The model is already available in the inference thread that owns `LlamaCppLoadedModel`.

**Current signature** (`agentix-llama/src/lib.rs`): `complete_sync(ctx, model, req, tx)` — `model: &LlamaModel` is already a parameter.

---

## Finding 7: `agentix-api` Type Changes

**Decision**: Add `response_format: Option<ResponseFormat>` as an explicit struct field (not via `extra` map) to `ChatCompletionRequest`. This keeps the API contract crate authoritative for the request schema per Constitution Principle V.

**New types in `agentix-api`**:
- `ResponseFormat { type: ResponseFormatType, json_schema: Option<JsonSchemaSpec> }`
- `ResponseFormatType` (enum): `JsonObject`, `JsonSchema`, `Text`, `Unknown(String)`
- `JsonSchemaSpec { name: Option<String>, schema: serde_json::Value, strict: Option<bool> }`
- `ResponsesRequest` / `ResponsesResponse` (see data-model.md)

**Serde shape** to match OpenAI SDK wire format:
```json
{ "response_format": { "type": "json_schema", "json_schema": { "schema": {...} } } }
```

---

## Finding 8: Constitution Compliance

All changes comply with the constitution:

| Principle | Status | Notes |
|-----------|--------|-------|
| I (Library-First) | ✓ | Grammar logic in `agentix-llama` lib; types in `agentix-api`/`agentix-infer` |
| II (Local-First) | ✓ | Feature is purely about local model output format |
| III (Reproducible) | ✓ | Only adding `common` feature to existing dep; no new external deps |
| IV (Isolation) | n/a | No sandbox changes |
| V (Layered API) | ✓ | New types in `agentix-api`; daemon proxies without body inspection |
| VI (Testing) | ✓ | Unit tests for grammar module; integration tests use fixture model |
| VII (State Machine) | n/a | No agent loop changes |
| VIII (Quality Gates) | ✓ | Must pass fmt, clippy, tests, nix build |
