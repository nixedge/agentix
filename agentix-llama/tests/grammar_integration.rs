//! Integration tests for grammar enforcement (T020).
//!
//! Requires a completion-capable GGUF model:
//!   AGENTIX_TEST_COMPLETION_MODEL_PATH — path to a small GGUF text-generation model
//!
//! Tests are skipped (not failed) when the env var is unset.
//!
//! These tests require a model that supports the chat completion capability.
//! Use a small quantized model (< 50 MB) such as TinyLlama or smol-lm.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use agentix_infer::{CompletionMessage, CompletionRequest, GrammarConstraint, InferConfig, InferEngine};
use agentix_llama::LlamaCppBackend;
use std::sync::Arc;
use tokio_stream::StreamExt;

const JSON_GBNF: &str = r#"root   ::= object
value  ::= object | array | string | number | ("true" | "false" | "null")
object ::= "{" (string ":" value ("," string ":" value)*)? "}"
array  ::= "[" (value ("," value)*)? "]"
string ::= "\"" ([^"\\] | "\\" (["\\/bfnrt] | "u" [0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]))* "\""
number ::= ("-"? ([0-9] | [1-9] [0-9]*)) ("." [0-9]+)? ([eE] [-+]? [0-9]+)?"#;

async fn make_engine(model_path: &str) -> (InferEngine, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = InferConfig::new(dir.path().to_path_buf(), None, 1, 512);
    let engine = InferEngine::new(config).await.expect("engine init");
    let backend = LlamaCppBackend::new().expect("LlamaCppBackend::new");
    engine.register_backend(Arc::new(backend));
    let info = engine.pull(model_path).await.expect("pull failed");
    let name = info.name.clone();
    (engine, name)
}

async fn collect_output(engine: &InferEngine, model: &str, req: CompletionRequest) -> String {
    let stream = engine
        .complete(model, req)
        .await
        .expect("complete returned Err");
    let mut output = String::new();
    let mut stream = stream;
    while let Some(chunk) = stream.next().await {
        if let Ok(c) = chunk {
            output.push_str(&c.delta);
        }
    }
    output
}

/// Verify that grammar enforcement produces valid JSON output.
#[ignore = "requires a completion-capable GGUF model via AGENTIX_TEST_COMPLETION_MODEL_PATH"]
#[tokio::test]
async fn json_object_produces_valid_json() {
    let model_path = match std::env::var("AGENTIX_TEST_COMPLETION_MODEL_PATH") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("AGENTIX_TEST_COMPLETION_MODEL_PATH not set — skipping");
            return;
        }
    };

    let (engine, model) = make_engine(&model_path).await;

    let req = CompletionRequest {
        messages: vec![CompletionMessage::new(
            "user",
            "Reply with a JSON object with a 'name' field set to 'test'.",
        )],
        max_tokens: Some(64),
        temperature: Some(0.0),
        top_p: None,
        stop: vec![],
        grammar: Some(GrammarConstraint::Gbnf(JSON_GBNF.to_string())),
    };

    let output = collect_output(&engine, &model, req).await;
    eprintln!("grammar output: {output:?}");

    serde_json::from_str::<serde_json::Value>(&output)
        .expect("grammar-constrained output must be valid JSON");
}

/// Regression guard: unconstrained output still completes without error.
#[ignore = "requires a completion-capable GGUF model via AGENTIX_TEST_COMPLETION_MODEL_PATH"]
#[tokio::test]
async fn no_grammar_output_unchanged() {
    let model_path = match std::env::var("AGENTIX_TEST_COMPLETION_MODEL_PATH") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("AGENTIX_TEST_COMPLETION_MODEL_PATH not set — skipping");
            return;
        }
    };

    let (engine, model) = make_engine(&model_path).await;

    let req = CompletionRequest {
        messages: vec![CompletionMessage::new("user", "Say hello.")],
        max_tokens: Some(32),
        temperature: Some(0.0),
        top_p: None,
        stop: vec![],
        grammar: None,
    };

    let output = collect_output(&engine, &model, req).await;
    assert!(
        !output.is_empty(),
        "unconstrained completion should produce non-empty output"
    );
    eprintln!("no-grammar output: {output:?}");
}
