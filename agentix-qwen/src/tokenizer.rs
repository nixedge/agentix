use crate::budget::{admit_context, ContextAdmission};
use crate::{Error, Result};
use minijinja::{Environment, ErrorKind, Value as TemplateValue};
use serde_json::json;
use std::path::Path;
use tokenizers::{Encoding, Tokenizer};

const TEMPLATE_NAME: &str = "qwen4-exp-chat";
const TOKENIZER_TOKEN_COUNT: usize = 248_077;
const MODEL_VOCABULARY_SIZE: usize = 248_320;
const MODEL_EOS: u32 = 248_044;
const TURN_END: u32 = 248_046;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GreedyStopReason {
    StopToken(u32),
    TokenLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GreedyGeneration {
    /// Includes a sampled stop token when `stop_reason` is `StopToken`.
    pub generated_token_ids: Vec<u32>,
    pub stop_reason: GreedyStopReason,
}

/// Selects the highest finite logit, resolving ties by the lower token ID.
pub fn greedy_next_token(logits: &[f32]) -> Result<u32> {
    if logits.is_empty() || logits.iter().any(|logit| !logit.is_finite()) {
        return Err(Error::Invalid(
            "greedy sampler received empty or non-finite logits",
        ));
    }
    let mut best = 0_usize;
    for index in 1..logits.len() {
        if logits[index] > logits[best] {
            best = index;
        }
    }
    u32::try_from(best).map_err(|_| Error::Overflow)
}

/// Runs ordinary greedy token selection over a caller-provided next-logits function.
///
/// The closure sees the complete prompt plus previously generated IDs. A sampled
/// stop token terminates generation without another logits call.
pub fn generate_greedy<F>(
    prompt_token_ids: &[u32],
    max_new_tokens: usize,
    stop_token_ids: &[u32],
    mut next_logits: F,
) -> Result<GreedyGeneration>
where
    F: FnMut(&[u32]) -> Result<Vec<f32>>,
{
    let total_capacity = prompt_token_ids
        .len()
        .checked_add(max_new_tokens)
        .ok_or(Error::Overflow)?;
    let mut context = Vec::new();
    context
        .try_reserve_exact(total_capacity)
        .map_err(|_| Error::Capacity("greedy generation context"))?;
    context.extend_from_slice(prompt_token_ids);
    let mut generated = Vec::new();
    generated
        .try_reserve_exact(max_new_tokens)
        .map_err(|_| Error::Capacity("greedy generation output"))?;

    for _ in 0..max_new_tokens {
        let logits = next_logits(&context)?;
        let token = greedy_next_token(&logits)?;
        generated.push(token);
        context.push(token);
        if stop_token_ids.contains(&token) {
            return Ok(GreedyGeneration {
                generated_token_ids: generated,
                stop_reason: GreedyStopReason::StopToken(token),
            });
        }
    }
    Ok(GreedyGeneration {
        generated_token_ids: generated,
        stop_reason: GreedyStopReason::TokenLimit,
    })
}

pub struct PromptRequest<'a> {
    pub messages: &'a [serde_json::Value],
    pub tools: &'a [serde_json::Value],
    pub reasoning_effort: &'a str,
    pub enable_thinking: bool,
    pub preserve_thinking: bool,
    pub add_generation_prompt: bool,
    pub max_new_tokens: usize,
    pub context_limit: usize,
}

pub struct PreparedPrompt {
    pub rendered: String,
    pub token_ids: Vec<u32>,
    pub admission: ContextAdmission,
}

/// Pinned Qwen tokenizer plus the checkpoint's native Jinja chat template.
/// Template input uses JSON messages in the Transformers chat-template schema.
pub struct QwenTokenizer {
    tokenizer: Tokenizer,
    environment: Environment<'static>,
}

