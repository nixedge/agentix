//! Independent CPU reference and storage primitives; not yet a full inference backend.
#![forbid(unsafe_code)]

pub mod budget;
pub mod config;
pub mod identity;
pub mod ngram;
pub mod ple;
pub mod reference;
pub mod rows;
pub mod tokenizer;
pub use reference::source;

#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    Overflow,
    Capacity(&'static str),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "invalid Qwen input: {reason}"),
            Self::Overflow => f.write_str("Qwen size arithmetic overflow"),
            Self::Capacity(tier) => write!(f, "insufficient {tier} capacity"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub mod schema {
    //! Expected Qwen4-Exp GGUF tensor names and geometries.

    use crate::{ngram, Error, Result};
    use std::collections::HashMap;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct TensorGeometry {
        pub name: String,
        /// GGUF extents in ggml order; missing dimensions are one.
        pub extents: [u64; 4],
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct TensorShapeMismatch {
        pub name: String,
        pub expected: [u64; 4],
        pub actual: [u64; 4],
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct TensorSchemaReport {
        pub expected_count: usize,
        pub actual_count: usize,
        pub missing: Vec<String>,
        pub unexpected: Vec<String>,
        pub duplicate_names: Vec<String>,
        pub shape_mismatches: Vec<TensorShapeMismatch>,
    }

    impl TensorSchemaReport {
        pub fn is_valid(&self) -> bool {
            self.missing.is_empty()
                && self.unexpected.is_empty()
                && self.duplicate_names.is_empty()
                && self.shape_mismatches.is_empty()
                && self.expected_count == self.actual_count
        }
    }

    fn geometry(name: String, dimensions: &[usize]) -> Result<TensorGeometry> {
        if dimensions.is_empty() || dimensions.len() > 4 || dimensions.contains(&0) {
            return Err(Error::Invalid("invalid expected GGUF tensor dimensions"));
        }
        let mut extents = [1_u64; 4];
        for (index, dimension) in dimensions.iter().enumerate() {
            extents[index] = u64::try_from(*dimension).map_err(|_| Error::Overflow)?;
        }
        Ok(TensorGeometry { name, extents })
    }

    fn add(
        tensors: &mut Vec<TensorGeometry>,
        name: impl Into<String>,
        dimensions: &[usize],
    ) -> Result<()> {
        tensors.push(geometry(name.into(), dimensions)?);
        Ok(())
    }

    /// Returns the checkpoint's PLE row count after source-shard padding.
    pub fn ple_table_rows(config: &crate::config::QwenTextConfig) -> Result<usize> {
        let ngram = ngram::Ngram::new(ngram::Config {
            vocabulary: u32::try_from(config.vocab_size).map_err(|_| Error::Overflow)?,
            eos: config.eos_token_id,
            size: config.ngram_size,
            heads_per_size: config.heads_per_ngram,
            vocabulary_base: u64::try_from(config.ngram_vocab_size_base)
                .map_err(|_| Error::Overflow)?,
            layer_ordinal: 0,
            seed: config.ngram_seed,
        })?;
        let rows = ngram.total_rows();
        let parts = u64::try_from(config.ngram_split_parts).map_err(|_| Error::Overflow)?;
        if parts == 0 {
            return Err(Error::Invalid("PLE shard count must be nonzero"));
        }
        let padded_rows = rows
            .checked_add(parts - 1)
            .ok_or(Error::Overflow)?
            .checked_div(parts)
            .ok_or(Error::Overflow)?
            .checked_mul(parts)
            .ok_or(Error::Overflow)?;
        usize::try_from(padded_rows).map_err(|_| Error::Overflow)
    }

    /// Returns the width of one source PLE embedding row.
    pub fn ple_row_width(config: &crate::config::QwenTextConfig) -> Result<usize> {
        let heads = config
            .ngram_size
            .checked_sub(1)
            .and_then(|sizes| sizes.checked_mul(config.heads_per_ngram))
            .filter(|&heads| heads > 0)
            .ok_or(Error::Invalid("invalid PLE n-gram head count"))?;
        if config.ple_embedding_dim.checked_rem(heads) != Some(0) {
            return Err(Error::Invalid(
                "PLE embedding width does not divide into heads",
            ));
        }
        Ok(config.ple_embedding_dim / heads)
    }

    /// Builds expected GGUF names and extents from the pinned text configuration.
    ///
    /// The names and dimension order follow the pinned upstream Qwen4-Exp GGUF
    /// loader. This validates the GGUF schema; it does not map Hugging Face source
    /// tensor names or claim a weight conversion implementation.
    pub fn qwen4exp_tensor_geometries(
        config: &crate::config::QwenTextConfig,
    ) -> Result<Vec<TensorGeometry>> {
        let hidden = config.hidden_size;
        let stream_width = hidden
            .checked_mul(config.residual_streams)
            .ok_or(Error::Overflow)?;
        let expert_count = config.expert_count;
        let expert_width = config.expert_intermediate_size;
        let shared_width = config.shared_expert_intermediate_size;
        let value_width = config
            .gdn_value_heads
            .checked_mul(config.gdn_value_head_dim)
            .ok_or(Error::Overflow)?;
        let key_width = config
            .gdn_key_heads
            .checked_mul(config.gdn_key_head_dim)
            .ok_or(Error::Overflow)?;
        let qkv_width = key_width
            .checked_mul(2)
            .and_then(|twice_key| twice_key.checked_add(value_width))
            .ok_or(Error::Overflow)?;
        let attention_query_width = config
            .attention_heads
            .checked_mul(config.attention_head_dim)
            .ok_or(Error::Overflow)?;
        let attention_kv_width = config
            .attention_kv_heads
            .checked_mul(config.attention_head_dim)
            .ok_or(Error::Overflow)?;
        let indexer_query_width = config
            .indexer_query_heads
            .checked_mul(config.indexer_head_dim)
            .ok_or(Error::Overflow)?;

        let recurrent_layers = config
            .layer_types
            .iter()
            .filter(|layer| **layer == crate::config::LayerType::GatedDeltaNet)
            .count();
        let qsa_layers = config.layer_types.len().saturating_sub(recurrent_layers);
        let ple_layers = config.ple_layer_ids.len();
        let estimate = 6_usize
            .checked_add(
                config
                    .layer_types
                    .len()
                    .checked_mul(16)
                    .ok_or(Error::Overflow)?,
            )
            .and_then(|count| count.checked_add(recurrent_layers.checked_mul(9)?))
            .and_then(|count| count.checked_add(qsa_layers.checked_mul(10)?))
            .and_then(|count| count.checked_add(ple_layers.checked_mul(6)?))
            .ok_or(Error::Overflow)?;
        let mut tensors = Vec::new();
        tensors
            .try_reserve_exact(estimate)
            .map_err(|_| Error::Capacity("Qwen tensor schema"))?;

        add(
            &mut tensors,
            "token_embd.weight",
            &[hidden, config.vocab_size],
        )?;
        add(&mut tensors, "output.weight", &[hidden, config.vocab_size])?;
        add(&mut tensors, "output_hc_norm.weight", &[stream_width])?;
        add(
            &mut tensors,
            "output_hc_down.weight",
            &[stream_width, config.residual_rank],
        )?;
        add(
            &mut tensors,
            "output_hc_up.weight",
            &[config.residual_rank, stream_width],
        )?;
        if !config.ple_layer_ids.is_empty() {
            add(
                &mut tensors,
                "per_layer_token_embd.weight",
                &[ple_row_width(config)?, ple_table_rows(config)?],
            )?;
        }

        for (layer_index, layer_type) in config.layer_types.iter().enumerate() {
            let prefix = format!("blk.{layer_index}");
            for (suffix, dimensions) in [
                (
                    "hc_attn_inject.weight",
                    vec![stream_width, config.residual_streams],
                ),
                ("hc_attn_norm.weight", vec![stream_width]),
                (
                    "hc_attn_down.weight",
                    vec![stream_width, config.residual_rank],
                ),
                (
                    "hc_attn_up.weight",
                    vec![config.residual_rank, stream_width],
                ),
                (
                    "hc_ffn_inject.weight",
                    vec![stream_width, config.residual_streams],
                ),
                ("hc_ffn_norm.weight", vec![stream_width]),
                (
                    "hc_ffn_down.weight",
                    vec![stream_width, config.residual_rank],
                ),
                ("hc_ffn_up.weight", vec![config.residual_rank, stream_width]),
                (
                    "ffn_gate_exps.weight",
                    vec![hidden, expert_width, expert_count],
                ),
                (
                    "ffn_up_exps.weight",
                    vec![hidden, expert_width, expert_count],
                ),
                (
                    "ffn_down_exps.weight",
                    vec![expert_width, hidden, expert_count],
                ),
                ("ffn_gate_inp.weight", vec![hidden, expert_count]),
                ("ffn_down_shexp.weight", vec![shared_width, hidden]),
                ("ffn_gate_shexp.weight", vec![hidden, shared_width]),
                ("ffn_up_shexp.weight", vec![hidden, shared_width]),
                ("ffn_gate_inp_shexp.weight", vec![hidden]),
            ] {
                add(&mut tensors, format!("{prefix}.{suffix}"), &dimensions)?;
            }

            match layer_type {
                crate::config::LayerType::GatedDeltaNet => {
                    for (suffix, dimensions) in [
                        ("ssm_a", vec![config.gdn_value_heads]),
                        (
                            "ssm_conv1d.weight",
                            vec![config.ple_conv_kernel_size, qkv_width],
                        ),
                        ("ssm_dt.bias", vec![config.gdn_value_heads]),
                        ("ssm_alpha.weight", vec![hidden, config.gdn_value_heads]),
                        ("ssm_beta.weight", vec![hidden, config.gdn_value_heads]),
                        ("attn_qkv.weight", vec![hidden, qkv_width]),
                        ("attn_gate.weight", vec![hidden, value_width]),
                        ("ssm_norm.weight", vec![config.gdn_value_head_dim]),
                        ("ssm_out.weight", vec![value_width, hidden]),
                    ] {
                        add(&mut tensors, format!("{prefix}.{suffix}"), &dimensions)?;
                    }
                }
                crate::config::LayerType::Qsa => {
                    for (suffix, dimensions) in [
                        ("indexer.q_proj.weight", vec![hidden, indexer_query_width]),
                        (
                            "indexer.k_proj.weight",
                            vec![hidden, config.indexer_head_dim],
                        ),
                        ("indexer.k_norm.weight", vec![config.indexer_head_dim]),
                        ("indexer.q_norm.weight", vec![config.indexer_head_dim]),
                        ("attn_k_norm.weight", vec![config.attention_head_dim]),
                        ("attn_k.weight", vec![hidden, attention_kv_width]),
                        ("attn_output.weight", vec![attention_query_width, hidden]),
                        ("attn_q_norm.weight", vec![config.attention_head_dim]),
                        (
                            "attn_q.weight",
                            vec![
                                hidden,
                                attention_query_width
                                    .checked_mul(2)
                                    .ok_or(Error::Overflow)?,
                            ],
                        ),
                        ("attn_v.weight", vec![hidden, attention_kv_width]),
                    ] {
                        add(&mut tensors, format!("{prefix}.{suffix}"), &dimensions)?;
                    }
                }
            }

            if config.ple_layer_ids.contains(&(layer_index + 1)) {
                for (suffix, dimensions) in [
                    ("ple_key.weight", vec![hidden, stream_width]),
                    ("ple_norm_conv.weight", vec![stream_width]),
                    ("ple_norm_key.weight", vec![stream_width]),
                    ("ple_norm_query.weight", vec![stream_width]),
                    (
                        "ple_conv1d.weight",
                        vec![config.ple_conv_kernel_size, stream_width],
                    ),
                    ("ple_value.weight", vec![hidden, hidden]),
                ] {
                    add(&mut tensors, format!("{prefix}.{suffix}"), &dimensions)?;
                }
            }
        }
        Ok(tensors)
    }

    /// Compares an actual GGUF tensor directory with an expected schema.
    pub fn validate_tensor_geometries(
        expected: &[TensorGeometry],
        actual: &[TensorGeometry],
    ) -> Result<TensorSchemaReport> {
        let mut expected_by_name = HashMap::new();
        expected_by_name
            .try_reserve(expected.len())
            .map_err(|_| Error::Capacity("expected tensor-name map"))?;
        for tensor in expected {
            if expected_by_name
                .insert(tensor.name.as_str(), tensor)
                .is_some()
            {
                return Err(Error::Invalid(
                    "expected GGUF tensor schema has duplicate names",
                ));
            }
        }

        let mut actual_by_name = HashMap::new();
        actual_by_name
            .try_reserve(actual.len())
            .map_err(|_| Error::Capacity("actual tensor-name map"))?;
        let mut duplicate_names = Vec::new();
        let mut unexpected = Vec::new();
        let mut shape_mismatches = Vec::new();
        for tensor in actual {
            if actual_by_name
                .insert(tensor.name.as_str(), tensor)
                .is_some()
            {
                duplicate_names.push(tensor.name.clone());
                continue;
            }
            if let Some(expected_tensor) = expected_by_name.get(tensor.name.as_str()) {
                if tensor.extents != expected_tensor.extents {
                    shape_mismatches.push(TensorShapeMismatch {
                        name: tensor.name.clone(),
                        expected: expected_tensor.extents,
                        actual: tensor.extents,
                    });
                }
            } else {
                unexpected.push(tensor.name.clone());
            }
        }
        let mut missing = Vec::new();
        for tensor in expected {
            if !actual_by_name.contains_key(tensor.name.as_str()) {
                missing.push(tensor.name.clone());
            }
        }
        Ok(TensorSchemaReport {
            expected_count: expected.len(),
            actual_count: actual.len(),
            missing,
            unexpected,
            duplicate_names,
            shape_mismatches,
        })
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum SourceTensorTransform {
        Direct,
        SplitIndexerQueryKey,
        SplitExpertGateUp,
        ConcatenatePleRowShard,
        PLEHashMetadata,
        AddOneToNorm,
        SqueezePleConvolution,
        ReorderGatedDeltaValueHeads,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct SourceTensorMapping {
        pub source_name: String,
        pub gguf_names: Vec<String>,
        pub transform: SourceTensorTransform,
    }

    fn mapped(
        source: &str,
        name: impl Into<String>,
        transform: SourceTensorTransform,
    ) -> SourceTensorMapping {
        SourceTensorMapping {
            source_name: source.to_owned(),
            gguf_names: vec![name.into()],
            transform,
        }
    }

    fn mapped_pair(
        source: &str,
        first: String,
        second: String,
        transform: SourceTensorTransform,
    ) -> SourceTensorMapping {
        SourceTensorMapping {
            source_name: source.to_owned(),
            gguf_names: vec![first, second],
            transform,
        }
    }

    /// Maps one pinned Transformers text tensor name to its GGUF tensor name(s).
    ///
    /// This captures namespace changes and converter-level fan-in/fan-out. It
    /// does not perform the value transforms named by `transform` or map vision
    /// and MTP tensors.
    pub fn map_qwen4exp_source_tensor(
        source_name: &str,
        config: &crate::config::QwenTextConfig,
    ) -> Result<Option<SourceTensorMapping>> {
        use SourceTensorTransform as Transform;

        let filtered_name = source_name
            .strip_prefix("model.language_model.")
            .map(|rest| format!("model.{rest}"));
        let name = filtered_name.as_deref().unwrap_or(source_name);
        let direct_root = match name {
            "lm_head.weight" => Some("output.weight"),
            "model.embed_tokens.weight" => Some("token_embd.weight"),
            "model.hyper_connection_mixer.hc_norm.weight" => Some("output_hc_norm.weight"),
            "model.hyper_connection_mixer.input_mix_weight_down.weight" => {
                Some("output_hc_down.weight")
            }
            "model.hyper_connection_mixer.input_mix_weight_up.weight" => {
                Some("output_hc_up.weight")
            }
            _ => None,
        };
        if let Some(target) = direct_root {
            return Ok(Some(mapped(source_name, target, Transform::Direct)));
        }
        if let Some(index) = name
            .strip_prefix("model.layers.")
            .and_then(|tail| tail.split_once('.'))
        {
            let (layer_text, suffix) = index;
            let layer = layer_text
                .parse::<usize>()
                .map_err(|_| Error::Invalid("invalid Qwen layer index in source tensor name"))?;
            if layer >= config.layer_types.len() {
                return Err(Error::Invalid("Qwen layer index exceeds configured layers"));
            }
            let prefix = format!("blk.{layer}");
            let target = match suffix {
                "attn_hyper_connection.block_inject_weight.weight" => {
                    Some(("hc_attn_inject.weight", Transform::Direct))
                }
                "attn_hyper_connection.hc_norm.weight" => {
                    Some(("hc_attn_norm.weight", Transform::Direct))
                }
                "attn_hyper_connection.input_mix_weight_down.weight" => {
                    Some(("hc_attn_down.weight", Transform::Direct))
                }
                "attn_hyper_connection.input_mix_weight_up.weight" => {
                    Some(("hc_attn_up.weight", Transform::Direct))
                }
                "mlp_hyper_connection.block_inject_weight.weight" => {
                    Some(("hc_ffn_inject.weight", Transform::Direct))
                }
                "mlp_hyper_connection.hc_norm.weight" => {
                    Some(("hc_ffn_norm.weight", Transform::Direct))
                }
                "mlp_hyper_connection.input_mix_weight_down.weight" => {
                    Some(("hc_ffn_down.weight", Transform::Direct))
                }
                "mlp_hyper_connection.input_mix_weight_up.weight" => {
                    Some(("hc_ffn_up.weight", Transform::Direct))
                }
                "mlp.gate.weight" => Some(("ffn_gate_inp.weight", Transform::Direct)),
                "mlp.experts.down_proj" | "mlp.experts.down_proj.weight" => {
                    Some(("ffn_down_exps.weight", Transform::Direct))
                }
                "mlp.shared_expert.down_proj.weight" => {
                    Some(("ffn_down_shexp.weight", Transform::Direct))
                }
                "mlp.shared_expert.gate_proj.weight" => {
                    Some(("ffn_gate_shexp.weight", Transform::Direct))
                }
                "mlp.shared_expert.up_proj.weight" => {
                    Some(("ffn_up_shexp.weight", Transform::Direct))
                }
                "mlp.shared_expert_gate.weight" => {
                    Some(("ffn_gate_inp_shexp.weight", Transform::Direct))
                }
                "linear_attn.A_log" => Some(("ssm_a", Transform::ReorderGatedDeltaValueHeads)),
                "linear_attn.conv1d.weight" => {
                    Some(("ssm_conv1d.weight", Transform::ReorderGatedDeltaValueHeads))
                }
                "linear_attn.dt_bias" => {
                    Some(("ssm_dt.bias", Transform::ReorderGatedDeltaValueHeads))
                }
                "linear_attn.in_proj_a.weight" => {
                    Some(("ssm_alpha.weight", Transform::ReorderGatedDeltaValueHeads))
                }
                "linear_attn.in_proj_b.weight" => {
                    Some(("ssm_beta.weight", Transform::ReorderGatedDeltaValueHeads))
                }
                "linear_attn.in_proj_qkv.weight" => {
                    Some(("attn_qkv.weight", Transform::ReorderGatedDeltaValueHeads))
                }
                "linear_attn.in_proj_z.weight" => {
                    Some(("attn_gate.weight", Transform::ReorderGatedDeltaValueHeads))
                }
                "linear_attn.norm.weight" => Some(("ssm_norm.weight", Transform::Direct)),
                "linear_attn.out_proj.weight" => {
                    Some(("ssm_out.weight", Transform::ReorderGatedDeltaValueHeads))
                }
                "self_attn.q_proj.weight" => Some(("attn_q.weight", Transform::Direct)),
                "self_attn.k_proj.weight" => Some(("attn_k.weight", Transform::Direct)),
                "self_attn.v_proj.weight" => Some(("attn_v.weight", Transform::Direct)),
                "self_attn.o_proj.weight" => Some(("attn_output.weight", Transform::Direct)),
                "self_attn.q_norm.weight" => Some(("attn_q_norm.weight", Transform::Direct)),
                "self_attn.k_norm.weight" => Some(("attn_k_norm.weight", Transform::Direct)),
                "self_attn.indexer.q_layernorm.weight" => {
                    Some(("indexer.q_norm.weight", Transform::AddOneToNorm))
                }
                "self_attn.indexer.k_layernorm.weight" => {
                    Some(("indexer.k_norm.weight", Transform::AddOneToNorm))
                }
                "self_attn.indexer.index_qk_proj.weight" => {
                    let base = format!("{prefix}.indexer");
                    return Ok(Some(mapped_pair(
                        source_name,
                        format!("{base}.q_proj.weight"),
                        format!("{base}.k_proj.weight"),
                        Transform::SplitIndexerQueryKey,
                    )));
                }
                "ple.key_proj.weight" => Some(("ple_key.weight", Transform::Direct)),
                "ple.value_proj.weight" => Some(("ple_value.weight", Transform::Direct)),
                "ple.norm_conv.weight" => Some(("ple_norm_conv.weight", Transform::AddOneToNorm)),
                "ple.norm_key.weight" => Some(("ple_norm_key.weight", Transform::AddOneToNorm)),
                "ple.norm_query.weight" => Some(("ple_norm_query.weight", Transform::AddOneToNorm)),
                "ple.conv1d.weight" => {
                    Some(("ple_conv1d.weight", Transform::SqueezePleConvolution))
                }
                "ple.ple_embedding.layer_multipliers"
                | "ple.ple_embedding.ngram_heads_offsets"
                | "ple.ple_embedding.ngram_heads_vocab_sizes" => {
                    return Ok(Some(SourceTensorMapping {
                        source_name: source_name.to_owned(),
                        gguf_names: Vec::new(),
                        transform: Transform::PLEHashMetadata,
                    }));
                }
                _ if suffix.starts_with("mlp.experts.gate_up_proj") => {
                    return Ok(Some(mapped_pair(
                        source_name,
                        format!("{prefix}.ffn_gate_exps.weight"),
                        format!("{prefix}.ffn_up_exps.weight"),
                        Transform::SplitExpertGateUp,
                    )));
                }
                _ if suffix.starts_with("ple.ple_embedding.ngram_embedding.shard_")
                    && suffix.ends_with(".weight") =>
                {
                    return Ok(Some(mapped(
                        source_name,
                        "per_layer_token_embd.weight",
                        Transform::ConcatenatePleRowShard,
                    )));
                }
                _ => None,
            };
            return Ok(target.map(|(tail, transform)| {
                mapped(source_name, format!("{prefix}.{tail}"), transform)
            }));
        }
        Ok(None)
    }
}
