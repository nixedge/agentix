//! PLE embedding-row lookup with transactional n-gram history updates.

use crate::{
    config::QwenTextConfig,
    ngram::Ngram,
    reference::{ple_step, DepthwiseConvState, PleLayerWeights},
    rows::SafeTensorIndex,
    Error, Result,
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

/// Opens the pinned text checkpoint's indexed PLE BF16 shards in numeric order.
/// `cache_bytes` is the aggregate encoded-row cache budget divided among shards.
pub fn open_qwen4exp_ple_rows(
    index_path: &Path,
    shard_directory: &Path,
    config: &QwenTextConfig,
    cache_bytes: usize,
) -> Result<ShardedEmbeddingRows<crate::rows::Bf16Rows>> {
    let shard_count = config.ngram_split_parts;
    if shard_count == 0 {
        return Err(Error::Invalid("Qwen PLE shard count is zero"));
    }
    let total_rows = crate::schema::ple_table_rows(config)?;
    if total_rows % shard_count != 0 {
        return Err(Error::Invalid(
            "Qwen PLE rows do not divide into source shards",
        ));
    }
    let rows_per_shard = total_rows / shard_count;
    let row_width = crate::schema::ple_row_width(config)?;
    let names = (0..shard_count)
        .map(|index| {
            format!(
                "model.language_model.layers.1.ple.ple_embedding.ngram_embedding.shard_{index}.weight"
            )
        })
        .collect::<Vec<_>>();
    open_indexed_bf16_shards(
        index_path,
        shard_directory,
        &names,
        [rows_per_shard, row_width],
        cache_bytes,
    )
}

/// Reads the three exact int64 PLE hash arrays through the checkpoint index.
/// These values are ready for a future GGUF integer-array metadata writer and
/// must not be converted to float tensor values.
pub fn open_qwen4exp_ple_hash_metadata(
    index_path: &Path,
    shard_directory: &Path,
    config: &QwenTextConfig,
) -> Result<Vec<crate::reference::source::ConvertedPleHashMetadata>> {
    use crate::reference::source::{
        transform_qwen4exp_i64_metadata, validate_qwen4exp_ple_hash_metadata,
    };

    let source_index = SafeTensorIndex::open(index_path, shard_directory)?;
    let head_count = config
        .ngram_size
        .checked_sub(1)
        .and_then(|sizes| sizes.checked_mul(config.heads_per_ngram))
        .ok_or(Error::Overflow)?;
    let vectors = [
        (
            "model.language_model.layers.1.ple.ple_embedding.layer_multipliers",
            config.ngram_size,
        ),
        (
            "model.language_model.layers.1.ple.ple_embedding.ngram_heads_offsets",
            head_count,
        ),
        (
            "model.language_model.layers.1.ple.ple_embedding.ngram_heads_vocab_sizes",
            head_count,
        ),
    ];
    let mut output = Vec::new();
    output
        .try_reserve_exact(vectors.len())
        .map_err(|_| Error::Capacity("PLE hash metadata arrays"))?;
    for (tensor_name, expected_len) in vectors {
        let file = source_index.open_tensor(tensor_name)?;
        let values = file.i64_vector(tensor_name, expected_len)?;
        output.push(transform_qwen4exp_i64_metadata(
            tensor_name,
            &[expected_len],
            &values,
            config,
        )?);
    }
    validate_qwen4exp_ple_hash_metadata(&output, config)?;
    Ok(output)
}

fn open_indexed_bf16_shards(
    index_path: &Path,
    shard_directory: &Path,
    tensor_names: &[String],
    expected_shape: [usize; 2],
    cache_bytes: usize,
) -> Result<ShardedEmbeddingRows<crate::rows::Bf16Rows>> {
    if tensor_names.is_empty() || expected_shape.contains(&0) {
        return Err(Error::Invalid("invalid indexed SafeTensors geometry"));
    }
    let source_index = SafeTensorIndex::open(index_path, shard_directory)?;
    let mut opened_files = HashMap::<PathBuf, crate::rows::SafeTensorFile>::new();
    let mut seen_names = HashSet::new();
    let mut shards = Vec::new();
    shards
        .try_reserve_exact(tensor_names.len())
        .map_err(|_| Error::Capacity("indexed PLE shard readers"))?;
    let cache_per_shard = cache_bytes / tensor_names.len();
    for tensor_name in tensor_names {
        if !seen_names.insert(tensor_name.as_str()) {
            return Err(Error::Invalid("duplicate tensor in SafeTensors shard list"));
        }
        let file_path = source_index.file_path(tensor_name)?;
        if !opened_files.contains_key(file_path) {
            opened_files.insert(
                file_path.to_path_buf(),
                source_index.open_tensor(tensor_name)?,
            );
        }
        let file = opened_files
            .get(file_path)
            .ok_or(Error::Invalid("SafeTensors shard was not opened"))?;
        let reader = file.bf16_rows(tensor_name, expected_shape, cache_per_shard)?;
        shards.push((
            u64::try_from(expected_shape[0]).map_err(|_| Error::Overflow)?,
            reader,
        ));
    }
    ShardedEmbeddingRows::new(shards, expected_shape[1])
}

/// Adapts independently verified row storage/decoding to the PLE lookup path.
pub trait EmbeddingRowReader {
    /// Fills one decoded embedding row. Implementations must write every value.
    fn read_row(&mut self, row_id: u64, output: &mut [f32]) -> Result<()>;
}

/// Presents ordered PLE source shards as one logical row table without
/// concatenating their payloads in memory.
pub struct ShardedEmbeddingRows<R> {
    shards: Vec<(u64, u64, R)>,
    row_width: usize,
    total_rows: u64,
}

impl<R> ShardedEmbeddingRows<R> {
    /// `shards` contains `(row_count, reader)` pairs in source concatenation
    /// order. Each reader is addressed with a shard-local row index.
    pub fn new(shards: Vec<(u64, R)>, row_width: usize) -> Result<Self> {
        if shards.is_empty() || row_width == 0 {
            return Err(Error::Invalid("PLE shard table is empty"));
        }
        let mut total_rows = 0_u64;
        let mut indexed = Vec::new();
        indexed
            .try_reserve_exact(shards.len())
            .map_err(|_| Error::Capacity("PLE shard index"))?;
        for (row_count, reader) in shards {
            if row_count == 0 {
                return Err(Error::Invalid("PLE shard has no rows"));
            }
            let end = total_rows.checked_add(row_count).ok_or(Error::Overflow)?;
            indexed.push((total_rows, end, reader));
            total_rows = end;
        }
        Ok(Self {
            shards: indexed,
            row_width,
            total_rows,
        })
    }

    pub fn row_width(&self) -> usize {
        self.row_width
    }

    pub fn total_rows(&self) -> u64 {
        self.total_rows
    }
}

impl<R: EmbeddingRowReader> EmbeddingRowReader for ShardedEmbeddingRows<R> {
    fn read_row(&mut self, row_id: u64, output: &mut [f32]) -> Result<()> {
        if output.len() != self.row_width {
            return Err(Error::Invalid("PLE shard output row width mismatch"));
        }
        if row_id >= self.total_rows {
            return Err(Error::Invalid("PLE row outside sharded table"));
        }
        let shard_index = self.shards.partition_point(|(_, end, _)| *end <= row_id);
        let Some((start, _, reader)) = self.shards.get_mut(shard_index) else {
            return Err(Error::Invalid("PLE row has no source shard"));
        };
        reader.read_row(row_id - *start, output)
    }
}

impl EmbeddingRowReader for crate::rows::Bf16Rows {
    fn read_row(&mut self, row_id: u64, output: &mut [f32]) -> Result<()> {
        self.read_f32_row(row_id, output)
    }
}

/// Hashes one token, reads its n-gram embedding rows in model order, and only
/// advances n-gram history after every row has decoded to finite FP32 values.
pub fn lookup_embedding(
    ngram: &mut Ngram,
    token: u32,
    row_width: usize,
    reader: &mut impl EmbeddingRowReader,
) -> Result<Vec<f32>> {
    if row_width == 0 {
        return Err(Error::Invalid("PLE embedding row width is zero"));
    }
    ngram.push_with(token, |row_ids| load_rows(row_ids, row_width, reader))
}

/// Runs one PLE token end-to-end from n-gram row decoding. N-gram history and
/// convolution state commit together only if row decoding and PLE computation
/// both succeed.
#[allow(clippy::too_many_arguments)]
pub fn step(
    ngram: &mut Ngram,
    token: u32,
    row_width: usize,
    reader: &mut impl EmbeddingRowReader,
    residual_streams: &[f32],
    hidden_size: usize,
    epsilon: f32,
    is_padding: bool,
    conv_state: &mut DepthwiseConvState,
    weights: PleLayerWeights<'_>,
) -> Result<Vec<f32>> {
    if row_width == 0 {
        return Err(Error::Invalid("PLE embedding row width is zero"));
    }
    let lookup_token = if is_padding {
        ngram.eos_token_id()
    } else {
        token
    };
    ngram.push_with(lookup_token, |row_ids| {
        let embedding = load_rows(row_ids, row_width, reader)?;
        ple_step(
            &embedding,
            residual_streams,
            hidden_size,
            epsilon,
            is_padding,
            conv_state,
            weights,
        )
    })
}

/// Row-backed PLE step for large or quantized key/value projection matrices.
/// N-gram history and convolution state advance only after the token succeeds.
#[allow(clippy::too_many_arguments)]
pub fn step_from_rows<K, V>(
    ngram: &mut Ngram,
    token: u32,
    row_width: usize,
    reader: &mut impl EmbeddingRowReader,
    residual_streams: &[f32],
    hidden_size: usize,
    epsilon: f32,
    is_padding: bool,
    conv_state: &mut DepthwiseConvState,
    key_projection: &mut K,
    value_projection: &mut V,
    key_norm_offset: &[f32],
    query_norm_offset: &[f32],
    conv_norm_offset: &[f32],
    conv_weights: &[f32],
) -> Result<Vec<f32>>
where
    K: crate::reference::LinearWeightRows,
    V: crate::reference::LinearWeightRows,
{
    if row_width == 0 {
        return Err(Error::Invalid("PLE embedding row width is zero"));
    }
    let lookup_token = if is_padding {
        ngram.eos_token_id()
    } else {
        token
    };
    ngram.push_with(lookup_token, |row_ids| {
        let embedding = load_rows(row_ids, row_width, reader)?;
        crate::reference::ple_step_from_rows(
            &embedding,
            residual_streams,
            hidden_size,
            epsilon,
            is_padding,
            conv_state,
            key_projection,
            value_projection,
            key_norm_offset,
            query_norm_offset,
            conv_norm_offset,
            conv_weights,
        )
    })
}

pub struct PrefillInput<'a> {
    pub token_ids: &'a [u32],
    /// Token-major `[tokens, streams * hidden_size]` residual activations.
    pub residual_streams: &'a [f32],
    pub streams: usize,
    pub hidden_size: usize,
    pub row_width: usize,
    pub padding_mask: &'a [bool],
}

