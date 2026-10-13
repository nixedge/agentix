use crate::{Error, Result};
use std::{
    collections::HashMap,
    fs::File,
    os::unix::fs::FileExt,
    path::{Component, Path, PathBuf},
};

const MAX_FLOAT_VECTOR_ELEMENTS: usize = 1_000_000;

/// Parsed SafeTensors file header and the open read-only source file.
pub struct SafeTensorFile {
    file: File,
    metadata: safetensors::tensor::Metadata,
    data_offset: u64,
}

impl SafeTensorFile {
    /// Parses the bounded SafeTensors header and validates its full data extent.
    /// Tensor metadata deserialization checks all ranges and dtype/shape sizes.
    pub fn open(file: File) -> Result<Self> {
        let file_metadata = file.metadata()?;
        if !file_metadata.is_file() || file_metadata.len() < 8 {
            return Err(Error::Invalid("SafeTensors source is not a regular file"));
        }
        let mut size_prefix = [0_u8; 8];
        file.read_exact_at(&mut size_prefix, 0)?;
        let header_len = usize::try_from(u64::from_le_bytes(size_prefix))
            .map_err(|_| Error::Invalid("SafeTensors header is too large"))?;
        // Safetensors 0.8 uses this same hard limit for its whole-buffer parser.
        const MAX_HEADER_LEN: usize = 100_000_000;
        if header_len > MAX_HEADER_LEN {
            return Err(Error::Invalid("SafeTensors header is too large"));
        }
        let header_file_offset = 8_u64;
        let header_end = header_file_offset
            .checked_add(u64::try_from(header_len).map_err(|_| Error::Overflow)?)
            .ok_or(Error::Overflow)?;
        if header_end > file_metadata.len() {
            return Err(Error::Invalid("truncated SafeTensors header"));
        }
        let mut header = Vec::new();
        header
            .try_reserve_exact(header_len)
            .map_err(|_| Error::Capacity("SafeTensors header"))?;
        header.resize(header_len, 0);
        file.read_exact_at(&mut header, header_file_offset)?;
        let metadata = serde_json::from_slice::<safetensors::tensor::Metadata>(&header)
            .map_err(|_| Error::Invalid("invalid SafeTensors file metadata"))?;
        let data_offset = 8_u64
            .checked_add(u64::try_from(header_len).map_err(|_| Error::Overflow)?)
            .ok_or(Error::Overflow)?;
        let expected_file_len = data_offset
            .checked_add(u64::try_from(metadata.data_len()).map_err(|_| Error::Overflow)?)
            .ok_or(Error::Overflow)?;
        if expected_file_len != file_metadata.len() {
            return Err(Error::Invalid("SafeTensors payload length mismatch"));
        }
        Ok(Self {
            file,
            metadata,
            data_offset,
        })
    }

    /// Opens one rank-two BF16 tensor as a bounded on-demand row reader.
    pub fn bf16_rows(
        &self,
        tensor_name: &str,
        expected_shape: [usize; 2],
        cache_bytes: usize,
    ) -> Result<Bf16Rows> {
        let info = self
            .metadata
            .info(tensor_name)
            .ok_or(Error::Invalid("SafeTensors tensor is missing"))?;
        if info.dtype != safetensors::Dtype::BF16 || info.shape.as_slice() != expected_shape {
            return Err(Error::Invalid("SafeTensors BF16 tensor shape mismatch"));
        }
        let row_bytes = expected_shape[1].checked_mul(2).ok_or(Error::Overflow)?;
        let tensor_offset = self
            .data_offset
            .checked_add(u64::try_from(info.data_offsets.0).map_err(|_| Error::Overflow)?)
            .ok_or(Error::Overflow)?;
        let store = RowStore::new(
            self.file.try_clone()?,
            tensor_offset,
            u64::try_from(expected_shape[0]).map_err(|_| Error::Overflow)?,
            row_bytes,
            cache_bytes,
        )?;
        Ok(Bf16Rows {
            store,
            row_width: expected_shape[1],
        })
    }

