use agentix_qwen::{config::LayerType, config::QwenTextConfig};
use sha2::{Digest, Sha256};

const PINNED_CONFIG: &[u8] = include_bytes!("fixtures/qwen3_8_flash_next_config.json");

#[test]
fn parses_pinned_qwen_text_geometry() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        format!("{:x}", Sha256::digest(PINNED_CONFIG)),
        "889658f2508e8c61d409b02e70e0d78d8d4452ec65aaafbe129805d213d2e74b"
    );
    let config = QwenTextConfig::parse(PINNED_CONFIG)?;
    assert_eq!(config.hidden_size, 2560);
    assert_eq!(config.layer_types.len(), 48);
    assert_eq!(config.layer_types[0], LayerType::GatedDeltaNet);
    assert_eq!(config.layer_types[3], LayerType::Qsa);
    assert_eq!(
        config.indexer_token_budget / config.indexer_compress_ratio,
        512
    );
    assert_eq!(config.gdn_key_heads, 16);
    assert_eq!(config.gdn_value_heads, 48);
    assert_eq!(config.gdn_conv_kernel_size, 4);
    assert!(config.norm_topk_prob);
    assert_eq!((config.ngram_size - 1) * config.heads_per_ngram, 16);
    assert_eq!(
        config.ple_embedding_dim / ((config.ngram_size - 1) * config.heads_per_ngram),
        160
    );
    assert_eq!(config.eos_token_id, 248_044);
    assert_eq!(config.ple_layer_ids, [2]);
    assert_eq!(config.rope_theta, 10_000_000.0);
    Ok(())
}

#[test]
fn rejects_malformed_and_unsupported_qwen_configurations() -> Result<(), Box<dyn std::error::Error>>
{
    assert!(QwenTextConfig::parse(b"{").is_err());
    let mut config: serde_json::Value = serde_json::from_slice(PINNED_CONFIG)?;
    config["text_config"]["indexer_budget"] = serde_json::json!(1024);
    let changed = serde_json::to_vec(&config)?;
    assert!(QwenTextConfig::parse(&changed).is_err());

    config["text_config"]["indexer_budget"] = serde_json::json!(2048);
    config["text_config"]["layer_types"][3] = serde_json::json!("linear_attention");
    let changed = serde_json::to_vec(&config)?;
    assert!(QwenTextConfig::parse(&changed).is_err());

    config["text_config"]["layer_types"][3] = serde_json::json!("full_attention");
    config["text_config"]["linear_conv_kernel_dim"] = serde_json::json!(8);
    let changed = serde_json::to_vec(&config)?;
    assert!(QwenTextConfig::parse(&changed).is_err());

    config["text_config"]["linear_conv_kernel_dim"] = serde_json::json!(4);
    config["text_config"]["norm_topk_prob"] = serde_json::json!(false);
    let changed = serde_json::to_vec(&config)?;
    assert!(!QwenTextConfig::parse(&changed)?.norm_topk_prob);
    config["text_config"]["norm_topk_prob"] = serde_json::json!("true");
    let changed = serde_json::to_vec(&config)?;
    assert!(QwenTextConfig::parse(&changed).is_err());
    Ok(())
}

#[test]
fn pinned_qwen_config_produces_complete_gguf_geometry_schema(
) -> Result<(), Box<dyn std::error::Error>> {
    use agentix_qwen::schema::{qwen4exp_tensor_geometries, validate_tensor_geometries};

    let config = QwenTextConfig::parse(PINNED_CONFIG)?;
    let expected = qwen4exp_tensor_geometries(&config)?;
    assert_eq!(expected.len(), 1224);
    assert!(expected
        .iter()
        .all(|tensor| tensor.extents.iter().all(|&extent| extent > 0)));
    let report = validate_tensor_geometries(&expected, &expected)?;
    assert!(report.is_valid());
    assert_eq!(report.expected_count, 1224);
    Ok(())
}

#[test]
fn gguf_schema_validation_reports_missing_unexpected_duplicate_and_wrong_shapes(
) -> Result<(), Box<dyn std::error::Error>> {
    use agentix_qwen::schema::{
        qwen4exp_tensor_geometries, validate_tensor_geometries, TensorGeometry,
    };

    let config = QwenTextConfig::parse(PINNED_CONFIG)?;
    let expected = qwen4exp_tensor_geometries(&config)?;
    let mut actual = expected.clone();
    actual.remove(0);
    actual[0].extents[1] += 1;
    actual.push(actual[1].clone());
    actual.push(TensorGeometry {
        name: "unexpected.weight".to_owned(),
        extents: [1, 1, 1, 1],
    });

    let report = validate_tensor_geometries(&expected, &actual)?;
    assert!(!report.is_valid());
    assert_eq!(report.missing.len(), 1);
    assert_eq!(report.unexpected, ["unexpected.weight"]);
    assert_eq!(report.duplicate_names.len(), 1);
    assert_eq!(report.shape_mismatches.len(), 1);
    Ok(())
}

