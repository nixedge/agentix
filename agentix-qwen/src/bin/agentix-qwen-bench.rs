use agentix_qwen::{
    engine::{Engine, Options},
    tokenizer::{greedy_next_token, PromptRequest, QwenTokenizer},
};
use serde_json::json;
use std::{error::Error, path::PathBuf, time::Instant};

fn number(name: &str, default: usize) -> Result<usize, Box<dyn Error>> {
    Ok(std::env::var(name)
        .ok()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(default))
}
fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os().nth(1).map(PathBuf::from).ok_or(
        "usage: agentix-qwen-bench FIRST_SHARD.gguf; set AGENTIX_QWEN_COMPUTE=cpu|cuda|vulkan",
    )?;
    let tokenizer_path = std::env::var_os("AGENTIX_QWEN_TOKENIZER")
        .or_else(|| std::env::var_os("AGENTIX_QWEN_TOKENIZER_PATH"))
        .ok_or("missing AGENTIX_QWEN_TOKENIZER")?;
    let template_path = std::env::var_os("AGENTIX_QWEN_CHAT_TEMPLATE")
        .or_else(|| std::env::var_os("AGENTIX_QWEN_TEMPLATE_PATH"))
        .ok_or("missing AGENTIX_QWEN_CHAT_TEMPLATE")?;
    let tokenizer = QwenTokenizer::from_files(
        &PathBuf::from(tokenizer_path),
        &PathBuf::from(template_path),
    )?;
    let context = number("AGENTIX_QWEN_CONTEXT", 4096)?;
    let maximum = number("AGENTIX_QWEN_BENCH_TOKENS", 128)?;
    let prompt = std::env::var("AGENTIX_QWEN_BENCH_PROMPT")
        .unwrap_or_else(|_| "Explain how a Rust iterator works, with a short example.".into());
    let messages = [json!({"role":"user", "content":prompt})];
    let prepared = tokenizer.prepare_prompt(PromptRequest {
        messages: &messages,
        tools: &[],
        reasoning_effort: "medium",
        enable_thinking: false,
        preserve_thinking: false,
        add_generation_prompt: true,
        max_new_tokens: maximum,
        context_limit: context,
    })?;
    let started = Instant::now();
    let engine = Engine::open(
        path.clone(),
        Options {
            backend: std::env::var("AGENTIX_QWEN_COMPUTE").unwrap_or_else(|_| "cuda".into()),
            device: number("AGENTIX_QWEN_CUDA_DEVICE", 0)?,
            context,
            chunk: number("AGENTIX_QWEN_PREFILL_CHUNK", 256)?,
            vram_margin: number("AGENTIX_QWEN_VRAM_MARGIN", 512 * 1024 * 1024)?,
        },
    )?;
    println!(
        "{}",
        json!({"event":"loaded", "revision":"9dd6340d25c7c6770f7918134a27984cf20d7ce3", "model":path,
        "architecture":engine.info.architecture,"backend":engine.info.backend,"device":engine.info.device,
        "load_seconds":started.elapsed().as_secs_f64(),"context":context,"prompt_tokens":prepared.token_ids.len(),"max_tokens":maximum})
    );
    let stops = tokenizer.stop_token_ids();
    for run in 0..=number("AGENTIX_QWEN_BENCH_RUNS", 3)? {
        let started = Instant::now();
        let mut sequence = engine.session()?;
        let mut logits = sequence.prefill(&prepared.token_ids)?;
        let mut ids = Vec::new();
        let mut first = None;
        let mut last = None;
        let mut stopped = false;
        for index in 0..maximum {
            let token = greedy_next_token(&logits)?;
            let now = Instant::now();
            first.get_or_insert(now);
            last = Some(now);
            if stops.contains(&token) {
                stopped = true;
                break;
            }
            ids.push(token);
            if index + 1 < maximum {
                logits = sequence.step(token)?;
            }
        }
        let decode = match (first, last) {
            (Some(first), Some(last)) => last.duration_since(first).as_secs_f64(),
            _ => 0.0,
        };
        let transitions = ids.len().saturating_sub(usize::from(!stopped));
        let throughput = (decode > 0.0).then_some(transitions as f64 / decode);
        println!(
            "{}",
            json!({"event":"generation","run":run,"warmup":run==0,
            "ttft_seconds":first.map(|first| first.duration_since(started).as_secs_f64()),
            "decode_seconds":decode,"decode_tokens_per_second":throughput,"generated_tokens":ids.len(),
            "total_seconds":started.elapsed().as_secs_f64(),"stopped":stopped,"token_ids":ids,"text":tokenizer.decode(&ids)?})
        );
    }
    Ok(())
}
