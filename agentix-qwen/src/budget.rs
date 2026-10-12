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
        [self.weights, self.context, self.scratch, self.staging, self.lookup_cache, self.headroom]
            .into_iter()
            .try_fold(0_u64, |sum, value| sum.checked_add(value).ok_or(Error::Overflow))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admission {
    pub host_remaining: u64,
    pub device_remaining: u64,
}

pub fn admit(host_limit: u64, device_limit: u64, host: Reservations, device: Reservations) -> Result<Admission> {
    let host_remaining = host_limit.checked_sub(host.total()?).ok_or(Error::Capacity("host RAM"))?;
    let device_remaining = device_limit.checked_sub(device.total()?).ok_or(Error::Capacity("device VRAM"))?;
    Ok(Admission { host_remaining, device_remaining })
}

impl Admission {
    pub fn expert_slots(self, bytes_per_expert: u64, max_experts: u64) -> Result<u64> {
        if bytes_per_expert == 0 { return Err(Error::Invalid("zero-sized expert")); }
        Ok((self.device_remaining / bytes_per_expert).min(max_experts))
    }
}