    /// Opens one rank-two F32/F16/BF16 tensor as bounded rows decoded to F32.
    pub fn float_rows(
        &self,
        tensor_name: &str,
        expected_shape: [usize; 2],
        cache_bytes: usize,
    ) -> Result<FloatRows> {
        let info = self
            .metadata
            .info(tensor_name)
            .ok_or(Error::Invalid("SafeTensors tensor is missing"))?;
        let (encoding, element_bytes) = match info.dtype {
            safetensors::Dtype::F32 => (FloatEncoding::F32, 4_usize),
            safetensors::Dtype::F16 => (FloatEncoding::F16, 2_usize),
            safetensors::Dtype::BF16 => (FloatEncoding::BF16, 2_usize),
            _ => return Err(Error::Invalid("unsupported SafeTensors float dtype")),
        };
        if expected_shape.contains(&0) || info.shape.as_slice() != expected_shape {
            return Err(Error::Invalid("SafeTensors float tensor shape mismatch"));
        }
        let row_bytes = expected_shape[1]
            .checked_mul(element_bytes)
            .ok_or(Error::Overflow)?;
        let tensor_offset = self
            .data_offset
            .checked_add(u64::try_from(info.data_offsets.0).map_err(|_| Error::Overflow)?)
            .ok_or(Error::Overflow)?;
        let store = RowStore::new(
            self.file.try_clone()?,
            tensor_offset,
            u64::try_from(expected_shape[0]).map_err(|_| Error::Overflow)?,
            row_bytes,
            cache_bytes,
        )?;
        Ok(FloatRows {
            store,
            row_width: expected_shape[1],
            encoding,
        })
    }

    /// Reads one bounded rank-one F32/F16/BF16 tensor as finite F32 values.
    pub fn float_vector(&self, tensor_name: &str, expected_len: usize) -> Result<Vec<f32>> {
        if expected_len == 0 || expected_len > MAX_FLOAT_VECTOR_ELEMENTS {
            return Err(Error::Invalid("SafeTensors float vector exceeds bounds"));
        }
        let info = self
            .metadata
            .info(tensor_name)
            .ok_or(Error::Invalid("SafeTensors tensor is missing"))?;
        if info.shape.as_slice() != [expected_len] {
            return Err(Error::Invalid("SafeTensors float vector shape mismatch"));
        }
        let (encoding, element_bytes) = match info.dtype {
            safetensors::Dtype::F32 => (FloatEncoding::F32, 4_usize),
            safetensors::Dtype::F16 => (FloatEncoding::F16, 2_usize),
            safetensors::Dtype::BF16 => (FloatEncoding::BF16, 2_usize),
            _ => return Err(Error::Invalid("unsupported SafeTensors float dtype")),
        };
        let byte_len = expected_len
            .checked_mul(element_bytes)
            .ok_or(Error::Overflow)?;
        let offset = self
            .data_offset
            .checked_add(u64::try_from(info.data_offsets.0).map_err(|_| Error::Overflow)?)
            .ok_or(Error::Overflow)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(byte_len)
            .map_err(|_| Error::Capacity("SafeTensors float vector bytes"))?;
        bytes.resize(byte_len, 0);
        self.file.read_exact_at(&mut bytes, offset)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(expected_len)
            .map_err(|_| Error::Capacity("SafeTensors float vector"))?;
        for chunk in bytes.chunks_exact(element_bytes) {
            let value = match encoding {
                FloatEncoding::F32 => f32::from_le_bytes(
                    chunk
                        .try_into()
                        .map_err(|_| Error::Invalid("invalid SafeTensors F32 vector"))?,
                ),
                FloatEncoding::F16 => f16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])),
                FloatEncoding::BF16 => {
                    f32::from_bits(u32::from(u16::from_le_bytes([chunk[0], chunk[1]])) << 16)
                }
            };
            if !value.is_finite() {
                return Err(Error::Invalid("SafeTensors float vector is non-finite"));
            }
            values.push(value);
        }
        Ok(values)
    }

    /// Reads one small, rank-one I64 tensor without converting through float.
    /// This is used for PLE hash constants whose values exceed F32's exact range.
    pub fn i64_vector(&self, tensor_name: &str, expected_len: usize) -> Result<Vec<i64>> {
        let info = self
            .metadata
            .info(tensor_name)
            .ok_or(Error::Invalid("SafeTensors tensor is missing"))?;
        if info.dtype != safetensors::Dtype::I64 || info.shape.as_slice() != [expected_len] {
            return Err(Error::Invalid("SafeTensors I64 vector shape mismatch"));
        }
        let byte_len = expected_len.checked_mul(8).ok_or(Error::Overflow)?;
        let offset = self
            .data_offset
            .checked_add(u64::try_from(info.data_offsets.0).map_err(|_| Error::Overflow)?)
            .ok_or(Error::Overflow)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(byte_len)
            .map_err(|_| Error::Capacity("SafeTensors I64 vector"))?;
        bytes.resize(byte_len, 0);
        self.file.read_exact_at(&mut bytes, offset)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(expected_len)
            .map_err(|_| Error::Capacity("SafeTensors I64 vector"))?;
        for chunk in bytes.chunks_exact(8) {
            values.push(i64::from_le_bytes(
                chunk
                    .try_into()
                    .map_err(|_| Error::Invalid("invalid SafeTensors I64 vector"))?,
            ));
        }
        Ok(values)
    }
}

