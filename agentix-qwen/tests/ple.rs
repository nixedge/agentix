use agentix_qwen::{
    ngram::{Config, Ngram},
    ple::{
        lookup_embedding, prefill, step, step_from_rows, EmbeddingRowReader, PrefillInput,
        ShardedEmbeddingRows,
    },
    reference::{DepthwiseConvState, LinearWeightRows, PleLayerWeights},
    Error, Result,
};

struct Rows {
    fail: bool,
    ids: Vec<u64>,
}

impl EmbeddingRowReader for Rows {
    fn read_row(&mut self, row_id: u64, output: &mut [f32]) -> Result<()> {
        self.ids.push(row_id);
        if self.fail {
            return Err(Error::Invalid("fixture row read failed"));
        }
        output.fill(1.0);
        Ok(())
    }
}

struct FailsAfter {
    successful_reads: usize,
    reads: usize,
}

impl EmbeddingRowReader for FailsAfter {
    fn read_row(&mut self, _row_id: u64, output: &mut [f32]) -> Result<()> {
        if self.reads >= self.successful_reads {
            return Err(Error::Invalid("late fixture row read failed"));
        }
        self.reads += 1;
        output.fill(1.0);
        Ok(())
    }
}

struct NumberedRows(f32);

impl EmbeddingRowReader for NumberedRows {
    fn read_row(&mut self, row_id: u64, output: &mut [f32]) -> Result<()> {
        output.fill(self.0 + row_id as f32);
        Ok(())
    }
}

struct FailOncePlerows {
    fail_once: bool,
    successful_reads: usize,
    reads: usize,
}

impl EmbeddingRowReader for FailOncePlerows {
    fn read_row(&mut self, _row_id: u64, output: &mut [f32]) -> Result<()> {
        if self.fail_once && self.reads == self.successful_reads {
            self.fail_once = false;
            return Err(Error::Invalid("injected PLE row failure"));
        }
        self.reads += 1;
        output.fill(1.0);
        Ok(())
    }
}

struct TestProjection(Vec<f32>);

impl LinearWeightRows for TestProjection {
    fn output_features(&self) -> u64 {
        1
    }

    fn input_features(&self) -> usize {
        self.0.len()
    }

    fn read_weight_row(&mut self, row: u64, output: &mut [f32]) -> Result<()> {
        if row != 0 || output.len() != self.0.len() {
            return Err(Error::Invalid("invalid PLE projection row request"));
        }
        output.copy_from_slice(&self.0);
        Ok(())
    }
}

#[test]
fn sharded_ple_rows_route_global_ids_across_boundaries() -> Result<()> {
    let mut rows =
        ShardedEmbeddingRows::new(vec![(2, NumberedRows(0.0)), (3, NumberedRows(10.0))], 2)?;
    assert_eq!(rows.total_rows(), 5);
    assert_eq!(rows.row_width(), 2);

    let mut output = [0.0; 2];
    rows.read_row(1, &mut output)?;
    assert_eq!(output, [1.0, 1.0]);
    rows.read_row(2, &mut output)?;
    assert_eq!(output, [10.0, 10.0]);
    rows.read_row(4, &mut output)?;
    assert_eq!(output, [12.0, 12.0]);
    assert!(rows.read_row(5, &mut output).is_err());
    assert!(rows.read_row(0, &mut [0.0]).is_err());
    Ok(())
}

