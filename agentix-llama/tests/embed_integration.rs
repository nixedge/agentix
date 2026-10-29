//! Integration test for InferEngine embedding path (T034).
//!
//! Requires:
//!   AGENTIX_TEST_MODEL_PATH — path to a small GGUF embedding model (e.g. all-MiniLM-L6-v2 Q8_0)
//!
//! The test is skipped (not failed) when the env var is unset.
//! In CI the Nix flake sets AGENTIX_TEST_MODEL_PATH via the test derivation (T035).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use agentix_infer::{InferConfig, InferEngine};
use agentix_llama::LlamaCppBackend;
use std::sync::Arc;

#[tokio::test]
async fn embed_with_local_model() {
    let model_path = match std::env::var("AGENTIX_TEST_MODEL_PATH") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("AGENTIX_TEST_MODEL_PATH not set — skipping embed integration test");
            return;
        }
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let config = InferConfig::new(dir.path().to_path_buf(), None, 1, 512);

    let engine = InferEngine::new(config).await.expect("engine init");

    // Register the LlamaCpp backend
    let backend = LlamaCppBackend::new().expect("LlamaCppBackend::new");
    engine.register_backend(Arc::new(backend));

    // Pull the fixture model from its local path
    let info = engine.pull(&model_path).await.expect("pull failed");
    assert!(!info.name.is_empty(), "model name should not be empty");
    assert!(
        info.capabilities
            .contains(&agentix_infer::Capability::Embedding),
        "fixture model should report Embedding capability; got: {:?}",
        info.capabilities
    );

    // Generate an embedding
    let vec = engine
        .embed(&info.name, "fn tokenize(s: &str) -> Vec<String>")
        .await
        .expect("embed failed");

    assert!(!vec.is_empty(), "embedding vector should not be empty");

    // all-MiniLM-L6-v2 produces 384-dim embeddings
    // Be tolerant: just assert it's a reasonable dimension
    assert!(vec.len() >= 64, "embedding too short: {} dims", vec.len());
    assert!(
        vec.iter().all(|&v| v.is_finite()),
        "embedding contains NaN or Inf"
    );

    // Test batch embedding
    let inputs = ["hello world", "fn main() {}", "struct Config {}"];
    let batch = engine
        .embed_batch(&info.name, &inputs)
        .await
        .expect("embed_batch failed");

    assert_eq!(batch.len(), inputs.len());
    for (i, emb) in batch.iter().enumerate() {
        assert!(!emb.is_empty(), "batch[{i}] is empty");
        assert!(
            emb.iter().all(|&v| v.is_finite()),
            "batch[{i}] contains NaN or Inf"
        );
    }

    // Oversized inputs used to abort the entire process inside llama.cpp.
    let oversized = "hello ".repeat(9000);
    let error = engine
        .embed(&info.name, &oversized)
        .await
        .expect_err("oversized input must fail before native inference");
    assert!(matches!(
        error,
        agentix_infer::InferError::ContextExceeded { .. }
    ));
    let recovered = engine
        .embed(&info.name, "hello after rejected input")
        .await
        .expect("model must remain usable after rejected input");
    assert_eq!(recovered.len(), vec.len());
    assert!(recovered.iter().all(|value| value.is_finite()));

    // Verify model appears in list
    let models = engine.list().await;
    assert!(
        models.iter().any(|m| m.name.contains(&info.name)),
        "model not found in list"
    );

    // Clean up
    engine.remove(&info.name).await.expect("remove failed");
    assert!(
        engine.info(&info.name).is_none(),
        "model still present after remove"
    );

    eprintln!(
        "embed integration test passed: {} dims, {} batch items",
        vec.len(),
        inputs.len()
    );
}
