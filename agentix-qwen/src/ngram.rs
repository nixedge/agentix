use crate::{Error, Result};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub vocabulary: u32,
    pub eos: u32,
    pub size: usize,
    pub heads_per_size: usize,
    pub vocabulary_base: u64,
    pub layer_ordinal: usize,
    pub seed: u64,
}

impl Config {
    pub fn flash_next() -> Self {
        Self {
            vocabulary: 248_320,
            eos: 248_044,
            size: 3,
            heads_per_size: 8,
            vocabulary_base: 20_000_000,
            layer_ordinal: 0,
            seed: 1234,
        }
    }
}

#[derive(Debug)]
struct Layout {
    config: Config,
    multipliers: Vec<u64>,
    moduli: Vec<u64>,
    offsets: Vec<u64>,
    total_rows: u64,
}

#[derive(Clone)]
pub struct Snapshot {
    layout: Arc<Layout>,
    history: Vec<u32>,
}

pub struct Ngram {
    layout: Arc<Layout>,
    history: Vec<u32>,
}

impl Ngram {
    pub fn new(config: Config) -> Result<Self> {
        if config.vocabulary == 0
            || config.eos >= config.vocabulary
            || !(2..=16).contains(&config.size)
            || !(1..=128).contains(&config.heads_per_size)
            || !(2..=1_000_000_000).contains(&config.vocabulary_base)
            || config.layer_ordinal > 128
        {
            return Err(Error::Invalid("n-gram geometry outside supported bounds"));
        }
        let heads = (config.size - 1) * config.heads_per_size;
        let mut prime = config.vocabulary_base - 1;
        for _ in 0..config.layer_ordinal * heads {
            prime = next_prime(prime)?;
        }
        let mut moduli = Vec::with_capacity(heads);
        let mut offsets = Vec::with_capacity(heads);
        let mut total_rows = 0_u64;
        for _ in 0..heads {
            prime = next_prime(prime)?;
            offsets.push(total_rows);
            moduli.push(prime);
            total_rows = total_rows.checked_add(prime).ok_or(Error::Overflow)?;
        }
        let half_bound = ((i64::MAX as u64 / u64::from(config.vocabulary)) / 2).max(1);
        let base = config.seed.wrapping_add(10007 * config.layer_ordinal as u64);
        let multipliers = (0..config.size)
            .map(|index| {
                let value = base.wrapping_add(0x9e3779b97f4a7c15_u64.wrapping_mul(index as u64 + 1));
                2 * (splitmix(value) % half_bound) + 1
            })
            .collect();
        Ok(Self {
            layout: Arc::new(Layout { config, multipliers, moduli, offsets, total_rows }),
            history: Vec::new(),
        })
    }

    pub fn total_rows(&self) -> u64 {
        self.layout.total_rows
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot { layout: Arc::clone(&self.layout), history: self.history.clone() }
    }

    pub fn restore(&mut self, snapshot: &Snapshot) -> Result<()> {
        if self.layout.config != snapshot.layout.config {
            return Err(Error::Invalid("n-gram snapshot belongs to another layout"));
        }
        self.history.clone_from(&snapshot.history);
        Ok(())
    }

    pub fn reset(&mut self) {
        self.history.clear();
    }

    pub fn push(&mut self, token: u32) -> Result<Vec<u64>> {
        let layout = &self.layout;
        if token >= layout.config.vocabulary {
            return Err(Error::Invalid("token outside vocabulary"));
        }
        let mut mixed = u64::from(token) * layout.multipliers[0];
        let mut rows = Vec::with_capacity(layout.moduli.len());
        for shift in 1..layout.config.size {
            let previous = self.history.iter().rev().nth(shift - 1).copied().unwrap_or(layout.config.eos);
            mixed ^= u64::from(previous) * layout.multipliers[shift];
            let first = (shift - 1) * layout.config.heads_per_size;
            for head in first..first + layout.config.heads_per_size {
                rows.push(layout.offsets[head] + mixed % layout.moduli[head]);
            }
        }
        // EOS itself sees the old segment; only subsequent tokens lose that history.
        if token == layout.config.eos {
            self.history.clear();
        } else {
            self.history.push(token);
            if self.history.len() >= layout.config.size {
                self.history.remove(0);
            }
        }
        Ok(rows)
    }

    pub fn extend(&mut self, tokens: &[u32]) -> Result<Vec<Vec<u64>>> {
        if tokens.iter().any(|&token| token >= self.layout.config.vocabulary) {
            return Err(Error::Invalid("token outside vocabulary"));
        }
        tokens.iter().map(|&token| self.push(token)).collect()
    }
}

fn splitmix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

fn next_prime(mut value: u64) -> Result<u64> {
    loop {
        value = value.checked_add(1).ok_or(Error::Overflow)?;
        if prime(value) { return Ok(value); }
    }
}

fn prime(value: u64) -> bool {
    if value < 2 { return false; }
    if value % 2 == 0 { return value == 2; }
    let mut divisor = 3;
    while divisor <= value / divisor {
        if value % divisor == 0 { return false; }
        divisor += 2;
    }
    true
}