impl QwenTokenizer {
    pub fn from_files(tokenizer_path: &Path, template_path: &Path) -> Result<Self> {
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|_| Error::Invalid("failed to load Qwen tokenizer JSON"))?;
        Self::from_parts(tokenizer, std::fs::read_to_string(template_path)?)
    }

    pub fn from_parts(tokenizer: Tokenizer, template: String) -> Result<Self> {
        if tokenizer.get_vocab_size(true) != TOKENIZER_TOKEN_COUNT {
            return Err(Error::Invalid("Qwen tokenizer token count mismatch"));
        }
        for (token, expected_id) in [
            ("<|endoftext|>", MODEL_EOS),
            ("<|im_start|>", 248_045),
            ("<|im_end|>", TURN_END),
            ("<tool_call>", 248_058),
            ("</tool_call>", 248_059),
            ("<tool_response>", 248_066),
            ("</tool_response>", 248_067),
            ("<think>", 248_068),
            ("</think>", 248_069),
        ] {
            if tokenizer.token_to_id(token) != Some(expected_id) {
                return Err(Error::Invalid("Qwen tokenizer special-token ID mismatch"));
            }
        }
        let mut environment = Environment::new();
        environment.add_function(
            "raise_exception",
            |message: String| -> std::result::Result<String, minijinja::Error> {
                Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    message,
                ))
            },
        );
        // MiniJinja does not implement Python/Jinja string methods by default.
        // The pinned Qwen template uses only startswith/endswith on strings.
        environment.set_unknown_method_callback(|_, value, method, args| {
            if value.kind() != minijinja::value::ValueKind::String {
                return Err(minijinja::Error::from(ErrorKind::UnknownMethod));
            }
            let Some(value) = value.as_str() else {
                return Err(minijinja::Error::from(ErrorKind::UnknownMethod));
            };
            if args.len() != 1 {
                return Err(minijinja::Error::new(
                    ErrorKind::InvalidOperation,
                    "string method expects one argument",
                ));
            }
            let Some(argument) = args[0].as_str() else {
                return Err(minijinja::Error::new(
                    ErrorKind::InvalidOperation,
                    "string method argument must be a string",
                ));
            };
            match method {
                "startswith" => Ok(TemplateValue::from(value.starts_with(argument))),
                "endswith" => Ok(TemplateValue::from(value.ends_with(argument))),
                _ => Err(minijinja::Error::from(ErrorKind::UnknownMethod)),
            }
        });
        environment
            .add_template_owned(TEMPLATE_NAME.to_owned(), template)
            .map_err(|_| Error::Invalid("invalid Qwen chat template"))?;
        Ok(Self {
            tokenizer,
            environment,
        })
    }

    /// Renders ordinary text/tool messages. Vision, video, and non-text content
    /// are rejected because this backend currently has no multimodal encoder.
    pub fn render(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        reasoning_effort: &str,
        enable_thinking: bool,
        preserve_thinking: bool,
        add_generation_prompt: bool,
    ) -> Result<String> {
        if messages.is_empty() {
            return Err(Error::Invalid("Qwen prompt has no messages"));
        }
        for message in messages {
            let role = message
                .get("role")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if !matches!(role, "system" | "user" | "assistant" | "tool") {
                return Err(Error::Invalid("unsupported Qwen message role"));
            }
            if let Some(content) = message.get("content") {
                if !content.is_null() && !content.is_string() {
                    return Err(Error::Invalid(
                        "Qwen text backend does not accept multimodal content",
                    ));
                }
            }
        }
        if !matches!(reasoning_effort, "xhigh" | "medium" | "low") {
            return Err(Error::Invalid("unsupported Qwen reasoning effort"));
        }
        let context = TemplateValue::from_serialize(json!({
            "messages": messages,
            "tools": tools,
            "enable_thinking": enable_thinking,
            "preserve_thinking": preserve_thinking,
            "reasoning_effort": reasoning_effort,
            "add_generation_prompt": add_generation_prompt,
            "add_vision_id": false,
        }));
        self.environment
            .get_template(TEMPLATE_NAME)
            .and_then(|template| template.render(context))
            .map_err(|_| Error::Invalid("Qwen chat-template rendering failed"))
    }

    /// The chat template already writes BOS/role tokens; do not add a second
    /// tokenizer post-processing sequence.
    pub fn encode_rendered(&self, rendered: &str) -> Result<Encoding> {
        self.tokenizer
            .encode(rendered, false)
            .map_err(|_| Error::Invalid("Qwen prompt tokenization failed"))
    }

    /// Renders and tokenizes a request, then admits its actual prompt length
    /// without truncating the prompt or reducing the requested output bound.
    pub fn prepare_prompt(&self, request: PromptRequest<'_>) -> Result<PreparedPrompt> {
        let rendered = self.render(
            request.messages,
            request.tools,
            request.reasoning_effort,
            request.enable_thinking,
            request.preserve_thinking,
            request.add_generation_prompt,
        )?;
        let encoding = self.encode_rendered(&rendered)?;
        let token_ids = encoding.get_ids();
        let admission = admit_context(
            token_ids.len(),
            request.max_new_tokens,
            request.context_limit,
        )?;
        let mut owned_token_ids = Vec::new();
        owned_token_ids
            .try_reserve_exact(token_ids.len())
            .map_err(|_| Error::Capacity("Qwen prompt token IDs"))?;
        owned_token_ids.extend_from_slice(token_ids);
        Ok(PreparedPrompt {
            rendered,
            token_ids: owned_token_ids,
            admission,
        })
    }

    pub fn decode(&self, token_ids: &[u32]) -> Result<String> {
        self.tokenizer
            .decode(token_ids, false)
            .map_err(|_| Error::Invalid("Qwen token decoding failed"))
    }

    /// Number of token IDs represented by the tokenizer artifact.
    pub fn tokenizer_token_count(&self) -> usize {
        self.tokenizer.get_vocab_size(true)
    }

    /// Padded embedding vocabulary from the pinned model configuration.
    pub fn model_vocabulary_size(&self) -> usize {
        MODEL_VOCABULARY_SIZE
    }

    pub fn stop_token_ids(&self) -> [u32; 2] {
        // Pinned generation_config.json orders end-of-turn before end-of-text.
        [TURN_END, MODEL_EOS]
    }
}
