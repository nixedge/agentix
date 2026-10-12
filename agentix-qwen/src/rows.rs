use crate::{Error, Result};
use std::{collections::HashMap, fs::File, os::unix::fs::FileExt};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub hits: u64,
    pub misses: u64,
    pub bytes_read: u64,
    pub evictions: u64,
}

struct Entry {
    bytes: Vec<u8>,
    touched: u64,
}

/// Fixed-width encoded rows. The caller supplies an independently validated decoder.
/// Blocking file I/O must run outside Tokio workers.
pub struct RowStore {
    file: File,
    offset: u64,
    rows: u64,
    row_bytes: usize,
    capacity: usize,
    entries: HashMap<u64, Entry>,
    clock: u64,
    stats: Stats,
}

impl RowStore {
    pub fn new(file: File, offset: u64, rows: u64, row_bytes: usize, cache_bytes: usize) -> Result<Self> {
        if rows == 0 || row_bytes == 0 || row_bytes > 16 * 1024 * 1024 {
            return Err(Error::Invalid("row dimensions outside supported bounds"));
        }
        let extent = rows.checked_mul(row_bytes as u64).and_then(|n| n.checked_add(offset)).ok_or(Error::Overflow)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || extent > metadata.len() {
            return Err(Error::Invalid("row payload exceeds regular file length"));
        }
        // Capacity bounds retained payload only; account for metadata and caller buffers separately.
        let capacity = (cache_bytes / row_bytes).min(usize::try_from(rows).unwrap_or(usize::MAX));
        Ok(Self { file, offset, rows, row_bytes, capacity, entries: HashMap::new(), clock: 0, stats: Stats::default() })
    }

    pub fn cached_bytes(&self) -> usize { self.entries.len() * self.row_bytes }
    pub fn stats(&self) -> Stats { self.stats }

    pub fn read(&mut self, row: u64) -> Result<Vec<u8>> {
        if row >= self.rows { return Err(Error::Invalid("row outside table")); }
        // Clearing on wrap retains correctness without introducing ambiguous LRU ages.
        if self.clock == u64::MAX { self.entries.clear(); self.clock = 0; }
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(&row) {
            self.stats.hits = self.stats.hits.saturating_add(1);
            entry.touched = self.clock;
            return Ok(entry.bytes.clone());
        }
        let offset = self.offset + row * self.row_bytes as u64;
        let mut bytes = vec![0; self.row_bytes];
        self.file.read_exact_at(&mut bytes, offset)?;
        self.stats.misses = self.stats.misses.saturating_add(1);
        self.stats.bytes_read = self.stats.bytes_read.saturating_add(self.row_bytes as u64);
        if self.capacity > 0 {
            if self.entries.len() == self.capacity {
                if let Some(oldest) = self.entries.iter().min_by_key(|(_, entry)| entry.touched).map(|(&key, _)| key) {
                    self.entries.remove(&oldest);
                    self.stats.evictions = self.stats.evictions.saturating_add(1);
                }
            }
            self.entries.insert(row, Entry { bytes: bytes.clone(), touched: self.clock });
        }
        Ok(bytes)
    }
}