#[test]
fn row_backed_ple_lookup_failure_preserves_ngram_and_convolution_state() -> Result<()> {
    let config = Config {
        vocabulary: 64,
        eos: 63,
        size: 3,
        heads_per_size: 1,
        vocabulary_base: 101,
        layer_ordinal: 0,
        seed: 19,
    };
    let mut ngram = Ngram::new(config.clone())?;
    let mut clean_ngram = Ngram::new(config)?;
    let mut convolution = DepthwiseConvState::new(1, 4, 3)?;
    let mut clean_convolution = DepthwiseConvState::new(1, 4, 3)?;
    let mut rows = FailOncePlerows {
        fail_once: true,
        successful_reads: 1,
        reads: 0,
    };
    let mut clean_rows = Rows {
        fail: false,
        ids: Vec::new(),
    };
    let mut key_rows = TestProjection(vec![1.0, 1.0]);
    let mut value_rows = TestProjection(vec![2.0, 2.0]);
    let mut clean_key_rows = TestProjection(vec![1.0, 1.0]);
    let mut clean_value_rows = TestProjection(vec![2.0, 2.0]);
    let conv_weights = [0.0; 4];
    let before = convolution.clone();
    let snapshot = ngram.snapshot();
    assert!(step_from_rows(
        &mut ngram,
        7,
        1,
        &mut rows,
        &[1.0],
        1,
        1.0,
        false,
        &mut convolution,
        &mut key_rows,
        &mut value_rows,
        &[0.0],
        &[0.0],
        &[0.0],
        &conv_weights,
    )
    .is_err());
    assert_eq!(convolution, before);
    ngram.restore(&snapshot)?;
    let retried = step_from_rows(
        &mut ngram,
        7,
        1,
        &mut rows,
        &[1.0],
        1,
        1.0,
        false,
        &mut convolution,
        &mut key_rows,
        &mut value_rows,
        &[0.0],
        &[0.0],
        &[0.0],
        &conv_weights,
    )?;
    let expected = step_from_rows(
        &mut clean_ngram,
        7,
        1,
        &mut clean_rows,
        &[1.0],
        1,
        1.0,
        false,
        &mut clean_convolution,
        &mut clean_key_rows,
        &mut clean_value_rows,
        &[0.0],
        &[0.0],
        &[0.0],
        &conv_weights,
    )?;
    assert_eq!(retried, expected);
    assert_eq!(convolution, clean_convolution);
    Ok(())
}

#[test]
fn sharded_ple_rows_reject_invalid_shard_geometry() {
    assert!(ShardedEmbeddingRows::<NumberedRows>::new(Vec::new(), 2).is_err());
    assert!(ShardedEmbeddingRows::new(vec![(0, NumberedRows(0.0))], 2).is_err());
    assert!(ShardedEmbeddingRows::new(vec![(1, NumberedRows(0.0))], 0).is_err());
    assert!(ShardedEmbeddingRows::new(
        vec![(u64::MAX, NumberedRows(0.0)), (1, NumberedRows(0.0))],
        2
    )
    .is_err());
}

#[test]
fn pinned_ple_source_shard_geometry_matches_ngram_table() -> Result<()> {
    let ngram = Ngram::new(Config::flash_next())?;
    let source_shards = (0..128).map(|_| (2_500_012, NumberedRows(0.0))).collect();
    let rows = ShardedEmbeddingRows::new(source_shards, 160)?;
    assert_eq!(rows.total_rows(), 320_001_536);
    assert_eq!(ngram.total_rows(), 320_001_446);
    assert_eq!(rows.total_rows() - ngram.total_rows(), 90);
    assert_eq!(rows.row_width(), 160);
    Ok(())
}

#[test]
fn row_lookup_failure_does_not_advance_ngram_history() -> Result<()> {
    let mut actual = Ngram::new(Config::flash_next())?;
    let mut expected = Ngram::new(Config::flash_next())?;
    let mut failing_reader = Rows {
        fail: true,
        ids: Vec::new(),
    };
    assert!(lookup_embedding(&mut actual, 42, 1, &mut failing_reader).is_err());

    let mut actual_reader = Rows {
        fail: false,
        ids: Vec::new(),
    };
    let mut expected_reader = Rows {
        fail: false,
        ids: Vec::new(),
    };
    assert_eq!(
        lookup_embedding(&mut actual, 42, 1, &mut actual_reader)?,
        lookup_embedding(&mut expected, 42, 1, &mut expected_reader)?
    );
    assert_eq!(
        lookup_embedding(&mut actual, 43, 1, &mut actual_reader)?,
        lookup_embedding(&mut expected, 43, 1, &mut expected_reader)?
    );
    Ok(())
}