/// Sequential PLE prefill for one sequence. N-gram and convolution states are
/// staged for the whole chunk and committed only if every token succeeds.
pub fn prefill(
    input: PrefillInput<'_>,
    epsilon: f32,
    ngram: &mut Ngram,
    reader: &mut impl EmbeddingRowReader,
    conv_state: &mut DepthwiseConvState,
    weights: PleLayerWeights<'_>,
) -> Result<Vec<f32>> {
    let PrefillInput {
        token_ids,
        residual_streams,
        streams,
        hidden_size,
        row_width,
        padding_mask,
    } = input;
    let residual_width = streams.checked_mul(hidden_size).ok_or(Error::Overflow)?;
    let expected_activations = token_ids
        .len()
        .checked_mul(residual_width)
        .ok_or(Error::Overflow)?;
    if streams == 0
        || hidden_size == 0
        || row_width == 0
        || padding_mask.len() != token_ids.len()
        || residual_streams.len() != expected_activations
        || conv_state.channels() != residual_width
    {
        return Err(Error::Invalid("PLE prefill input geometry mismatch"));
    }
    let mut staged_ngram = ngram.try_clone_state()?;
    let mut staged_convolution = conv_state.try_clone_state()?;
    let mut outputs = Vec::new();
    outputs
        .try_reserve_exact(expected_activations)
        .map_err(|_| Error::Capacity("PLE prefill output"))?;
    for (index, (&token_id, &is_padding)) in token_ids.iter().zip(padding_mask).enumerate() {
        let start = index.checked_mul(residual_width).ok_or(Error::Overflow)?;
        let end = start.checked_add(residual_width).ok_or(Error::Overflow)?;
        outputs.extend(step(
            &mut staged_ngram,
            token_id,
            row_width,
            reader,
            &residual_streams[start..end],
            hidden_size,
            epsilon,
            is_padding,
            &mut staged_convolution,
            weights,
        )?);
    }
    *ngram = staged_ngram;
    *conv_state = staged_convolution;
    Ok(outputs)
}