#[test]
fn maps_complete_pinned_text_checkpoint_namespace_to_gguf() -> Result<(), Box<dyn std::error::Error>>
{
    use agentix_qwen::schema::{
        map_qwen4exp_source_tensor, qwen4exp_tensor_geometries, SourceTensorTransform,
    };
    use std::collections::{BTreeSet, HashMap};

    let config = QwenTextConfig::parse(PINNED_CONFIG)?;
    let mut source_names = vec![
        "model.language_model.embed_tokens.weight".to_owned(),
        "lm_head.weight".to_owned(),
        "model.language_model.hyper_connection_mixer.hc_norm.weight".to_owned(),
        "model.language_model.hyper_connection_mixer.input_mix_weight_down.weight".to_owned(),
        "model.language_model.hyper_connection_mixer.input_mix_weight_up.weight".to_owned(),
    ];
    let common = [
        "attn_hyper_connection.block_inject_weight.weight",
        "attn_hyper_connection.hc_norm.weight",
        "attn_hyper_connection.input_mix_weight_down.weight",
        "attn_hyper_connection.input_mix_weight_up.weight",
        "mlp_hyper_connection.block_inject_weight.weight",
        "mlp_hyper_connection.hc_norm.weight",
        "mlp_hyper_connection.input_mix_weight_down.weight",
        "mlp_hyper_connection.input_mix_weight_up.weight",
        "mlp.gate.weight",
        "mlp.experts.gate_up_proj",
        "mlp.experts.down_proj",
        "mlp.shared_expert.down_proj.weight",
        "mlp.shared_expert.gate_proj.weight",
        "mlp.shared_expert.up_proj.weight",
        "mlp.shared_expert_gate.weight",
    ];
    let gated_delta = [
        "linear_attn.A_log",
        "linear_attn.conv1d.weight",
        "linear_attn.dt_bias",
        "linear_attn.in_proj_a.weight",
        "linear_attn.in_proj_b.weight",
        "linear_attn.in_proj_qkv.weight",
        "linear_attn.in_proj_z.weight",
        "linear_attn.norm.weight",
        "linear_attn.out_proj.weight",
    ];
    let qsa = [
        "self_attn.q_proj.weight",
        "self_attn.k_proj.weight",
        "self_attn.v_proj.weight",
        "self_attn.o_proj.weight",
        "self_attn.q_norm.weight",
        "self_attn.k_norm.weight",
        "self_attn.indexer.index_qk_proj.weight",
        "self_attn.indexer.q_layernorm.weight",
        "self_attn.indexer.k_layernorm.weight",
    ];
    for (layer, layer_type) in config.layer_types.iter().enumerate() {
        let fields = match layer_type {
            LayerType::GatedDeltaNet => &gated_delta[..],
            LayerType::Qsa => &qsa[..],
        };
        for suffix in common.iter().chain(fields) {
            source_names.push(format!("model.language_model.layers.{layer}.{suffix}"));
        }
        if config.ple_layer_ids.contains(&(layer + 1)) {
            for suffix in [
                "ple.key_proj.weight",
                "ple.value_proj.weight",
                "ple.norm_conv.weight",
                "ple.norm_key.weight",
                "ple.norm_query.weight",
                "ple.conv1d.weight",
                "ple.ple_embedding.layer_multipliers",
                "ple.ple_embedding.ngram_heads_offsets",
                "ple.ple_embedding.ngram_heads_vocab_sizes",
            ] {
                source_names.push(format!("model.language_model.layers.{layer}.{suffix}"));
            }
            for shard in 0..config.ngram_split_parts {
                source_names.push(format!(
                    "model.language_model.layers.{layer}.ple.ple_embedding.ngram_embedding.shard_{shard}.weight"
                ));
            }
        }
    }

    let mut targets = BTreeSet::new();
    let mut transforms = HashMap::new();
    for source in &source_names {
        let mapping = map_qwen4exp_source_tensor(source, &config)?
            .ok_or_else(|| format!("unmapped pinned text tensor {source}"))?;
        *transforms.entry(mapping.transform).or_insert(0_usize) += 1;
        targets.extend(mapping.gguf_names);
    }
    let expected: BTreeSet<_> = qwen4exp_tensor_geometries(&config)?
        .into_iter()
        .map(|tensor| tensor.name)
        .collect();
    assert_eq!(source_names.len(), 1294);
    assert_eq!(targets, expected);
    assert_eq!(targets.len(), 1224);
    assert_eq!(
        transforms.get(&SourceTensorTransform::ConcatenatePleRowShard),
        Some(&128)
    );
    assert_eq!(
        transforms.get(&SourceTensorTransform::SplitExpertGateUp),
        Some(&48)
    );
    assert_eq!(
        transforms.get(&SourceTensorTransform::SplitIndexerQueryKey),
        Some(&12)
    );
    assert_eq!(
        transforms.get(&SourceTensorTransform::PLEHashMetadata),
        Some(&3)
    );
    assert!(map_qwen4exp_source_tensor("mtp.fc_embedding.weight", &config)?.is_none());
    assert!(map_qwen4exp_source_tensor("model.visual.patch_embed.weight", &config)?.is_none());
    Ok(())
}
