use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reservations {
    pub weights: u64,
    pub context: u64,
    pub scratch: u64,
    pub staging: u64,
    pub lookup_cache: u64,
    pub headroom: u64,
}

impl Reservations {
    pub fn total(self) -> Result<u64> {
        [
            self.weights,
            self.context,
            self.scratch,
            self.staging,
            self.lookup_cache,
            self.headroom,
        ]
        .into_iter()
        .try_fold(0_u64, |sum, value| {
            sum.checked_add(value).ok_or(Error::Overflow)
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admission {
    pub host_remaining: u64,
    pub device_remaining: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextAdmission {
    pub prompt_tokens: usize,
    pub requested_output_tokens: usize,
    pub total_tokens: usize,
    pub remaining_tokens: usize,
}

/// Admits a rendered prompt and the caller's full requested generation bound.
/// This never truncates the prompt or silently reduces `max_new_tokens`.
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

pub fn admit(
    host_limit: u64,
    device_limit: u64,
    host: Reservations,
    device: Reservations,
) -> Result<Admission> {
    let host_remaining = host_limit
        .checked_sub(host.total()?)
        .ok_or(Error::Capacity("host RAM"))?;
    let device_remaining = device_limit
        .checked_sub(device.total()?)
        .ok_or(Error::Capacity("device VRAM"))?;
    Ok(Admission {
        host_remaining,
        device_remaining,
    })
}

impl Admission {
    pub fn expert_slots(self, bytes_per_expert: u64, max_experts: u64) -> Result<u64> {
        if bytes_per_expert == 0 {
            return Err(Error::Invalid("zero-sized expert"));
        }
        Ok((self.device_remaining / bytes_per_expert).min(max_experts))
    }
}