/// Bounded BF16 rows decoded to finite FP32 values on request.
pub struct Bf16Rows {
    store: RowStore,
    row_width: usize,
}

#[derive(Clone, Copy)]
enum FloatEncoding {
    F32,
    F16,
    BF16,
}

/// Bounded F32/F16/BF16 source rows decoded to finite F32 values on request.
pub struct FloatRows {
    store: RowStore,
    row_width: usize,
    encoding: FloatEncoding,
}

impl FloatRows {
    pub fn row_width(&self) -> usize {
        self.row_width
    }

    pub fn rows(&self) -> u64 {
        self.store.rows
    }

    pub fn stats(&self) -> Stats {
        self.store.stats()
    }

    pub fn read_f32_row(&mut self, row_id: u64, output: &mut [f32]) -> Result<()> {
        if output.len() != self.row_width {
            return Err(Error::Invalid("source float output row width mismatch"));
        }
        let bytes = self.store.read(row_id)?;
        match self.encoding {
            FloatEncoding::F32 => {
                for (value, chunk) in output.iter_mut().zip(bytes.chunks_exact(4)) {
                    *value = f32::from_bits(u32::from_le_bytes(
                        chunk
                            .try_into()
                            .map_err(|_| Error::Invalid("invalid SafeTensors F32 row"))?,
                    ));
                }
            }
            FloatEncoding::F16 => {
                for (value, chunk) in output.iter_mut().zip(bytes.chunks_exact(2)) {
                    let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
                    *value = f16_to_f32(bits);
                }
            }
            FloatEncoding::BF16 => {
                for (value, chunk) in output.iter_mut().zip(bytes.chunks_exact(2)) {
                    let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
                    *value = f32::from_bits(u32::from(bits) << 16);
                }
            }
        }
        if output.iter().any(|value| !value.is_finite()) {
            return Err(Error::Invalid("source float row contains non-finite value"));
        }
        Ok(())
    }
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = u32::from((bits >> 10) & 0x1f);
    let fraction = u32::from(bits & 0x03ff);
    let f32_bits = match exponent {
        0 if fraction == 0 => sign,
        0 => {
            let mut mantissa = fraction;
            let mut shifts = 0_u32;
            while mantissa & 0x0400 == 0 {
                mantissa <<= 1;
                shifts += 1;
            }
            let normalized = mantissa & 0x03ff;
            let exponent = 127_u32 - 14 - shifts;
            sign | (exponent << 23) | (normalized << 13)
        }
        0x1f => sign | 0x7f80_0000 | (fraction << 13),
        _ => sign | ((exponent + 112) << 23) | (fraction << 13),
    };
    f32::from_bits(f32_bits)
}

impl Bf16Rows {
    pub fn row_width(&self) -> usize {
        self.row_width
    }

    pub fn rows(&self) -> u64 {
        self.store.rows
    }

    pub fn read_f32_row(&mut self, row_id: u64, output: &mut [f32]) -> Result<()> {
        if output.len() != self.row_width {
            return Err(Error::Invalid("BF16 output row width mismatch"));
        }
        let bytes = self.store.read(row_id)?;
        for (value, pair) in output.iter_mut().zip(bytes.chunks_exact(2)) {
            let bits = u16::from_le_bytes([pair[0], pair[1]]);
            *value = f32::from_bits(u32::from(bits) << 16);
            if !value.is_finite() {
                return Err(Error::Invalid("BF16 row contains non-finite value"));
            }
        }
        Ok(())
    }

