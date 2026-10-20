use agentix_qwen::{
    budget::{admit, admit_context, Reservations},
    ple::{EmbeddingRowReader, ShardedEmbeddingRows},
    rows::{RowStore, SafeTensorFile},
    Error,
};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> std::io::Result<(Self, File)> {
        Self::with_bytes(&[99, 98, 1, 2, 3, 4, 5, 6])
    }

    fn with_bytes(bytes: &[u8]) -> std::io::Result<(Self, File)> {
        let path = std::env::temp_dir().join(format!(
            "agentix-qwen-{}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let fixture = Self(path);
        file.write_all(bytes)?;
        Ok((fixture, file))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn offset_reads_lru_and_uncached_equivalence() -> TestResult {
    let (_fixture, file) = Fixture::new()?;
    let mut uncached = RowStore::new(file.try_clone()?, 2, 3, 2, 0)?;
    let mut cache = RowStore::new(file, 2, 3, 2, 5)?;
    for row in [0, 1, 0, 2, 1] {
        let expected = vec![(row * 2 + 1) as u8, (row * 2 + 2) as u8];
        assert_eq!(cache.read(row)?, expected);
        assert_eq!(uncached.read(row)?, expected);
        assert!(cache.cached_bytes() <= 4);
        assert_eq!(uncached.cached_bytes(), 0);
    }
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(cache.stats().misses, 4);
    assert_eq!(cache.stats().evictions, 2);
    assert_eq!(cache.stats().bytes_read, 8);
    let stats = cache.stats();
    assert!(cache.read(3).is_err());
    assert_eq!(cache.stats(), stats);
    Ok(())
}

#[test]
fn row_bounds_overflow_and_truncation() -> TestResult {
    let (_fixture, file) = Fixture::new()?;
    assert!(RowStore::new(file.try_clone()?, 2, 4, 2, 0).is_err());
    assert!(matches!(
        RowStore::new(file.try_clone()?, u64::MAX, 2, 2, 0),
        Err(Error::Overflow)
    ));
    assert!(RowStore::new(file.try_clone()?, 0, 0, 1, 0).is_err());
    let mut store = RowStore::new(file.try_clone()?, 2, 3, 2, 0)?;
    file.set_len(3)?;
    assert!(matches!(store.read(0), Err(Error::Io(_))));
    assert_eq!(store.stats().bytes_read, 0);
    Ok(())
}

fn safetensors_fixture(
    dtype: &str,
    shape: [usize; 2],
    payload: &[u8],
) -> Result<(Fixture, File), Box<dyn std::error::Error>> {
    let mut header = format!(
        "{{\"ple\":{{\"dtype\":\"{dtype}\",\"shape\":[{},{}],\"data_offsets\":[0,{}]}}}}",
        shape[0],
        shape[1],
        payload.len()
    )
    .into_bytes();
    while header.len() % 8 != 0 {
        header.push(b' ');
    }
    let header_len = u64::try_from(header.len())?.to_le_bytes();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&header_len);
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(payload);
    Ok(Fixture::with_bytes(&bytes)?)
}

fn safetensors_vector_fixture(
    dtype: &str,
    length: usize,
    payload: &[u8],
) -> Result<(Fixture, File), Box<dyn std::error::Error>> {
    let mut header = format!(
        "{{\"vector\":{{\"dtype\":\"{dtype}\",\"shape\":[{length}],\"data_offsets\":[0,{}]}}}}",
        payload.len()
    )
    .into_bytes();
    while header.len() % 8 != 0 {
        header.push(b' ');
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&u64::try_from(header.len())?.to_le_bytes());
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(payload);
    Ok(Fixture::with_bytes(&bytes)?)
}

#[test]
fn safetensors_bf16_reader_validates_header_and_reads_only_requested_rows() -> TestResult {
    let payload = [0x80, 0x3f, 0x00, 0xc0, 0x00, 0x3f, 0x00, 0x40];
    let (_fixture, file) = safetensors_fixture("BF16", [2, 2], &payload)?;
    let tensors = SafeTensorFile::open(file)?;
    let mut rows = tensors.bf16_rows("ple", [2, 2], 4)?;
    assert_eq!(rows.rows(), 2);
    assert_eq!(rows.row_width(), 2);

    let mut output = [0.0; 2];
    rows.read_f32_row(1, &mut output)?;
    assert_eq!(output, [0.5, 2.0]);
    assert_eq!(rows.stats().bytes_read, 4);
    rows.read_f32_row(1, &mut output)?;
    assert_eq!(rows.stats().hits, 1);
    assert!(rows.read_f32_row(2, &mut output).is_err());
    assert!(rows.read_f32_row(0, &mut [0.0]).is_err());
    assert!(tensors.bf16_rows("absent", [2, 2], 0).is_err());
    assert!(tensors.bf16_rows("ple", [2, 3], 0).is_err());

    let first = tensors.bf16_rows("ple", [2, 2], 0)?;
    let second = tensors.bf16_rows("ple", [2, 2], 0)?;
    let mut sharded = ShardedEmbeddingRows::new(vec![(2, first), (2, second)], 2)?;
    sharded.read_row(2, &mut output)?;
    assert_eq!(output, [1.0, -2.0]);
    Ok(())
}

#[test]
fn safetensors_bf16_reader_rejects_other_types_and_non_finite_values() -> TestResult {
    let (_fixture, file) = safetensors_fixture("F32", [1, 2], &[0; 8])?;
    let tensors = SafeTensorFile::open(file)?;
    assert!(tensors.bf16_rows("ple", [1, 2], 0).is_err());

    let (_fixture, file) = safetensors_fixture("BF16", [1, 2], &[0x80, 0x7f, 0, 0])?;
    let tensors = SafeTensorFile::open(file)?;
    let mut rows = tensors.bf16_rows("ple", [1, 2], 0)?;
    assert!(rows.read_f32_row(0, &mut [0.0; 2]).is_err());

    let (_fixture, truncated) = safetensors_fixture("BF16", [2, 2], &[0; 4])?;
    assert!(SafeTensorFile::open(truncated).is_err());
    Ok(())
}

#[test]
fn safetensors_dense_float_reader_decodes_f32_f16_bf16_in_bounded_rows() -> TestResult {
    let payload = [1.25_f32, -2.5_f32, 0.5_f32, 2.0_f32]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect::<Vec<_>>();
    let (_fixture, file) = safetensors_fixture("F32", [2, 2], &payload)?;
    let tensors = SafeTensorFile::open(file)?;
    let mut rows = tensors.float_rows("ple", [2, 2], 16)?;
    let mut output = [0.0; 2];
    rows.read_f32_row(1, &mut output)?;
    assert_eq!(output, [0.5, 2.0]);
    assert_eq!(rows.stats().bytes_read, 8);
    rows.read_f32_row(1, &mut output)?;
    assert_eq!(rows.stats().hits, 1);

    let payload = [0x3d00_u16, 0xc100, 0x3800, 0x4000]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let (_fixture, file) = safetensors_fixture("F16", [2, 2], &payload)?;
    let tensors = SafeTensorFile::open(file)?;
    let mut rows = tensors.float_rows("ple", [2, 2], 0)?;
    rows.read_f32_row(0, &mut output)?;
    assert_eq!(output, [1.25, -2.5]);
    rows.read_f32_row(1, &mut output)?;
    assert_eq!(output, [0.5, 2.0]);

    let payload = [0x3fa0_u16, 0xc020, 0x3f00, 0x4000]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let (_fixture, file) = safetensors_fixture("BF16", [2, 2], &payload)?;
    let tensors = SafeTensorFile::open(file)?;
    let mut rows = tensors.float_rows("ple", [2, 2], 0)?;
    rows.read_f32_row(0, &mut output)?;
    assert_eq!(output, [1.25, -2.5]);
    rows.read_f32_row(1, &mut output)?;
    assert_eq!(output, [0.5, 2.0]);
    assert!(tensors.float_rows("ple", [2, 3], 0).is_err());
    Ok(())
}

#[test]
fn safetensors_dense_float_reader_rejects_integer_and_non_finite_rows() -> TestResult {
    let (_fixture, file) = safetensors_fixture("I32", [1, 2], &[0; 8])?;
    let tensors = SafeTensorFile::open(file)?;
    assert!(tensors.float_rows("ple", [1, 2], 0).is_err());

    let (_fixture, file) = safetensors_fixture("F16", [1, 2], &[0x00, 0x7c, 0, 0])?;
    let tensors = SafeTensorFile::open(file)?;
    let mut rows = tensors.float_rows("ple", [1, 2], 0)?;
    assert!(rows.read_f32_row(0, &mut [0.0; 2]).is_err());

    let (_fixture, file) = safetensors_fixture("F32", [1, 2], &[0; 8])?;
    let tensors = SafeTensorFile::open(file)?;
    assert!(tensors.float_rows("ple", [0, 2], 0).is_err());
    Ok(())
}

#[test]
fn safetensors_float_vector_reads_f32_f16_bf16_and_rejects_invalid_inputs() -> TestResult {
    let payload = [1.25_f32, -2.5_f32]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect::<Vec<_>>();
    let (_fixture, file) = safetensors_vector_fixture("F32", 2, &payload)?;
    let tensors = SafeTensorFile::open(file)?;
    assert_eq!(tensors.float_vector("vector", 2)?, [1.25, -2.5]);
    assert!(tensors.float_vector("vector", 1).is_err());
    assert!(tensors.float_vector("vector", 0).is_err());
    assert!(tensors.float_vector("vector", 1_000_001).is_err());

    let payload = [0x3d00_u16, 0xc100]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let (_fixture, file) = safetensors_vector_fixture("F16", 2, &payload)?;
    let tensors = SafeTensorFile::open(file)?;
    assert_eq!(tensors.float_vector("vector", 2)?, [1.25, -2.5]);

    let payload = [0x3fa0_u16, 0xc020]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let (_fixture, file) = safetensors_vector_fixture("BF16", 2, &payload)?;
    let tensors = SafeTensorFile::open(file)?;
    assert_eq!(tensors.float_vector("vector", 2)?, [1.25, -2.5]);

    let (_fixture, file) = safetensors_vector_fixture("I64", 2, &[0; 16])?;
    let tensors = SafeTensorFile::open(file)?;
    assert!(tensors.float_vector("vector", 2).is_err());

    let (_fixture, file) = safetensors_vector_fixture("F16", 2, &[0x00, 0x7c, 0, 0])?;
    let tensors = SafeTensorFile::open(file)?;
    assert!(tensors.float_vector("vector", 2).is_err());
    Ok(())
}

#[test]
fn safetensors_i64_vector_preserves_large_hash_constants() -> TestResult {
    let values = [1_i64, (1_i64 << 45) + 123, i64::MAX - 7];
    let payload: Vec<u8> = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let mut header = format!(
        "{{\"hashes\":{{\"dtype\":\"I64\",\"shape\":[3],\"data_offsets\":[0,{}]}}}}",
        payload.len()
    )
    .into_bytes();
    while header.len() % 8 != 0 {
        header.push(b' ');
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&u64::try_from(header.len())?.to_le_bytes());
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&payload);
    let (_fixture, file) = Fixture::with_bytes(&bytes)?;
    let tensors = SafeTensorFile::open(file)?;
    assert_eq!(tensors.i64_vector("hashes", 3)?, values);
    assert!(tensors.i64_vector("hashes", 2).is_err());
    Ok(())
}

#[test]
fn independent_memory_tiers_and_checked_totals() -> TestResult {
    let host = Reservations {
        weights: 70,
        context: 10,
        scratch: 5,
        staging: 2,
        lookup_cache: 3,
        headroom: 10,
    };
    let device = Reservations {
        weights: 10,
        context: 4,
        ..Reservations::default()
    };
    let admitted = admit(100, 24, host, device)?;
    assert_eq!(admitted.host_remaining, 0);
    assert_eq!(admitted.device_remaining, 10);
    assert_eq!(admitted.expert_slots(3, 99)?, 3);
    assert_eq!(admitted.expert_slots(3, 2)?, 2);
    assert!(admitted.expert_slots(0, 1).is_err());
    assert!(matches!(
        admit(99, 999, host, device),
        Err(Error::Capacity("host RAM"))
    ));
    assert!(matches!(
        admit(999, 13, host, device),
        Err(Error::Capacity("device VRAM"))
    ));
    assert!(matches!(
        Reservations {
            weights: u64::MAX,
            context: 1,
            ..Reservations::default()
        }
        .total(),
        Err(Error::Overflow)
    ));
    Ok(())
}

#[test]
fn context_admission_preserves_requested_output_and_never_truncates() -> TestResult {
    assert_eq!(
        admit_context(3072, 1024, 4096)?,
        agentix_qwen::budget::ContextAdmission {
            prompt_tokens: 3072,
            requested_output_tokens: 1024,
            total_tokens: 4096,
            remaining_tokens: 0,
        }
    );
    for (prompt_tokens, requested_output_tokens) in
        [(3073, 1024), (3072, 1025), (4097, 1), (1, 4097)]
    {
        assert!(matches!(
            admit_context(prompt_tokens, requested_output_tokens, 4096),
            Err(Error::Capacity("context window"))
        ));
    }
    assert!(matches!(admit_context(1, 0, 4096), Err(Error::Invalid(_))));
    assert_eq!(
        admit_context(usize::MAX - 1, 1, usize::MAX)?,
        agentix_qwen::budget::ContextAdmission {
            prompt_tokens: usize::MAX - 1,
            requested_output_tokens: 1,
            total_tokens: usize::MAX,
            remaining_tokens: 0,
        }
    );
    for (prompt_tokens, requested_output_tokens) in
        [(usize::MAX, 1), (1, usize::MAX), (usize::MAX, usize::MAX)]
    {
        assert!(matches!(
            admit_context(prompt_tokens, requested_output_tokens, usize::MAX),
            Err(Error::Overflow)
        ));
    }
    Ok(())
}
