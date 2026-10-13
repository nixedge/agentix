use agentix_qwen::tokenizer::QwenTokenizer;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{error::Error, fs, path::PathBuf};

const REPEATED_SEGMENT: &str =
    "Benchmark context record: preserve this complete instruction and its numbered sequence. ";
const TARGETS: [usize; 3] = [4_096, 32_768, 65_536];

fn fixture(tokenizer: &QwenTokenizer, body: &str) -> Result<(String, Vec<u32>), Box<dyn Error>> {
    let messages = [json!({"role":"user", "content":body})];
    let rendered = tokenizer.render(&messages, &[], "medium", true, true, true)?;
    let ids = tokenizer.encode_rendered(&rendered)?.get_ids().to_vec();
    Ok((rendered, ids))
}

fn find_body(
    tokenizer: &QwenTokenizer,
    target: usize,
) -> Result<(String, String, Vec<u32>), Box<dyn Error>> {
    let mut upper = 1_usize;
    loop {
        let body = REPEATED_SEGMENT.repeat(upper);
        if fixture(tokenizer, &body)?.1.len() >= target {
            break;
        }
        upper = upper
            .checked_mul(2)
            .ok_or("benchmark prompt repeat count overflow")?;
    }

    let mut lower = 0_usize;
    while lower + 1 < upper {
        let middle = lower + (upper - lower) / 2;
        if fixture(tokenizer, &REPEATED_SEGMENT.repeat(middle))?
            .1
            .len()
            < target
        {
            lower = middle;
        } else {
            upper = middle;
        }
    }

    let start = lower.saturating_sub(2);
    let end = upper.saturating_add(2);
    for repeats in start..=end {
        let base = REPEATED_SEGMENT.repeat(repeats);
        let (rendered, ids) = fixture(tokenizer, &base)?;
        if ids.len() == target {
            return Ok((base, rendered, ids));
        }
        if ids.len() > target {
            continue;
        }

        let (rendered, ids) = fixture(tokenizer, &format!("{base}{}", " x".repeat(64)))?;
        if ids.len() < target {
            continue;
        }
        if ids.len() == target {
            return Ok((format!("{base}{}", " x".repeat(64)), rendered, ids));
        }

        let mut low = 0_usize;
        let mut high = 64_usize;
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            let body = format!("{base}{}", " x".repeat(middle));
            if fixture(tokenizer, &body)?.1.len() < target {
                low = middle;
            } else {
                high = middle;
            }
        }
        for padding in high.saturating_sub(2)..=high.saturating_add(2) {
            let body = format!("{base}{}", " x".repeat(padding));
            let (rendered, ids) = fixture(tokenizer, &body)?;
            if ids.len() == target {
                return Ok((body, rendered, ids));
            }
        }
    }
    Err(format!("could not construct a prompt with exactly {target} tokens").into())
}

fn sha256_token_ids(ids: &[u32]) -> String {
    let mut hasher = Sha256::new();
    for token in ids {
        hasher.update(token.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn main() -> Result<(), Box<dyn Error>> {
    let tokenizer_path = PathBuf::from(
        std::env::var_os("AGENTIX_QWEN_TOKENIZER_PATH")
            .ok_or("AGENTIX_QWEN_TOKENIZER_PATH is not set; run inside `nix develop`")?,
    );
    let template_path = PathBuf::from(
        std::env::var_os("AGENTIX_QWEN_TEMPLATE_PATH")
            .ok_or("AGENTIX_QWEN_TEMPLATE_PATH is not set; run inside `nix develop`")?,
    );
    let output_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("specs/021-strata-inference-research/fixtures/benchmark_prompts.json")
        });
    let tokenizer = QwenTokenizer::from_files(&tokenizer_path, &template_path)?;
    let mut prompts: Vec<Value> = Vec::new();
    for target in TARGETS {
        let (body, rendered, token_ids) = find_body(&tokenizer, target)?;
        prompts.push(json!({
            "target_tokens": target,
            "actual_tokens": token_ids.len(),
            "construction": {
                "message_role": "user",
                "repeated_segment": REPEATED_SEGMENT,
                "repeated_segment_count": body.matches(REPEATED_SEGMENT).count(),
                "padding_suffix": body.strip_prefix(&REPEATED_SEGMENT.repeat(body.matches(REPEATED_SEGMENT).count())).unwrap_or("")
            },
            "message_content": body,
            "rendered_prompt": rendered,
            "token_ids": token_ids,
            "token_ids_little_endian_sha256": sha256_token_ids(&token_ids)
        }));
    }
    let artifact = json!({
        "tokenizer_token_count": tokenizer.tokenizer_token_count(),
        "model_vocabulary_size": tokenizer.model_vocabulary_size(),
        "stop_token_ids": tokenizer.stop_token_ids(),
        "tokenizer_artifact_sha256": "sha256-CZf0EMV6H05TsJ5L6PShctkO3ZVkNo+whHAwk3IpufM=",
        "chat_template_artifact_sha256": "sha256-w8+eNKv0+eNsLXIWWqnBMtPipyW2wlhqqjqK+deoEEE=",
        "prompts": prompts
    });
    let serialized = serde_json::to_vec_pretty(&artifact)?;
    fs::write(&output_path, serialized)?;
    println!("wrote {}", output_path.display());
    Ok(())
}