#[test]
fn ple_step_failure_rolls_back_both_ngram_and_convolution_state() -> Result<()> {
    let mut actual_ngram = Ngram::new(Config::flash_next())?;
    let mut expected_ngram = Ngram::new(Config::flash_next())?;
    let mut actual_conv = DepthwiseConvState::new(1, 4, 3)?;
    let mut expected_conv = DepthwiseConvState::new(1, 4, 3)?;
    let actual_initial_conv = actual_conv.clone();
    let mut actual_reader = Rows {
        fail: false,
        ids: Vec::new(),
    };
    let mut expected_reader = Rows {
        fail: false,
        ids: Vec::new(),
    };
    let invalid_weights = PleLayerWeights {
        key_projection: &[],
        value_projection: &[1.0; 16],
        key_norm_offset: &[0.0],
        query_norm_offset: &[0.0],
        conv_norm_offset: &[0.0],
        conv_weights: &[0.0; 4],
    };
    assert!(step(
        &mut actual_ngram,
        42,
        1,
        &mut actual_reader,
        &[1.0],
        1,
        1e-6,
        false,
        &mut actual_conv,
        invalid_weights,
    )
    .is_err());
    assert_eq!(actual_conv, actual_initial_conv);

    let valid_weights = PleLayerWeights {
        key_projection: &[1.0; 16],
        value_projection: &[1.0; 16],
        key_norm_offset: &[0.0],
        query_norm_offset: &[0.0],
        conv_norm_offset: &[0.0],
        conv_weights: &[0.0; 4],
    };
    let actual = step(
        &mut actual_ngram,
        42,
        1,
        &mut actual_reader,
        &[1.0],
        1,
        1e-6,
        false,
        &mut actual_conv,
        valid_weights,
    )?;
    let expected = step(
        &mut expected_ngram,
        42,
        1,
        &mut expected_reader,
        &[1.0],
        1,
        1e-6,
        false,
        &mut expected_conv,
        valid_weights,
    )?;
    assert_eq!(actual, expected);
    assert_eq!(actual_conv, expected_conv);
    Ok(())
}

#[test]
fn padded_ple_tokens_hash_as_eos_and_preserve_segment_boundaries() -> Result<()> {
    let config = Config::flash_next();
    let eos = config.eos;
    let mut actual_ngram = Ngram::new(config.clone())?;
    let mut expected_ngram = Ngram::new(config)?;
    let mut actual_conv = DepthwiseConvState::new(1, 4, 3)?;
    let mut expected_conv = DepthwiseConvState::new(1, 4, 3)?;
    let mut actual_reader = Rows {
        fail: false,
        ids: Vec::new(),
    };
    let mut expected_reader = Rows {
        fail: false,
        ids: Vec::new(),
    };
    let weights = PleLayerWeights {
        key_projection: &[1.0; 16],
        value_projection: &[1.0; 16],
        key_norm_offset: &[0.0],
        query_norm_offset: &[0.0],
        conv_norm_offset: &[0.0],
        conv_weights: &[0.0; 4],
    };
    let padded = step(
        &mut actual_ngram,
        42,
        1,
        &mut actual_reader,
        &[1.0],
        1,
        1e-6,
        true,
        &mut actual_conv,
        weights,
    )?;
    let eos_padded = step(
        &mut expected_ngram,
        eos,
        1,
        &mut expected_reader,
        &[1.0],
        1,
        1e-6,
        true,
        &mut expected_conv,
        weights,
    )?;
    assert_eq!(padded, eos_padded);
    assert_eq!(actual_reader.ids, expected_reader.ids);

    let actual_next = step(
        &mut actual_ngram,
        43,
        1,
        &mut actual_reader,
        &[1.0],
        1,
        1e-6,
        false,
        &mut actual_conv,
        weights,
    )?;
    let expected_next = step(
        &mut expected_ngram,
        43,
        1,
        &mut expected_reader,
        &[1.0],
        1,
        1e-6,
        false,
        &mut expected_conv,
        weights,
    )?;
    assert_eq!(actual_next, expected_next);
    assert_eq!(actual_reader.ids, expected_reader.ids);
    Ok(())
}

