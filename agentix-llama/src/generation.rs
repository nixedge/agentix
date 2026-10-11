use agentix_infer::{exchange::Cancellation, FinishReason, GrammarConstraint, InferError};
use llama_cpp_2::{
    context::LlamaContext,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel},
    sampling::LlamaSampler,
};

pub(crate) struct Options<'a> {
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub grammar: Option<&'a GrammarConstraint>,
    pub stop: &'a [String],
}

pub(crate) fn generate(
    model: &LlamaModel,
    ctx: &mut LlamaContext<'_>,
    prompt: &str,
    options: Options<'_>,
    cancel: &Cancellation,
    mut emit: impl FnMut(String) -> Result<(), InferError>,
) -> Result<FinishReason, InferError> {
    if options.max_tokens == 0 || options.max_tokens > 32768 || prompt.len() > 1024 * 1024 {
        return Err(InferError::InvalidToolExchange(
            "generation limits exceeded",
        ));
    }
    let tokens = model
        .str_to_token(prompt, AddBos::Always)
        .map_err(backend_error)?;
    if tokens.len() as u64 + u64::from(options.max_tokens) > u64::from(ctx.n_ctx()) {
        return Err(InferError::ContextExceeded {
            prompt_tokens: tokens.len() as u32,
            max_new_tokens: options.max_tokens,
            context_window: ctx.n_ctx(),
        });
    }
    if tokens.is_empty() {
        return Ok(FinishReason::Stop);
    }
    ctx.clear_kv_cache();
    // Bound the interval between cancellation checks during prefill.
    for (offset, part) in tokens.chunks(64).enumerate() {
        check_cancel(cancel)?;
        let mut batch = LlamaBatch::new(part.len(), 1);
        for (index, token) in part.iter().enumerate() {
            let position = offset * 64 + index;
            batch
                .add(*token, position as i32, &[0], position == tokens.len() - 1)
                .map_err(backend_error)?;
        }
        ctx.decode(&mut batch).map_err(backend_error)?;
    }
    let mut samplers = Vec::new();
    if let Some(GrammarConstraint::Gbnf(grammar)) = options.grammar {
        samplers.push(LlamaSampler::grammar(model, grammar, "root").map_err(backend_error)?);
    }
    if options.temperature == Some(0.0) {
        samplers.push(LlamaSampler::greedy());
    } else {
        samplers.extend([
            LlamaSampler::top_k(40),
            LlamaSampler::top_p(options.top_p.unwrap_or(0.95), 1),
            LlamaSampler::temp(options.temperature.unwrap_or(0.7)),
            LlamaSampler::dist(0xDEAD_BEEF),
        ]);
    }
    let mut sampler = LlamaSampler::chain_simple(samplers);
    let mut utf8 = Vec::new();
    let mut pending = String::new();
    let mut total_bytes = 0usize;
    for index in 0..options.max_tokens {
        check_cancel(cancel)?;
        let token = sampler.sample(ctx, -1);
        if model.is_eog_token(token) {
            if !utf8.is_empty() {
                return Err(InferError::Backend(
                    "incomplete UTF-8 at end of generation".into(),
                ));
            }
            if !pending.is_empty() {
                emit(pending)?;
            }
            return Ok(FinishReason::Stop);
        }
        utf8.extend(
            model
                .token_to_piece_bytes(token, 256, false, None)
                .map_err(backend_error)?,
        );
        let valid = match std::str::from_utf8(&utf8) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => return Err(InferError::Backend("invalid UTF-8 generated".into())),
        };
        let text = std::str::from_utf8(&utf8[..valid]).map_err(backend_error)?;
        total_bytes = total_bytes.saturating_add(text.len());
        if total_bytes > super::tool_template::MAX_OUTPUT {
            return Err(InferError::InvalidToolExchange(
                "model output exceeds limit",
            ));
        }
        pending.push_str(text);
        utf8.drain(..valid);
        if let Some(position) = options
            .stop
            .iter()
            .filter(|s| !s.is_empty())
            .filter_map(|stop| pending.find(stop))
            .min()
        {
            emit(pending[..position].to_string())?;
            return Ok(FinishReason::Stop);
        }
        let retain = options
            .stop
            .iter()
            .filter(|s| !s.is_empty())
            .flat_map(|stop| {
                (1..stop.len())
                    .filter(|n| stop.is_char_boundary(*n))
                    .map(move |n| &stop[..n])
            })
            .filter(|prefix| pending.ends_with(prefix))
            .map(str::len)
            .max()
            .unwrap_or(0);
        let ready = pending.len() - retain;
        if ready > 0 {
            emit(pending[..ready].to_string())?;
            pending.drain(..ready);
        }
        if index + 1 < options.max_tokens {
            check_cancel(cancel)?;
            let mut batch = LlamaBatch::new(1, 1);
            batch
                .add(token, (tokens.len() + index as usize) as i32, &[0], true)
                .map_err(backend_error)?;
            ctx.decode(&mut batch).map_err(backend_error)?;
        }
    }
    if !pending.is_empty() {
        emit(pending)?;
    }
    Ok(FinishReason::Length)
}

fn check_cancel(cancel: &Cancellation) -> Result<(), InferError> {
    if cancel.is_cancelled() {
        Err(InferError::Cancelled)
    } else {
        Ok(())
    }
}
fn backend_error(error: impl std::fmt::Debug) -> InferError {
    InferError::Backend(format!("generation failed: {error:?}"))
}
