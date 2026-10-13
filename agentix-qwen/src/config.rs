//! Fail-closed parsing for the pinned text-only execution geometry.

use crate::{Error, Result};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerType {
    GatedDeltaNet,
    Qsa,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QwenTextConfig {
    pub hidden_size: usize,
    pub layer_types: Vec<LayerType>,
    pub attention_heads: usize,
    pub attention_kv_heads: usize,
    pub attention_head_dim: usize,
    pub indexer_query_heads: usize,
    pub indexer_kv_heads: usize,
    pub indexer_head_dim: usize,
    pub indexer_token_budget: usize,
    pub indexer_compress_ratio: usize,
    pub gdn_key_heads: usize,
    pub gdn_key_head_dim: usize,
    pub gdn_value_heads: usize,
    pub gdn_value_head_dim: usize,
    pub gdn_conv_kernel_size: usize,
    pub residual_streams: usize,
    pub residual_rank: usize,
    pub expert_count: usize,
    pub experts_per_token: usize,
    pub norm_topk_prob: bool,
    pub expert_intermediate_size: usize,
    pub shared_expert_intermediate_size: usize,
    pub ngram_size: usize,
    pub heads_per_ngram: usize,
    pub ngram_vocab_size_base: usize,
    pub ngram_split_parts: usize,
    pub ngram_vocab_divisor: usize,
    pub ngram_seed: u64,
    pub ple_embedding_dim: usize,
    pub ple_conv_kernel_size: usize,
    pub eos_token_id: u32,
    pub vocab_size: usize,
    pub max_position_embeddings: usize,
    pub rope_theta: f32,
    pub partial_rotary_factor: f32,
    pub rms_norm_epsilon: f32,
    pub ple_layer_ids: Vec<usize>,
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value
        .as_object()
        .and_then(|object| object.get(key))
        .ok_or(Error::Invalid("Qwen config is missing a required field"))
}

fn object<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    let value = field(value, key)?;
    value
        .is_object()
        .then_some(value)
        .ok_or(Error::Invalid("Qwen config field must be an object"))
}

fn usize_value(value: &Value) -> Result<usize> {
    let integer = value
        .as_u64()
        .ok_or(Error::Invalid("Qwen config integer field is invalid"))?;
    usize::try_from(integer)
        .map_err(|_| Error::Invalid("Qwen config integer exceeds address space"))
}

fn usize_field(value: &Value, key: &str) -> Result<usize> {
    usize_value(field(value, key)?)
}

fn float_field(value: &Value, key: &str) -> Result<f32> {
    let number = field(value, key)?
        .as_f64()
        .ok_or(Error::Invalid("Qwen config float field is invalid"))? as f32;
    number
        .is_finite()
        .then_some(number)
        .ok_or(Error::Invalid("Qwen config float field is not finite"))
}

fn string_field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    field(value, key)?
        .as_str()
        .ok_or(Error::Invalid("Qwen config string field is invalid"))
}

fn expect_string(value: &Value, key: &str, expected: &str) -> Result<()> {
    (string_field(value, key)? == expected)
        .then_some(())
        .ok_or(Error::Invalid("unsupported Qwen text configuration"))
}

fn expect_usize(value: &Value, key: &str, expected: usize) -> Result<()> {
    (usize_field(value, key)? == expected)
        .then_some(())
        .ok_or(Error::Invalid("unsupported Qwen text geometry"))
}

fn expect_float(value: &Value, key: &str, expected: f32) -> Result<f32> {
    let actual = float_field(value, key)?;
    (actual == expected).then_some(actual).ok_or(Error::Invalid(
        "unsupported Qwen positional or norm configuration",
    ))
}

fn usize_array(value: &Value, key: &str) -> Result<Vec<usize>> {
    let array = field(value, key)?
        .as_array()
        .ok_or(Error::Invalid("Qwen config array field is invalid"))?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(array.len())
        .map_err(|_| Error::Capacity("Qwen config metadata"))?;
    for entry in array {
        values.push(usize_value(entry)?);
    }
    Ok(values)
}

fn layer_types(text: &Value) -> Result<Vec<LayerType>> {
    let array = field(text, "layer_types")?
        .as_array()
        .ok_or(Error::Invalid("Qwen layer types must be an array"))?;
    if array.len() != 48 {
        return Err(Error::Invalid("unsupported Qwen layer count"));
    }
    let mut layers = Vec::new();
    layers
        .try_reserve_exact(array.len())
        .map_err(|_| Error::Capacity("Qwen layer metadata"))?;
    for (index, layer) in array.iter().enumerate() {
        let (kind, name) = if index % 4 == 3 {
            (LayerType::Qsa, "full_attention")
        } else {
            (LayerType::GatedDeltaNet, "linear_attention")
        };
        if layer.as_str() != Some(name) {
            return Err(Error::Invalid("unsupported Qwen layer ordering"));
        }
        layers.push(kind);
    }
    Ok(layers)
}

