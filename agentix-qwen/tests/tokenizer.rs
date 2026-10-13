use agentix_qwen::tokenizer::{
    generate_greedy, greedy_next_token, GreedyStopReason, PromptRequest, QwenTokenizer,
};
use serde_json::{json, Value};
use std::{error::Error, io, path::PathBuf};

fn fixture(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    std::env::var_os(name).map(PathBuf::from).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("missing Nix-pinned fixture {name}"),
        )
        .into()
    })
}

fn tokenizer() -> Result<QwenTokenizer, Box<dyn Error>> {
    Ok(QwenTokenizer::from_files(
        &fixture("AGENTIX_QWEN_TOKENIZER_PATH")?,
        &fixture("AGENTIX_QWEN_TEMPLATE_PATH")?,
    )?)
}

#[test]
fn pinned_vocabulary_chat_template_and_generation_stops() -> Result<(), Box<dyn Error>> {
    let tokenizer = tokenizer()?;
    assert_eq!(tokenizer.tokenizer_token_count(), 248_077);
    assert_eq!(tokenizer.model_vocabulary_size(), 248_320);
    assert_eq!(tokenizer.stop_token_ids(), [248_046, 248_044]);
    let messages = [json!({"role":"user", "content":"Hi!"})];
    let rendered = tokenizer.render(&messages, &[], "medium", true, true, true)?;
    assert_eq!(
        rendered,
        "<|im_start|>user\nHi!<|im_end|>\n<|im_start|>assistant\n<think>\n"
    );
    let encoding = tokenizer.encode_rendered(&rendered)?;
    assert_eq!(
        encoding.get_ids(),
        &[248_045, 846, 198, 12_675, 0, 248_046, 198, 248_045, 74_455, 198, 248_068, 198]
    );
    let decoded = tokenizer.decode(encoding.get_ids())?;
    assert_eq!(decoded, rendered);
    Ok(())
}

#[test]
fn pinned_tool_and_reasoning_template_preserves_special_tokens() -> Result<(), Box<dyn Error>> {
    let tokenizer = tokenizer()?;
    let messages = [
        json!({
            "role":"assistant",
            "content":"Checking.",
            "reasoning_content":"Need a tool.",
            "tool_calls":[{"function":{"name":"lookup","arguments":{"q":"red fox"}}}]
        }),
        json!({"role":"tool", "content":"found"}),
        json!({"role":"user", "content":"Thanks"}),
    ];
    let tools: [Value; 1] =
        [json!({"type":"function", "function":{"name":"lookup", "parameters":{"type":"object"}}})];
    let rendered = tokenizer.render(&messages, &tools, "low", true, true, true)?;
    assert!(rendered.contains("<function=lookup>"));
    assert!(rendered.contains("<parameter=q>\nred fox\n</parameter>"));
    assert!(rendered.contains("<tool_response>\nfound\n</tool_response>"));
    assert!(rendered.contains("<think>\nNeed a tool.\n</think>"));
    assert!(rendered.ends_with("<|im_start|>assistant\n<think>\n"));
    let encoding = tokenizer.encode_rendered(&rendered)?;
    assert_eq!(tokenizer.decode(encoding.get_ids())?, rendered);
    Ok(())
}

#[test]
fn unsupported_prompts_and_reasoning_effort_are_rejected() -> Result<(), Box<dyn Error>> {
    let tokenizer = tokenizer()?;
    assert!(tokenizer
        .render(&[], &[], "medium", true, true, true)
        .is_err());
    let image = [json!({"role":"user", "content":[{"type":"image"}]})];
    assert!(tokenizer
        .render(&image, &[], "medium", true, true, true)
        .is_err());
    let text = [json!({"role":"user", "content":"Hi"})];
    assert!(tokenizer
        .render(&text, &[], "high", true, true, true)
        .is_err());
    Ok(())
}

#[test]
fn greedy_generation_uses_lower_id_ties_and_stops_without_an_extra_logits_call(
) -> Result<(), Box<dyn Error>> {
    assert_eq!(greedy_next_token(&[1.0, 3.0, 3.0])?, 1);
    let mut contexts = Vec::new();
    let generated = generate_greedy(&[10], 8, &[0, 2], |context| {
        contexts.push(context.to_vec());
        if context.len() == 1 {
            Ok(vec![0.0, 2.0, 2.0])
        } else {
            Ok(vec![4.0, 0.0])
        }
    })?;
    assert_eq!(generated.generated_token_ids, [1, 0]);
    assert_eq!(generated.stop_reason, GreedyStopReason::StopToken(0));
    assert_eq!(contexts, [vec![10], vec![10, 1]]);
    Ok(())
}

#[test]
fn greedy_generation_respects_token_limit_and_rejects_bad_logits() -> Result<(), Box<dyn Error>> {
    let mut calls = 0;
    let limited = generate_greedy(&[], 2, &[9], |_| {
        calls += 1;
        Ok(vec![0.0, 1.0])
    })?;
    assert_eq!(limited.generated_token_ids, [1, 1]);
    assert_eq!(limited.stop_reason, GreedyStopReason::TokenLimit);
    assert_eq!(calls, 2);

    let zero_limit = generate_greedy(&[3], 0, &[9], |_| {
        Err(agentix_qwen::Error::Invalid("unexpected logits request"))
    })?;
    assert!(zero_limit.generated_token_ids.is_empty());
    assert_eq!(zero_limit.stop_reason, GreedyStopReason::TokenLimit);
    assert!(greedy_next_token(&[]).is_err());
    assert!(greedy_next_token(&[f32::NAN]).is_err());
    Ok(())
}

#[test]
fn prepared_prompt_admits_rendered_token_count_and_keeps_requested_output(
) -> Result<(), Box<dyn Error>> {
    let tokenizer = tokenizer()?;
    let messages = [json!({"role":"user", "content":"Hi!"})];
    let prepared = tokenizer.prepare_prompt(PromptRequest {
        messages: &messages,
        tools: &[],
        reasoning_effort: "medium",
        enable_thinking: true,
        preserve_thinking: true,
        add_generation_prompt: true,
        max_new_tokens: 16,
        context_limit: 28,
    })?;
    assert_eq!(prepared.admission.prompt_tokens, prepared.token_ids.len());
    assert_eq!(prepared.admission.requested_output_tokens, 16);
    assert_eq!(prepared.admission.remaining_tokens, 0);
    assert!(tokenizer
        .prepare_prompt(PromptRequest {
            messages: &messages,
            tools: &[],
            reasoning_effort: "medium",
            enable_thinking: true,
            preserve_thinking: true,
            add_generation_prompt: true,
            max_new_tokens: 17,
            context_limit: 28,
        })
        .is_err());
    Ok(())
}