#[test]
fn ple_prefill_matches_steps_and_rolls_back_late_row_failures() -> Result<()> {
    let config = Config::flash_next();
    let tokens = [10, 11, config.eos, 12];
    let padding = [false, false, false, true];
    let activations = [1.0; 4];
    let weights = PleLayerWeights {
        key_projection: &[1.0; 16],
        value_projection: &[1.0; 16],
        key_norm_offset: &[0.0],
        query_norm_offset: &[0.0],
        conv_norm_offset: &[0.0],
        conv_weights: &[0.0, 0.0, 0.5, 0.0],
    };
    let mut prefill_ngram = Ngram::new(config.clone())?;
    let mut prefill_conv = DepthwiseConvState::new(1, 4, 3)?;
    let mut prefill_reader = Rows {
        fail: false,
        ids: Vec::new(),
    };
    let prefilled = prefill(
        PrefillInput {
            token_ids: &tokens,
            residual_streams: &activations,
            streams: 1,
            hidden_size: 1,
            row_width: 1,
            padding_mask: &padding,
        },
        1e-6,
        &mut prefill_ngram,
        &mut prefill_reader,
        &mut prefill_conv,
        weights,
    )?;

    let mut step_ngram = Ngram::new(config.clone())?;
    let mut step_conv = DepthwiseConvState::new(1, 4, 3)?;
    let mut step_reader = Rows {
        fail: false,
        ids: Vec::new(),
    };
    let mut stepped = Vec::new();
    for (index, &token) in tokens.iter().enumerate() {
        stepped.extend(step(
            &mut step_ngram,
            token,
            1,
            &mut step_reader,
            &activations[index..index + 1],
            1,
            1e-6,
            padding[index],
            &mut step_conv,
            weights,
        )?);
    }
    assert_eq!(prefilled, stepped);
    assert_eq!(prefill_conv, step_conv);
    assert_eq!(prefill_reader.ids, step_reader.ids);

    let mut failed_ngram = Ngram::new(config.clone())?;
    let mut failed_conv = DepthwiseConvState::new(1, 4, 3)?;
    let initial_conv = failed_conv.clone();
    let mut late_failure = FailsAfter {
        successful_reads: 19,
        reads: 0,
    };
    assert!(prefill(
        PrefillInput {
            token_ids: &tokens,
            residual_streams: &activations,
            streams: 1,
            hidden_size: 1,
            row_width: 1,
            padding_mask: &padding,
        },
        1e-6,
        &mut failed_ngram,
        &mut late_failure,
        &mut failed_conv,
        weights,
    )
    .is_err());
    assert_eq!(failed_conv, initial_conv);

    let mut expected_ngram = Ngram::new(config)?;
    let mut expected_conv = DepthwiseConvState::new(1, 4, 3)?;
    let mut after_failure_reader = Rows {
        fail: false,
        ids: Vec::new(),
    };
    let mut expected_reader = Rows {
        fail: false,
        ids: Vec::new(),
    };
    let actual_next = step(
        &mut failed_ngram,
        13,
        1,
        &mut after_failure_reader,
        &[1.0],
        1,
        1e-6,
        false,
        &mut failed_conv,
        weights,
    )?;
    let expected_next = step(
        &mut expected_ngram,
        13,
        1,
        &mut expected_reader,
        &[1.0],
        1,
        1e-6,
        false,
        &mut expected_conv,
        weights,
    )?;
    assert_eq!(actual_next, expected_next);
    assert_eq!(failed_conv, expected_conv);
    assert_eq!(after_failure_reader.ids, expected_reader.ids);
    Ok(())
}
