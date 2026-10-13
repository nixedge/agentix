use agentix_qwen::tokenizer::QwenTokenizer;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{error::Error, io, path::PathBuf};

const FIXTURE: &str =
    include_str!("../../specs/021-strata-inference-research/fixtures/benchmark_prompts.json");

fn env_path(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("missing {name}")).into())
}

fn token_ids_sha256(ids: &[u32]) -> String {
    let mut hasher = Sha256::new();
    for token in ids {
        hasher.update(token.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[test]
fn pinned_benchmark_prompts_match_tokenizer_and_hashes() -> Result<(), Box<dyn Error>> {
    let tokenizer = QwenTokenizer::from_files(
        &env_path("AGENTIX_QWEN_TOKENIZER_PATH")?,
        &env_path("AGENTIX_QWEN_TEMPLATE_PATH")?,
    )?;
    let artifact: Value = serde_json::from_str(FIXTURE)?;
    assert_eq!(
        artifact["tokenizer_token_count"].as_u64(),
        Some(u64::try_from(tokenizer.tokenizer_token_count())?)
    );
    assert_eq!(
        artifact["model_vocabulary_size"].as_u64(),
        Some(u64::try_from(tokenizer.model_vocabulary_size())?)
    );
    assert_eq!(
        artifact["stop_token_ids"],
        json!(tokenizer.stop_token_ids())
    );

    let prompts = artifact["prompts"]
        .as_array()
        .ok_or("benchmark prompt artifact has no prompt array")?;
    assert_eq!(prompts.len(), 3);
    for prompt in prompts {
        let content = prompt["message_content"]
            .as_str()
            .ok_or("benchmark prompt has no message content")?;
        let messages = [json!({"role":"user", "content":content})];
        let rendered = tokenizer.render(&messages, &[], "medium", true, true, true)?;
        let expected_rendered = prompt["rendered_prompt"]
            .as_str()
            .ok_or("benchmark prompt has no rendered prompt")?;
        assert_eq!(rendered, expected_rendered);
        let actual_ids = tokenizer.encode_rendered(&rendered)?.get_ids().to_vec();
        let expected_ids: Vec<u32> = serde_json::from_value(prompt["token_ids"].clone())?;
        let expected_count = prompt["target_tokens"]
            .as_u64()
            .ok_or("benchmark prompt has no target token count")?;
        assert_eq!(u64::try_from(actual_ids.len())?, expected_count);
        assert_eq!(actual_ids, expected_ids);
        assert_eq!(
            token_ids_sha256(&actual_ids),
            prompt["token_ids_little_endian_sha256"]
                .as_str()
                .ok_or("benchmark prompt has no token-ID hash")?
        );
    }
    Ok(())
}