    pub fn stats(&self) -> Stats {
        self.store.stats()
    }
}

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
    pub fn new(
        file: File,
        offset: u64,
        rows: u64,
        row_bytes: usize,
        cache_bytes: usize,
    ) -> Result<Self> {
        if rows == 0 || row_bytes == 0 || row_bytes > 16 * 1024 * 1024 {
            return Err(Error::Invalid("row dimensions outside supported bounds"));
        }
        let extent = rows
            .checked_mul(row_bytes as u64)
            .and_then(|n| n.checked_add(offset))
            .ok_or(Error::Overflow)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || extent > metadata.len() {
            return Err(Error::Invalid("row payload exceeds regular file length"));
        }
        // Capacity bounds retained payload only; account for metadata and caller buffers separately.
        let capacity = (cache_bytes / row_bytes).min(usize::try_from(rows).unwrap_or(usize::MAX));
        let mut entries = HashMap::new();
        entries
            .try_reserve(capacity)
            .map_err(|_| Error::Capacity("row cache metadata"))?;
        Ok(Self {
            file,
            offset,
            rows,
            row_bytes,
            capacity,
            entries,
            clock: 0,
            stats: Stats::default(),
        })
    }

    pub fn cached_bytes(&self) -> usize {
        self.entries.len() * self.row_bytes
    }
    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn read(&mut self, row: u64) -> Result<Vec<u8>> {
        if row >= self.rows {
            return Err(Error::Invalid("row outside table"));
        }
        // Clearing on wrap retains correctness without introducing ambiguous LRU ages.
        if self.clock == u64::MAX {
            self.entries.clear();
            self.clock = 0;
        }
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(&row) {
            self.stats.hits = self.stats.hits.saturating_add(1);
            entry.touched = self.clock;
            let mut copy = Vec::new();
            copy.try_reserve_exact(entry.bytes.len())
                .map_err(|_| Error::Capacity("row output buffer"))?;
            copy.extend_from_slice(&entry.bytes);
            return Ok(copy);
        }
        let offset = self.offset + row * self.row_bytes as u64;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.row_bytes)
            .map_err(|_| Error::Capacity("row output buffer"))?;
        bytes.resize(self.row_bytes, 0);
        self.file.read_exact_at(&mut bytes, offset)?;
        self.stats.misses = self.stats.misses.saturating_add(1);
        self.stats.bytes_read = self.stats.bytes_read.saturating_add(self.row_bytes as u64);
        if self.capacity > 0 {
            let mut cached_bytes = Vec::new();
            if cached_bytes.try_reserve_exact(self.row_bytes).is_ok() {
                cached_bytes.extend_from_slice(&bytes);
                if self.entries.len() == self.capacity {
                    if let Some(oldest) = self
                        .entries
                        .iter()
                        .min_by_key(|(_, entry)| entry.touched)
                        .map(|(&key, _)| key)
                    {
                        self.entries.remove(&oldest);
                        self.stats.evictions = self.stats.evictions.saturating_add(1);
                    }
                }
                self.entries.insert(
                    row,
                    Entry {
                        bytes: cached_bytes,
                        touched: self.clock,
                    },
                );
            }
        }
        Ok(bytes)
    }
}

const MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024;

/// Resolves tensor names to canonical, in-checkpoint SafeTensors shard paths.
pub struct SafeTensorIndex {
    tensor_files: HashMap<String, PathBuf>,
}

