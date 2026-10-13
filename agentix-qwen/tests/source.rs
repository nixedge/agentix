use agentix_qwen::{
    config::QwenTextConfig,
    reference::{
        add_one_to_norm, reorder_gdn_qkv_value_rows, reorder_gdn_value_head_columns,
        reorder_gdn_value_heads_axis0, split_expert_gate_up, split_indexer_qk_rows,
        squeeze_ple_convolution,
    },
    schema::{map_qwen4exp_source_tensor, qwen4exp_tensor_geometries, SourceTensorTransform},
    source::{transform_qwen4exp_f32_tensor, transform_qwen4exp_i64_metadata, PleHashMetadataKind},
    Result,
};
use std::collections::{HashMap, HashSet};

const PINNED_CONFIG: &[u8] = include_bytes!("fixtures/qwen3_8_flash_next_config.json");
const SOURCE_HEADERS: &str = include_str!(
    "../../specs/021-strata-inference-research/evidence/qwen3_8_flash_next_text_tensor_headers.tsv"
);

fn small_config() -> Result<QwenTextConfig> {
    let mut config = QwenTextConfig::parse(PINNED_CONFIG)?;
    config.hidden_size = 4;
    config.indexer_query_heads = 1;
    config.indexer_kv_heads = 1;
    config.indexer_head_dim = 2;
    config.expert_count = 2;
    config.expert_intermediate_size = 2;
    config.gdn_key_heads = 2;
    config.gdn_key_head_dim = 1;
    config.gdn_value_heads = 4;
    config.gdn_value_head_dim = 1;
    config.gdn_conv_kernel_size = 2;
    config.residual_streams = 2;
    config.ple_conv_kernel_size = 2;
    Ok(config)
}

fn values(count: usize) -> Vec<f32> {
    (0..count).map(|value| value as f32).collect()
}

fn normalize_dimensions(
    dimensions: &[usize],
) -> std::result::Result<Vec<u64>, std::num::TryFromIntError> {
    dimensions
        .iter()
        .rev()
        .map(|&dimension| u64::try_from(dimension))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map(|dimensions| {
            dimensions
                .into_iter()
                .filter(|&dimension| dimension != 1)
                .collect()
        })
}

fn geometry_dimensions(dimensions: &[u64; 4]) -> Vec<u64> {
    dimensions
        .iter()
        .copied()
        .filter(|&dimension| dimension != 1)
        .collect()
}

fn source_transform_name(transform: SourceTensorTransform) -> &'static str {
    match transform {
        SourceTensorTransform::Direct => "Direct",
        SourceTensorTransform::SplitIndexerQueryKey => "SplitIndexerQueryKey",
        SourceTensorTransform::SplitExpertGateUp => "SplitExpertGateUp",
        SourceTensorTransform::ConcatenatePleRowShard => "ConcatenatePleRowShard",
        SourceTensorTransform::PLEHashMetadata => "PLEHashMetadata",
        SourceTensorTransform::AddOneToNorm => "AddOneToNorm",
        SourceTensorTransform::SqueezePleConvolution => "SqueezePleConvolution",
        SourceTensorTransform::ReorderGatedDeltaValueHeads => "ReorderGatedDeltaValueHeads",
    }
}

#[test]
fn source_dispatch_wires_split_norm_and_ple_convolution_transforms() -> Result<()> {
    let config = small_config()?;

    let fused_indexer = values(16);
    let indexer = transform_qwen4exp_f32_tensor(
        "model.language_model.layers.0.self_attn.indexer.index_qk_proj.weight",
        &[4, 4],
        &fused_indexer,
        &config,
    )?;
    let (expected_query, expected_key) = split_indexer_qk_rows(&fused_indexer, 2, 2, 4)?;
    assert_eq!(indexer[0].name, "blk.0.indexer.q_proj.weight");
    assert_eq!(indexer[0].dimensions, [2, 4]);
    assert_eq!(indexer[0].values, expected_query);
    assert_eq!(indexer[1].name, "blk.0.indexer.k_proj.weight");
    assert_eq!(indexer[1].dimensions, [2, 4]);
    assert_eq!(indexer[1].values, expected_key);

    let fused_experts = values(32);
    let experts = transform_qwen4exp_f32_tensor(
        "model.language_model.layers.0.mlp.experts.gate_up_proj",
        &[2, 4, 4],
        &fused_experts,
        &config,
    )?;
    let (expected_gate, expected_up) = split_expert_gate_up(&fused_experts, 2, 2, 4)?;
    assert_eq!(experts[0].name, "blk.0.ffn_gate_exps.weight");
    assert_eq!(experts[0].dimensions, [2, 2, 4]);
    assert_eq!(experts[0].values, expected_gate);
    assert_eq!(experts[1].name, "blk.0.ffn_up_exps.weight");
    assert_eq!(experts[1].dimensions, [2, 2, 4]);
    assert_eq!(experts[1].values, expected_up);

    let norm = transform_qwen4exp_f32_tensor(
        "model.language_model.layers.0.self_attn.indexer.q_layernorm.weight",
        &[4],
        &[-1.0, 0.0, 1.0, 2.0],
        &config,
    )?;
    assert_eq!(norm[0].name, "blk.0.indexer.q_norm.weight");
    assert_eq!(norm[0].values, add_one_to_norm(&[-1.0, 0.0, 1.0, 2.0])?);

    let ple_convolution = values(16);
    let ple = transform_qwen4exp_f32_tensor(
        "model.language_model.layers.1.ple.conv1d.weight",
        &[8, 1, 2],
        &ple_convolution,
        &config,
    )?;
    assert_eq!(ple[0].name, "blk.1.ple_conv1d.weight");
    assert_eq!(ple[0].dimensions, [8, 2]);
    assert_eq!(
        ple[0].values,
        squeeze_ple_convolution(&ple_convolution, 8, 1, 2)?
    );
    Ok(())
}

