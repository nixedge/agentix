use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextAdmission {
    pub prompt_tokens: usize,
    pub requested_output_tokens: usize,
    pub total_tokens: usize,
    pub remaining_tokens: usize,
}

/// Admit the full rendered prompt and requested output without silent truncation.
pub fn admit_context(
    prompt_tokens: usize,
    max_new_tokens: usize,
    context_limit: usize,
) -> Result<ContextAdmission> {
    if prompt_tokens == 0 || max_new_tokens == 0 || context_limit == 0 {
        return Err(Error::Invalid(
            "empty prompt, output bound, or context limit",
        ));
    }
    let total_tokens = prompt_tokens
        .checked_add(max_new_tokens)
        .ok_or(Error::Overflow)?;
    let remaining_tokens = context_limit
        .checked_sub(total_tokens)
        .ok_or(Error::Capacity("context window"))?;
    Ok(ContextAdmission {
        prompt_tokens,
        requested_output_tokens: max_new_tokens,
        total_tokens,
        remaining_tokens,
    })
}