impl SafeTensorIndex {
    /// Loads a bounded index and validates every listed shard path against the
    /// canonical checkpoint directory. Tensor payloads remain unopened.
    pub fn open(index_path: &Path, shard_directory: &Path) -> Result<Self> {
        let index_file = File::open(index_path)?;
        let index_metadata = index_file.metadata()?;
        if !index_metadata.is_file() || index_metadata.len() > MAX_INDEX_BYTES {
            return Err(Error::Invalid(
                "SafeTensors index is not a bounded regular file",
            ));
        }
        let index: serde_json::Value = serde_json::from_reader(index_file)
            .map_err(|_| Error::Invalid("invalid SafeTensors index JSON"))?;
        let weight_map = index
            .get("weight_map")
            .and_then(serde_json::Value::as_object)
            .ok_or(Error::Invalid("SafeTensors index has no weight map"))?;
        if weight_map.is_empty() {
            return Err(Error::Invalid("SafeTensors index has an empty weight map"));
        }

        let shard_root = shard_directory.canonicalize()?;
        if !shard_root.is_dir() {
            return Err(Error::Invalid("SafeTensors shard root is not a directory"));
        }
        let mut tensor_files = HashMap::new();
        tensor_files
            .try_reserve(weight_map.len())
            .map_err(|_| Error::Capacity("SafeTensors tensor index"))?;
        for (tensor_name, file_name) in weight_map {
            if tensor_name.is_empty() {
                return Err(Error::Invalid("SafeTensors index has an empty tensor name"));
            }
            let file_name = file_name
                .as_str()
                .ok_or(Error::Invalid("SafeTensors shard path is not a string"))?;
            let relative_path = Path::new(file_name);
            let mut components = relative_path.components();
            if !matches!(components.next(), Some(Component::Normal(_)))
                || components.next().is_some()
            {
                return Err(Error::Invalid("unsafe SafeTensors shard path"));
            }
            let file_path = shard_root.join(relative_path).canonicalize()?;
            if !file_path.starts_with(&shard_root) {
                return Err(Error::Invalid(
                    "SafeTensors shard resolves outside model directory",
                ));
            }
            if !file_path.is_file() {
                return Err(Error::Invalid("SafeTensors shard is not a regular file"));
            }
            tensor_files.insert(tensor_name.clone(), file_path);
        }
        Ok(Self { tensor_files })
    }

    /// Returns a validated canonical shard path for one indexed tensor.
    pub fn file_path(&self, tensor_name: &str) -> Result<&Path> {
        self.tensor_files
            .get(tensor_name)
            .map(PathBuf::as_path)
            .ok_or(Error::Invalid("SafeTensors index is missing the tensor"))
    }

    /// Opens and validates the indexed shard containing one tensor.
    pub fn open_tensor(&self, tensor_name: &str) -> Result<SafeTensorFile> {
        SafeTensorFile::open(File::open(self.file_path(tensor_name)?)?)
    }
}

#[cfg(test)]
mod source_index_tests {
    use super::SafeTensorIndex;
    use crate::Error;
    use std::{
        fs::{self, File},
        io::Write,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> std::io::Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "agentix-qwen-source-index-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_vector(path: &Path, name: &str, value: f32) -> std::io::Result<()> {
        let payload = value.to_le_bytes();
        let mut header =
            format!("{{\"{name}\":{{\"dtype\":\"F32\",\"shape\":[1],\"data_offsets\":[0,4]}}}}")
                .into_bytes();
        while header.len() % 8 != 0 {
            header.push(b' ');
        }
        let header_len = u64::try_from(header.len()).map_err(std::io::Error::other)?;
        let mut file = File::create(path)?;
        file.write_all(&header_len.to_le_bytes())?;
        file.write_all(&header)?;
        file.write_all(&payload)
    }

    fn write_index(path: &Path, shard: &str) -> std::io::Result<()> {
        fs::write(
            path,
            format!("{{\"weight_map\":{{\"vector\":\"{shard}\"}}}}"),
        )
    }

    #[test]
    fn opens_an_indexed_tensor_through_its_canonical_shard(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let shard = directory.0.join("model-00001.safetensors");
        let index_file = directory.0.join("model.safetensors.index.json");
        write_vector(&shard, "vector", 1.25)?;
        write_index(&index_file, "model-00001.safetensors")?;

        let index = SafeTensorIndex::open(&index_file, &directory.0)?;
        assert_eq!(index.file_path("vector")?, shard.canonicalize()?);
        assert!(matches!(index.file_path("missing"), Err(Error::Invalid(_))));
        assert_eq!(
            index.open_tensor("vector")?.float_vector("vector", 1)?,
            [1.25]
        );
        Ok(())
    }

    #[test]
    fn rejects_traversal_and_symlinks_that_escape_the_shard_root(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let outside = TestDirectory::new()?;
        let outside_shard = outside.0.join("outside.safetensors");
        write_vector(&outside_shard, "vector", 1.25)?;
        let index_file = directory.0.join("model.safetensors.index.json");

        write_index(&index_file, "../outside.safetensors")?;
        assert!(SafeTensorIndex::open(&index_file, &directory.0).is_err());

        std::os::unix::fs::symlink(&outside_shard, directory.0.join("linked.safetensors"))?;
        write_index(&index_file, "linked.safetensors")?;
        assert!(SafeTensorIndex::open(&index_file, &directory.0).is_err());
        Ok(())
    }
}
