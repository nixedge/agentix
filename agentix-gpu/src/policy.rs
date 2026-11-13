use crate::protocol::{Allocation, Error, Owner, Result};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Observation {
    pub device: u32,
    pub uuid: String,
    pub total: u64,
    pub free: u64,
    pub processes: HashMap<u32, u64>,
}
#[derive(Clone, Debug)]
pub struct Held {
    pub owner: Owner,
    pub allocation: Allocation,
}

/// Free bytes less safety and still-unallocated peak reservations. Missing
/// process measurements leave the entire reservation charged as future growth.
pub fn available(observation: &Observation, held: &[Held], safety: u64) -> Result<u64> {
    if observation.free > observation.total {
        return Err(Error::Invalid("device free memory exceeds total".into()));
    }
    let mut peaks: HashMap<u32, (u64, u64)> = HashMap::new();
    for entry in held
        .iter()
        .filter(|h| h.allocation.device == observation.device)
    {
        if entry.allocation.resident_floor > entry.allocation.bytes {
            return Err(Error::Invalid(
                "resident floor exceeds its peak lease".into(),
            ));
        }
        let (peak, floor) = peaks.entry(entry.owner.process.pid).or_default();
        *peak = peak
            .checked_add(entry.allocation.bytes)
            .ok_or_else(|| Error::Invalid("GPU reservation overflow".into()))?;
        *floor = floor
            .checked_add(entry.allocation.resident_floor)
            .ok_or_else(|| Error::Invalid("GPU resident floor overflow".into()))?;
    }
    let slack = peaks
        .into_iter()
        .try_fold(0u64, |sum, (pid, (reserved, floor))| {
            // A transient scratch peak may disappear between driver observations.
            // Credit only guaranteed persistent allocations, never that transient peak.
            let resident = observation
                .processes
                .get(&pid)
                .copied()
                .unwrap_or(0)
                .min(floor);
            sum.checked_add(reserved.saturating_sub(resident))
                .ok_or_else(|| Error::Invalid("GPU reservation overflow".into()))
        })?;
    Ok(observation
        .free
        .saturating_sub(safety)
        .saturating_sub(slack))
}

pub fn external_bytes(observation: &Observation, held: &[Held]) -> u64 {
    let mut registered: Vec<u32> = held
        .iter()
        .filter(|h| h.allocation.device == observation.device)
        .map(|h| h.owner.process.pid)
        .collect();
    registered.sort_unstable();
    registered.dedup();
    let own = registered.into_iter().fold(0u64, |sum, pid| {
        let reserved = held
            .iter()
            .filter(|h| h.owner.process.pid == pid && h.allocation.device == observation.device)
            .fold(0u64, |sum, h| sum.saturating_add(h.allocation.bytes));
        sum.saturating_add(
            observation
                .processes
                .get(&pid)
                .copied()
                .unwrap_or(0)
                .min(reserved),
        )
    });
    observation
        .total
        .saturating_sub(observation.free)
        .saturating_sub(own)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Process;
    fn held(pid: u32, bytes: u64) -> Held {
        Held {
            owner: Owner {
                role: "test".into(),
                session: 1,
                process: Process {
                    pid,
                    start_ticks: 1,
                },
            },
            allocation: Allocation {
                id: format!("{pid}"),
                model: "test".into(),
                device: 0,
                bytes,
                resident_floor: bytes,
                cache: false,
                control: None,
            },
        }
    }
    fn observation(free: u64, processes: &[(u32, u64)]) -> Observation {
        Observation {
            device: 0,
            uuid: "gpu".into(),
            total: 100,
            free,
            processes: processes.iter().copied().collect(),
        }
    }
    #[test]
    fn charges_pending_peak_and_external_usage() -> Result<()> {
        let state = vec![held(1, 40)];
        assert_eq!(available(&observation(70, &[(1, 20)]), &state, 5)?, 45);
        assert_eq!(external_bytes(&observation(70, &[(1, 20)]), &state), 10);
        Ok(())
    }
    #[test]
    fn active_and_loading_allocations_share_one_budget() -> Result<()> {
        let state = vec![held(1, 40), held(2, 30)];
        assert_eq!(available(&observation(70, &[(1, 20)]), &state, 5)?, 15);
        Ok(())
    }
    #[test]
    fn process_slack_is_aggregated_without_double_counting_resident() -> Result<()> {
        assert_eq!(
            available(&observation(50, &[(1, 50)]), &[held(1, 20), held(1, 40)], 5)?,
            35
        );
        Ok(())
    }
    #[test]
    fn unknown_process_usage_is_conservative() -> Result<()> {
        assert_eq!(available(&observation(50, &[]), &[held(1, 40)], 5)?, 5);
        Ok(())
    }
    #[test]
    fn unleased_usage_inside_a_registered_process_is_external() {
        assert_eq!(
            external_bytes(&observation(20, &[(1, 70)]), &[held(1, 20)]),
            60
        );
    }
    #[test]
    fn impossible_observations_and_overflow_fail_closed() {
        assert!(available(&observation(101, &[]), &[], 0).is_err());
        assert!(available(&observation(50, &[]), &[held(1, u64::MAX), held(1, 1)], 0).is_err());
    }
    #[test]
    fn zero_slack_when_actual_usage_exceeds_reserved_peak() -> Result<()> {
        assert_eq!(
            available(&observation(20, &[(1, 80)]), &[held(1, 40)], 5)?,
            15
        );
        Ok(())
    }
    #[test]
    fn transient_scratch_measurements_never_release_future_growth_slack() -> Result<()> {
        let mut entry = held(1, 60);
        entry.allocation.resident_floor = 20;
        assert_eq!(available(&observation(50, &[(1, 50)]), &[entry], 5)?, 5);
        Ok(())
    }
}