fn load_rows(
    row_ids: &[u64],
    row_width: usize,
    reader: &mut impl EmbeddingRowReader,
) -> Result<Vec<f32>> {
    let output_len = row_ids
        .len()
        .checked_mul(row_width)
        .ok_or(Error::Overflow)?;
    let mut embedding = Vec::new();
    embedding
        .try_reserve_exact(output_len)
        .map_err(|_| Error::Capacity("PLE embedding output"))?;
    let mut row = Vec::new();
    row.try_reserve_exact(row_width)
        .map_err(|_| Error::Capacity("PLE embedding row"))?;
    row.resize(row_width, 0.0);
    for &row_id in row_ids {
        reader.read_row(row_id, &mut row)?;
        if row.iter().any(|value| !value.is_finite()) {
            return Err(Error::Invalid(
                "PLE embedding row contains non-finite values",
            ));
        }
        embedding.extend_from_slice(&row);
    }
    Ok(embedding)
}
#[cfg(test)]
mod indexed_source_tests {
    use super::*;
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> std::io::Result<Self> {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            let path = std::env::temp_dir().join(format!("agentix-qwen-ple-index-{nonce}"));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_bf16_tensor(path: &Path, name: &str, values: [u16; 4]) -> std::io::Result<()> {
        write_bf16_matrix(path, name, [2, 2], &values)
    }

    fn write_bf16_matrix(
        path: &Path,
        name: &str,
        shape: [usize; 2],
        values: &[u16],
    ) -> std::io::Result<()> {
        let data_len = values.len() * 2;
        let mut header = format!(
            "{{\"{name}\":{{\"dtype\":\"BF16\",\"shape\":[{},{}],\"data_offsets\":[0,{data_len}]}}}}",
            shape[0], shape[1]
        )
        .into_bytes();
        while header.len() % 8 != 0 {
            header.push(b' ');
        }
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(&(header.len() as u64).to_le_bytes())?;
        file.write_all(&header)?;
        for value in values {
            file.write_all(&value.to_le_bytes())?;
        }
        Ok(())
    }

    fn write_i64_vector(path: &Path, name: &str, values: &[i64]) -> std::io::Result<()> {
        let data_len = values.len() * 8;
        let mut header = format!(
            "{{\"{name}\":{{\"dtype\":\"I64\",\"shape\":[{}],\"data_offsets\":[0,{data_len}]}}}}",
            values.len()
        )
        .into_bytes();
        while header.len() % 8 != 0 {
            header.push(b' ');
        }
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(&(header.len() as u64).to_le_bytes())?;
        file.write_all(&header)?;
        for value in values {
            file.write_all(&value.to_le_bytes())?;
        }
        Ok(())
    }

    fn write_index(path: &Path, entries: &[(&str, &str)]) -> std::io::Result<()> {
        let weight_map: HashMap<_, _> = entries.iter().copied().collect();
        let bytes = serde_json::to_vec(&serde_json::json!({"weight_map": weight_map}))
            .map_err(std::io::Error::other)?;
        fs::write(path, bytes)
    }

    #[test]
    fn indexed_bf16_shards_open_in_requested_order_and_compose_rows() -> Result<()> {
        let directory = TempDir::new()?;
        write_bf16_tensor(
            &directory.0.join("first.safetensors"),
            "first",
            [0x3f80, 0xc000, 0x3f00, 0x4000],
        )?;
        write_bf16_tensor(
            &directory.0.join("second.safetensors"),
            "second",
            [0x4040, 0x4080, 0x40a0, 0x40c0],
        )?;
        let index_path = directory.0.join("model.safetensors.index.json");
        write_index(
            &index_path,
            &[
                ("first", "first.safetensors"),
                ("second", "second.safetensors"),
            ],
        )?;

        let mut rows = open_indexed_bf16_shards(
            &index_path,
            &directory.0,
            &["first".to_owned(), "second".to_owned()],
            [2, 2],
            16,
        )?;
        assert_eq!(rows.total_rows(), 4);
        let mut output = [0.0_f32; 2];
        rows.read_row(0, &mut output)?;
        assert_eq!(output, [1.0, -2.0]);
        rows.read_row(2, &mut output)?;
        assert_eq!(output, [3.0, 4.0]);
        rows.read_row(3, &mut output)?;
        assert_eq!(output, [5.0, 6.0]);
        Ok(())
    }

    #[test]
    fn indexed_bf16_shards_reject_bad_names_paths_and_geometry() -> Result<()> {
        let directory = TempDir::new()?;
        write_bf16_tensor(&directory.0.join("one.safetensors"), "present", [0; 4])?;
        let index_path = directory.0.join("model.safetensors.index.json");
        write_index(&index_path, &[("present", "one.safetensors")])?;
        assert!(open_indexed_bf16_shards(
            &index_path,
            &directory.0,
            &["missing".to_owned()],
            [2, 2],
            0,
        )
        .is_err());
        assert!(open_indexed_bf16_shards(
            &index_path,
            &directory.0,
            &["present".to_owned()],
            [2, 3],
            0,
        )
        .is_err());

        write_index(&index_path, &[("present", "../escape.safetensors")])?;
        assert!(open_indexed_bf16_shards(
            &index_path,
            &directory.0,
            &["present".to_owned()],
            [2, 2],
            0,
        )
        .is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let outside = TempDir::new()?;
            let outside_tensor = outside.0.join("outside.safetensors");
            write_bf16_tensor(&outside_tensor, "present", [0; 4])?;
            symlink(&outside_tensor, directory.0.join("linked.safetensors"))?;
            write_index(&index_path, &[("present", "linked.safetensors")])?;
            assert!(open_indexed_bf16_shards(
                &index_path,
                &directory.0,
                &["present".to_owned()],
                [2, 2],
                0,
            )
            .is_err());
        }
        Ok(())
    }

    #[test]
    fn public_qwen_ple_opener_derives_shard_geometry_from_config() -> Result<()> {
        let directory = TempDir::new()?;
        let mut config = QwenTextConfig::parse(include_bytes!(
            "../tests/fixtures/qwen3_8_flash_next_config.json"
        ))?;
        config.vocab_size = 8;
        config.eos_token_id = 7;
        config.ngram_size = 3;
        config.heads_per_ngram = 1;
        config.ngram_vocab_size_base = 2;
        config.ngram_split_parts = 2;
        config.ple_embedding_dim = 4;

        let tensor_prefix =
            "model.language_model.layers.1.ple.ple_embedding.ngram_embedding.shard_";
        let first_name = format!("{tensor_prefix}0.weight");
        let second_name = format!("{tensor_prefix}1.weight");
        write_bf16_matrix(
            &directory.0.join("first.safetensors"),
            &first_name,
            [3, 2],
            &[0x3f80, 0x4000, 0x4040, 0x4080, 0x40a0, 0x40c0],
        )?;
        write_bf16_matrix(
            &directory.0.join("second.safetensors"),
            &second_name,
            [3, 2],
            &[0x4120, 0x4130, 0x4140, 0x4150, 0x4160, 0x4170],
        )?;
        let index_path = directory.0.join("model.safetensors.index.json");
        write_index(
            &index_path,
            &[
                (&first_name, "first.safetensors"),
                (&second_name, "second.safetensors"),
            ],
        )?;

        let mut rows = open_qwen4exp_ple_rows(&index_path, &directory.0, &config, 12)?;
        assert_eq!(rows.total_rows(), 6);
        assert_eq!(rows.row_width(), 2);
        let mut output = [0.0; 2];
        rows.read_row(2, &mut output)?;
        assert_eq!(output, [5.0, 6.0]);
        rows.read_row(3, &mut output)?;
        assert_eq!(output, [10.0, 11.0]);
        Ok(())
    }

    #[test]
    fn public_qwen_ple_metadata_opener_preserves_int64_hash_constants() -> Result<()> {
        let directory = TempDir::new()?;
        let mut config = QwenTextConfig::parse(include_bytes!(
            "../tests/fixtures/qwen3_8_flash_next_config.json"
        ))?;
        config.vocab_size = 8;
        config.eos_token_id = 7;
        config.ngram_size = 3;
        config.heads_per_ngram = 1;
        config.ngram_vocab_size_base = 20_000_000;
        let ngram = crate::ngram::Ngram::new(crate::ngram::Config {
            vocabulary: u32::try_from(config.vocab_size).map_err(|_| Error::Overflow)?,
            eos: config.eos_token_id,
            size: config.ngram_size,
            heads_per_size: config.heads_per_ngram,
            vocabulary_base: u64::try_from(config.ngram_vocab_size_base)
                .map_err(|_| Error::Overflow)?,
            layer_ordinal: 0,
            seed: config.ngram_seed,
        })?;
        let as_i64 = |values: &[u64]| -> Result<Vec<i64>> {
            values
                .iter()
                .map(|value| i64::try_from(*value).map_err(|_| Error::Overflow))
                .collect()
        };
        let multipliers_name = "model.language_model.layers.1.ple.ple_embedding.layer_multipliers";
        let offsets_name = "model.language_model.layers.1.ple.ple_embedding.ngram_heads_offsets";
        let sizes_name = "model.language_model.layers.1.ple.ple_embedding.ngram_heads_vocab_sizes";
        let multipliers = as_i64(ngram.hash_multipliers())?;
        let offsets = as_i64(ngram.head_offsets())?;
        let sizes = as_i64(ngram.head_vocab_sizes())?;
        write_i64_vector(
            &directory.0.join("multipliers.safetensors"),
            multipliers_name,
            &multipliers,
        )?;
        write_i64_vector(
            &directory.0.join("offsets.safetensors"),
            offsets_name,
            &offsets,
        )?;
        write_i64_vector(&directory.0.join("sizes.safetensors"), sizes_name, &sizes)?;
        let index_path = directory.0.join("model.safetensors.index.json");
        write_index(
            &index_path,
            &[
                (multipliers_name, "multipliers.safetensors"),
                (offsets_name, "offsets.safetensors"),
                (sizes_name, "sizes.safetensors"),
            ],
        )?;

        let mut metadata = open_qwen4exp_ple_hash_metadata(&index_path, &directory.0, &config)?;
        assert_eq!(metadata.len(), 3);
        assert_eq!(
            metadata[0].kind,
            crate::reference::source::PleHashMetadataKind::LayerMultipliers
        );
        assert_eq!(metadata[0].values, multipliers);
        assert_eq!(metadata[1].values, offsets);
        assert_eq!(metadata[2].values, sizes);
        metadata[1].values[1] += 1;
        assert!(
            crate::reference::source::validate_qwen4exp_ple_hash_metadata(&metadata, &config)
                .is_err()
        );
        Ok(())
    }
}