impl QwenTextConfig {
    /// Parses the official top-level conditional-generation config and validates
    /// the nested text architecture used by the pinned checkpoint.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let root: Value = serde_json::from_slice(bytes)
            .map_err(|_| Error::Invalid("Qwen config JSON is invalid"))?;
        expect_string(&root, "model_type", "qwen4_exp")?;
        let architectures = field(&root, "architectures")?
            .as_array()
            .ok_or(Error::Invalid("Qwen architectures field is invalid"))?;
        if architectures.len() != 1
            || architectures[0].as_str() != Some("Qwen4ExpForConditionalGeneration")
        {
            return Err(Error::Invalid("unsupported Qwen model architecture"));
        }
        let text = object(&root, "text_config")?;
        expect_string(text, "model_type", "qwen4_exp_text")?;
        expect_string(text, "hidden_act", "silu")?;
        expect_string(text, "output_gate_type", "sigmoid")?;
        if field(text, "attention_bias")?
            .as_bool()
            .ok_or(Error::Invalid("Qwen attention_bias must be boolean"))?
        {
            return Err(Error::Invalid("Qwen attention bias is unsupported"));
        }

        for (key, expected) in [
            ("hidden_size", 2560),
            ("num_hidden_layers", 48),
            ("full_attention_interval", 4),
            ("head_dim", 256),
            ("num_attention_heads", 24),
            ("num_key_value_heads", 2),
            ("indexer_n_heads", 4),
            ("indexer_kv_heads", 1),
            ("indexer_head_dim", 128),
            ("indexer_budget", 2048),
            ("indexer_compress_ratio", 4),
            ("linear_num_key_heads", 16),
            ("linear_key_head_dim", 128),
            ("linear_num_value_heads", 48),
            ("linear_value_head_dim", 128),
            ("linear_conv_kernel_dim", 4),
            ("hc_count", 4),
            ("hc_lowrank", 320),
            ("num_experts", 512),
            ("num_experts_per_tok", 10),
            ("moe_intermediate_size", 640),
            ("shared_expert_intermediate_size", 640),
            ("ngram_size", 3),
            ("heads_per_ngram", 8),
            ("ngram_vocab_size_base", 20_000_000),
            ("split_ngram_parts", 128),
            ("make_ngram_vocab_size_divisible_by", 128),
            ("ple_embed_dim", 2560),
            ("ple_conv_kernel_size", 4),
            ("eos_token_id", 248_044),
            ("vocab_size", 248_320),
            ("max_position_embeddings", 262_144),
        ] {
            expect_usize(text, key, expected)?;
        }
        let partial_rotary_factor = expect_float(text, "partial_rotary_factor", 0.25)?;
        let rms_norm_epsilon = expect_float(text, "rms_norm_eps", 1e-6)?;
        let norm_topk_prob = match text.get("norm_topk_prob") {
            Some(value) => value
                .as_bool()
                .ok_or(Error::Invalid("Qwen norm_topk_prob must be boolean"))?,
            None => true,
        };
        let rope = object(text, "rope_parameters")?;
        expect_string(rope, "rope_type", "default")?;
        let rope_theta = expect_float(rope, "rope_theta", 10_000_000.0)?;
        expect_float(rope, "partial_rotary_factor", partial_rotary_factor)?;
        if usize_array(rope, "mrope_section")? != [11, 11, 10]
            || usize_array(text, "ple_layer_ids")? != [2]
        {
            return Err(Error::Invalid("unsupported Qwen PLE or MRoPE geometry"));
        }

        Ok(Self {
            hidden_size: 2560,
            layer_types: layer_types(text)?,
            attention_heads: 24,
            attention_kv_heads: 2,
            attention_head_dim: 256,
            indexer_query_heads: 4,
            indexer_kv_heads: 1,
            indexer_head_dim: 128,
            indexer_token_budget: 2048,
            indexer_compress_ratio: 4,
            gdn_key_heads: 16,
            gdn_key_head_dim: 128,
            gdn_value_heads: 48,
            gdn_value_head_dim: 128,
            gdn_conv_kernel_size: 4,
            residual_streams: 4,
            residual_rank: 320,
            expert_count: 512,
            experts_per_token: 10,
            norm_topk_prob,
            expert_intermediate_size: 640,
            shared_expert_intermediate_size: 640,
            ngram_size: 3,
            heads_per_ngram: 8,
            ngram_vocab_size_base: 20_000_000,
            ngram_split_parts: 128,
            ngram_vocab_divisor: 128,
            ngram_seed: 1234,
            ple_embedding_dim: 2560,
            ple_conv_kernel_size: 4,
            eos_token_id: 248_044,
            vocab_size: 248_320,
            max_position_embeddings: 262_144,
            rope_theta,
            partial_rotary_factor,
            rms_norm_epsilon,
            ple_layer_ids: vec![2],
        })
    }
}