#[test]
fn pinned_source_header_geometry_maps_to_complete_gguf_schema(
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let config = QwenTextConfig::parse(PINNED_CONFIG)?;
    let schema = qwen4exp_tensor_geometries(&config)?;
    let schema = schema
        .into_iter()
        .map(|tensor| (tensor.name, tensor.extents))
        .collect::<HashMap<_, _>>();
    let mut mapped_targets = HashSet::new();
    let mut ple_shards = 0_usize;
    let mut ple_rows = 0_usize;
    let mut source_count = 0_usize;

    for line in SOURCE_HEADERS.lines().skip(1) {
        let fields = line.split('\t').collect::<Vec<_>>();
        assert_eq!(
            fields.len(),
            8,
            "malformed pinned tensor header row: {line}"
        );
        let [source_name, target_names, transform, dtype, shape, _shard, start, end] =
            fields.as_slice()
        else {
            return Err("pinned tensor header row has wrong column count".into());
        };
        let dimensions = shape
            .split('x')
            .map(str::parse::<usize>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert!(!dimensions.is_empty() && dimensions.iter().all(|&dimension| dimension > 0));
        let source_elements = dimensions
            .iter()
            .try_fold(1_usize, |elements, &dimension| {
                elements.checked_mul(dimension)
            })
            .ok_or("source tensor element count overflow")?;
        let element_bytes = match *dtype {
            "BF16" => 2_u64,
            "I64" => 8_u64,
            other => return Err(format!("unexpected source dtype {other}").into()),
        };
        let start = start.parse::<u64>()?;
        let end = end.parse::<u64>()?;
        assert_eq!(
            end.checked_sub(start),
            Some(
                u64::try_from(source_elements)?
                    .checked_mul(element_bytes)
                    .ok_or("source byte size overflow")?
            ),
            "source tensor byte extent mismatch for {source_name}"
        );
        let mapping = map_qwen4exp_source_tensor(source_name, &config)?
            .ok_or_else(|| format!("unmapped pinned source tensor {source_name}"))?;
        assert_eq!(source_transform_name(mapping.transform), *transform);
        assert_eq!(mapping.gguf_names.join(","), *target_names);
        source_count += 1;

        let output_shapes = match mapping.transform {
            SourceTensorTransform::Direct
            | SourceTensorTransform::AddOneToNorm
            | SourceTensorTransform::ReorderGatedDeltaValueHeads => {
                assert_eq!(*dtype, "BF16");
                vec![dimensions.clone()]
            }
            SourceTensorTransform::SqueezePleConvolution => {
                assert_eq!(*dtype, "BF16");
                assert_eq!(dimensions.len(), 3);
                assert_eq!(dimensions[1], 1);
                vec![vec![dimensions[0], dimensions[2]]]
            }
            SourceTensorTransform::SplitIndexerQueryKey => {
                assert_eq!(*dtype, "BF16");
                let query_rows = config
                    .indexer_query_heads
                    .checked_mul(config.indexer_head_dim)
                    .ok_or("indexer query width overflow")?;
                let key_rows = config
                    .indexer_kv_heads
                    .checked_mul(config.indexer_head_dim)
                    .ok_or("indexer key width overflow")?;
                assert_eq!(dimensions, [query_rows + key_rows, config.hidden_size]);
                vec![
                    vec![query_rows, config.hidden_size],
                    vec![key_rows, config.hidden_size],
                ]
            }
            SourceTensorTransform::SplitExpertGateUp => {
                assert_eq!(*dtype, "BF16");
                let combined_width = config
                    .expert_intermediate_size
                    .checked_mul(2)
                    .ok_or("expert gate/up width overflow")?;
                assert_eq!(
                    dimensions,
                    [config.expert_count, combined_width, config.hidden_size]
                );
                let shape = vec![
                    config.expert_count,
                    config.expert_intermediate_size,
                    config.hidden_size,
                ];
                vec![shape.clone(), shape]
            }
            SourceTensorTransform::ConcatenatePleRowShard => {
                assert_eq!(*dtype, "BF16");
                assert_eq!(dimensions.len(), 2);
                assert_eq!(dimensions[1], agentix_qwen::schema::ple_row_width(&config)?);
                ple_shards += 1;
                ple_rows = ple_rows
                    .checked_add(dimensions[0])
                    .ok_or("PLE source row total overflow")?;
                let [target] = mapping.gguf_names.as_slice() else {
                    return Err("PLE shard must have one GGUF target".into());
                };
                mapped_targets.insert(target.clone());
                continue;
            }
            SourceTensorTransform::PLEHashMetadata => {
                assert_eq!(*dtype, "I64");
                assert!(mapping.gguf_names.is_empty());
                let expected_len = if source_name.ends_with("layer_multipliers") {
                    config.ngram_size
                } else {
                    config
                        .ngram_size
                        .checked_sub(1)
                        .and_then(|sizes| sizes.checked_mul(config.heads_per_ngram))
                        .ok_or("PLE metadata length overflow")?
                };
                assert_eq!(dimensions, [expected_len]);
                continue;
            }
        };
        assert_eq!(output_shapes.len(), mapping.gguf_names.len());
        for (target_name, output_shape) in mapping.gguf_names.iter().zip(output_shapes) {
            let expected = schema
                .get(target_name)
                .ok_or_else(|| format!("mapped target missing from schema: {target_name}"))?;
            assert_eq!(
                normalize_dimensions(&output_shape)?,
                geometry_dimensions(expected),
                "source geometry mismatch for {source_name} -> {target_name}"
            );
            mapped_targets.insert(target_name.clone());
        }
    }
    assert_eq!(source_count, 1294);
    assert_eq!(ple_shards, config.ngram_split_parts);
    assert_eq!(ple_rows, agentix_qwen::schema::ple_table_rows(&config)?);
    let ple_target = "per_layer_token_embd.weight";
    assert_eq!(
        geometry_dimensions(
            schema
                .get(ple_target)
                .ok_or("PLE tensor missing from schema")?
        ),
        vec![
            u64::try_from(agentix_qwen::schema::ple_row_width(&config)?)?,
            u64::try_from(ple_rows)?
        ]
    );
    assert_eq!(mapped_targets.len(), schema.len());
    assert!(schema.keys().all(|name| mapped_targets.contains(name)));
    Ok(())
}

#[test]
fn source_dispatch_wires_all_gated_delta_value_reorders() -> Result<()> {
    let config = small_config()?;

    let qkv_values = values(32);
    let qkv = transform_qwen4exp_f32_tensor(
        "model.language_model.layers.0.linear_attn.in_proj_qkv.weight",
        &[8, 4],
        &qkv_values,
        &config,
    )?;
    assert_eq!(qkv[0].name, "blk.0.attn_qkv.weight");
    assert_eq!(
        qkv[0].values,
        reorder_gdn_qkv_value_rows(&qkv_values, 4, 2, 2, 1, 4)?
    );

    let z_values = values(16);
    let z = transform_qwen4exp_f32_tensor(
        "model.language_model.layers.0.linear_attn.in_proj_z.weight",
        &[4, 4],
        &z_values,
        &config,
    )?;
    assert_eq!(z[0].name, "blk.0.attn_gate.weight");
    assert_eq!(
        z[0].values,
        reorder_gdn_value_heads_axis0(&z_values, 2, 2, 1, 4)?
    );

    for name in ["in_proj_a", "in_proj_b"] {
        let projection = transform_qwen4exp_f32_tensor(
            &format!("model.language_model.layers.0.linear_attn.{name}.weight"),
            &[4, 4],
            &z_values,
            &config,
        )?;
        assert_eq!(
            projection[0].values,
            reorder_gdn_value_heads_axis0(&z_values, 2, 2, 1, 4)?
        );
    }

    for name in ["A_log", "dt_bias"] {
        let source_decay = [0.0, 1.0, 2.0, 3.0];
        let decay = transform_qwen4exp_f32_tensor(
            &format!("model.language_model.layers.0.linear_attn.{name}"),
            &[4],
            &source_decay,
            &config,
        )?;
        let expected = if name == "A_log" {
            source_decay.map(|value| -value.exp())
        } else {
            source_decay
        };
        assert_eq!(
            decay[0].values,
            reorder_gdn_value_heads_axis0(&expected, 2, 2, 1, 1)?
        );
    }

    assert!(transform_qwen4exp_f32_tensor(
        "model.language_model.layers.0.linear_attn.A_log",
        &[4],
        &[1_000.0; 4],
        &config,
    )
    .is_err());

    let convolution_values = values(16);
    let convolution = transform_qwen4exp_f32_tensor(
        "model.language_model.layers.0.linear_attn.conv1d.weight",
        &[8, 1, 2],
        &convolution_values,
        &config,
    )?;
    let mut expected = convolution_values[..8].to_vec();
    expected.extend(reorder_gdn_value_heads_axis0(
        &convolution_values[8..],
        2,
        2,
        1,
        2,
    )?);
    assert_eq!(convolution[0].dimensions, [8, 2]);
    assert_eq!(convolution[0].values, expected);

    let output_values = values(16);
    let output = transform_qwen4exp_f32_tensor(
        "model.language_model.layers.0.linear_attn.out_proj.weight",
        &[4, 4],
        &output_values,
        &config,
    )?;
    assert_eq!(output[0].name, "blk.0.ssm_out.weight");
    assert_eq!(
        output[0].values,
        reorder_gdn_value_head_columns(&output_values, 4, 2, 2, 1)?
    );
    Ok(())
}

#[test]
fn source_dispatch_rejects_bad_geometry_and_separate_ple_paths() -> Result<()> {
    let config = small_config()?;
    let direct = transform_qwen4exp_f32_tensor(
        "model.language_model.layers.0.linear_attn.norm.weight",
        &[4],
        &[0.25, 0.5, 0.75, 1.0],
        &config,
    )?;
    assert_eq!(direct[0].name, "blk.0.ssm_norm.weight");
    assert_eq!(direct[0].values, [0.25, 0.5, 0.75, 1.0]);

    assert!(transform_qwen4exp_f32_tensor(
        "model.language_model.layers.0.self_attn.indexer.index_qk_proj.weight",
        &[3, 4],
        &[0.0; 12],
        &config,
    )
    .is_err());
    assert!(transform_qwen4exp_f32_tensor(
        "model.language_model.layers.1.ple.ple_embedding.ngram_embedding.shard_0.weight",
        &[1, 2],
        &[0.0; 2],
        &config,
    )
    .is_err());
    assert!(transform_qwen4exp_f32_tensor(
        "model.language_model.layers.1.ple.ple_embedding.layer_multipliers",
        &[3],
        &[1.0, 2.0, 3.0],
        &config,
    )
    .is_err());
    assert!(transform_qwen4exp_f32_tensor("not.a.qwen.tensor", &[1], &[0.0], &config).is_err());
    assert!(transform_qwen4exp_f32_tensor(
        "model.language_model.layers.0.linear_attn.norm.weight",
        &[1],
        &[f32::NAN],
        &config,
    )
    .is_err());
    Ok(())
}

#[test]
fn source_dispatch_preserves_ple_hash_int64_metadata_exactly() -> Result<()> {
    let config = small_config()?;
    let multipliers = [1_i64, (1_i64 << 45) + 7, (1_i64 << 46) + 9];
    let converted = transform_qwen4exp_i64_metadata(
        "model.language_model.layers.1.ple.ple_embedding.layer_multipliers",
        &[3],
        &multipliers,
        &config,
    )?;
    assert_eq!(converted.kind, PleHashMetadataKind::LayerMultipliers);
    assert_eq!(converted.values, multipliers);

    let offsets: Vec<i64> = (0..16).map(i64::from).collect();
    let converted = transform_qwen4exp_i64_metadata(
        "model.language_model.layers.1.ple.ple_embedding.ngram_heads_offsets",
        &[16],
        &offsets,
        &config,
    )?;
    assert_eq!(converted.kind, PleHashMetadataKind::HeadOffsets);
    assert_eq!(converted.values, offsets);

    let vocab_sizes = vec![20_000_001_i64; 16];
    let converted = transform_qwen4exp_i64_metadata(
        "model.language_model.layers.1.ple.ple_embedding.ngram_heads_vocab_sizes",
        &[16],
        &vocab_sizes,
        &config,
    )?;
    assert_eq!(converted.kind, PleHashMetadataKind::HeadVocabSizes);
    assert_eq!(converted.values, vocab_sizes);

    assert!(transform_qwen4exp_i64_metadata(
        "model.language_model.layers.1.ple.ple_embedding.layer_multipliers",
        &[3],
        &[1, -2, 3],
        &config,
    )
    .is_err());
    assert!(transform_qwen4exp_i64_metadata(
        "model.language_model.layers.1.ple.ple_embedding.layer_multipliers",
        &[2],
        &[1, 2],
        &config,
    )
    .is_err());
    Ok(())
}
