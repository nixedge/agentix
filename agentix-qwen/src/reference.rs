use crate::{Error, Result};

fn finite(values: &[f32]) -> Result<()> {
    if values.iter().all(|x| x.is_finite()) {
        Ok(())
    } else {
        Err(Error::Invalid("non-finite operator input or result"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Route {
    pub expert: usize,
    pub weight: f32,
}

/// Routes one token from already projected logits. Ties prefer the lower expert ID.
pub fn route(logits: &[f32], top_k: usize, normalize_selected: bool) -> Result<Vec<Route>> {
    if top_k == 0 || top_k > logits.len() {
        return Err(Error::Invalid("invalid routed expert count"));
    }
    finite(logits)?;
    let maximum = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let probabilities: Vec<f32> = logits.iter().map(|x| (x - maximum).exp()).collect();
    let total: f32 = probabilities.iter().sum();
    let mut routes: Vec<Route> = probabilities
        .iter()
        .enumerate()
        .map(|(expert, p)| Route {
            expert,
            weight: p / total,
        })
        .collect();
    routes.sort_by(|a, b| b.weight.total_cmp(&a.weight).then(a.expert.cmp(&b.expert)));
    routes.truncate(top_k);
    if normalize_selected {
        let selected: f32 = routes.iter().map(|r| r.weight).sum();
        for route in &mut routes {
            route.weight /= selected;
        }
    }
    Ok(routes)
}

/// Selects QSA's complete compressed key blocks and always appends the
/// incomplete visible-token tail. Queries are already RMS-normalized and RoPE
/// rotated; raw token keys are pooled and normalized here before block-start
/// RoPE. Visibility is supplied by intersecting the causal and padding masks.
#[allow(clippy::too_many_arguments)]
pub fn qsa_select_tokens(
    query_heads: &[f32],
    raw_keys: &[f32],
    key_norm_weight: &[f32],
    visible: &[bool],
    rope_cos: &[f32],
    rope_sin: &[f32],
    head_dim: usize,
    rotary_dim: usize,
    compress_ratio: usize,
    block_budget: usize,
    epsilon: f32,
) -> Result<Vec<usize>> {
    let rope_values = visible
        .len()
        .checked_mul(rotary_dim)
        .ok_or(Error::Overflow)?;
    if head_dim == 0
        || query_heads.is_empty()
        || !query_heads.len().is_multiple_of(head_dim)
        || raw_keys.len() != visible.len().checked_mul(head_dim).ok_or(Error::Overflow)?
        || key_norm_weight.len() != head_dim
        || rotary_dim == 0
        || rotary_dim > head_dim
        || !rotary_dim.is_multiple_of(2)
        || rope_cos.len() != rope_values
        || rope_sin.len() != rope_values
        || compress_ratio == 0
        || block_budget == 0
    {
        return Err(Error::Invalid("QSA indexer geometry mismatch"));
    }
    finite(query_heads)?;
    finite(raw_keys)?;
    finite(key_norm_weight)?;
    finite(rope_cos)?;
    finite(rope_sin)?;
    if !epsilon.is_finite() || epsilon <= 0.0 {
        return Err(Error::Invalid("invalid QSA normalization epsilon"));
    }

    let mut visible_indices = Vec::new();
    visible_indices
        .try_reserve_exact(visible.len())
        .map_err(|_| Error::Capacity("QSA visible token indices"))?;
    visible_indices.extend(
        visible
            .iter()
            .enumerate()
            .filter_map(|(index, &is_visible)| is_visible.then_some(index)),
    );
    let complete_tokens = visible_indices.len() / compress_ratio * compress_ratio;
    let blocks = visible_indices[..complete_tokens].chunks_exact(compress_ratio);
    let mut scored_blocks = Vec::new();
    let block_count = complete_tokens / compress_ratio;
    scored_blocks
        .try_reserve_exact(block_count)
        .map_err(|_| Error::Capacity("QSA block scores"))?;
    let query_count = query_heads.len() / head_dim;
    let scale = (head_dim as f32).sqrt().recip();
    for (block_index, block) in blocks.enumerate() {
        let mut pooled = Vec::new();
        pooled
            .try_reserve_exact(head_dim)
            .map_err(|_| Error::Capacity("QSA pooled key"))?;
        for column in 0..head_dim {
            let sum = block
                .iter()
                .map(|&token| raw_keys[token * head_dim + column])
                .sum::<f32>();
            pooled.push(sum / compress_ratio as f32);
        }
        let mut pooled = rms_norm(&pooled, key_norm_weight, head_dim, epsilon)?;
        let start = block[0];
        let cos_start = start * rotary_dim;
        for index in 0..rotary_dim / 2 {
            let first = pooled[index];
            let second = pooled[index + rotary_dim / 2];
            pooled[index] =
                first * rope_cos[cos_start + index] - second * rope_sin[cos_start + index];
            pooled[index + rotary_dim / 2] = second * rope_cos[cos_start + index + rotary_dim / 2]
                + first * rope_sin[cos_start + index + rotary_dim / 2];
        }
        finite(&pooled)?;

        let mut score = 0.0_f32;
        for head in 0..query_count {
            let query_start = head * head_dim;
            let dot = query_heads[query_start..query_start + head_dim]
                .iter()
                .zip(&pooled)
                .map(|(query, key)| query * key)
                .sum::<f32>();
            if !dot.is_finite() {
                return Err(Error::Invalid("QSA indexer score overflow"));
            }
            score += dot.max(0.0);
        }
        score *= scale;
        if !score.is_finite() {
            return Err(Error::Invalid("QSA indexer score overflow"));
        }
        scored_blocks.push((block_index, score));
    }
    scored_blocks
        .sort_unstable_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));

    let selected_blocks = block_budget.min(block_count);
    let tail_count = visible_indices.len() - complete_tokens;
    let selected_count = selected_blocks
        .checked_mul(compress_ratio)
        .and_then(|count| count.checked_add(tail_count))
        .ok_or(Error::Overflow)?;
    let mut selected = Vec::new();
    selected
        .try_reserve_exact(selected_count)
        .map_err(|_| Error::Capacity("QSA selected token indices"))?;
    for &(block_index, _) in scored_blocks.iter().take(selected_blocks) {
        let start = block_index * compress_ratio;
        selected.extend_from_slice(&visible_indices[start..start + compress_ratio]);
    }
    selected.extend_from_slice(&visible_indices[complete_tokens..]);
    Ok(selected)
}

/// Single-query FP32 grouped-query attention. Query/key/value tensors are
/// flattened token-major by head; keys are already RoPE-rotated and `visible`
/// must combine the causal/padding mask with any QSA-selected-token mask.
#[allow(clippy::too_many_arguments)]
pub fn grouped_query_attention(
    query: &[f32],
    output_gate: &[f32],
    keys: &[f32],
    values: &[f32],
    visible: &[bool],
    query_heads: usize,
    kv_heads: usize,
    head_dim: usize,
) -> Result<Vec<f32>> {
    if query_heads == 0 || kv_heads == 0 || head_dim == 0 || !query_heads.is_multiple_of(kv_heads) {
        return Err(Error::Invalid("attention head geometry mismatch"));
    }
    let query_width = query_heads.checked_mul(head_dim).ok_or(Error::Overflow)?;
    let kv_width = kv_heads.checked_mul(head_dim).ok_or(Error::Overflow)?;
    let cache_width = visible.len().checked_mul(kv_width).ok_or(Error::Overflow)?;
    if query.len() != query_width
        || output_gate.len() != query_width
        || keys.len() != cache_width
        || values.len() != cache_width
        || !visible.iter().any(|&is_visible| is_visible)
    {
        return Err(Error::Invalid("attention tensor or mask shape mismatch"));
    }
    finite(query)?;
    finite(output_gate)?;
    finite(keys)?;
    finite(values)?;

    let mut output = Vec::new();
    output
        .try_reserve_exact(query_width)
        .map_err(|_| Error::Capacity("attention output"))?;
    let groups_per_kv = query_heads / kv_heads;
    let scale = (head_dim as f32).sqrt().recip();
    for query_head in 0..query_heads {
        let kv_head = query_head / groups_per_kv;
        let query_start = query_head * head_dim;
        let mut scores = Vec::new();
        scores
            .try_reserve_exact(visible.len())
            .map_err(|_| Error::Capacity("attention scores"))?;
        let mut max_score = f32::NEG_INFINITY;
        for (token, &is_visible) in visible.iter().enumerate() {
            if is_visible {
                let key_start = token * kv_width + kv_head * head_dim;
                let score = query[query_start..query_start + head_dim]
                    .iter()
                    .zip(&keys[key_start..key_start + head_dim])
                    .map(|(query, key)| query * key)
                    .sum::<f32>()
                    * scale;
                if !score.is_finite() {
                    return Err(Error::Invalid("attention score overflow"));
                }
                scores.push((token, score));
                max_score = max_score.max(score);
            }
        }
        let mut denominator = 0.0_f32;
        for (_, score) in &mut scores {
            *score = (*score - max_score).exp();
            denominator += *score;
        }
        if !denominator.is_finite() || denominator <= 0.0 {
            return Err(Error::Invalid("invalid attention softmax denominator"));
        }
        let mut head_output = vec![0.0_f32; head_dim];
        for (token, unnormalized) in scores {
            let weight = unnormalized / denominator;
            let value_start = token * kv_width + kv_head * head_dim;
            for column in 0..head_dim {
                head_output[column] += weight * values[value_start + column];
            }
        }
        for (column, value) in head_output.into_iter().enumerate() {
            let index = query_start + column;
            output.push(value * sigmoid(output_gate[index]));
        }
    }
    finite(&output)?;
    Ok(output)
}

#[derive(Clone, Copy)]
pub struct QsaAttentionWeights<'a> {
    /// Fused per-head `[query, gate]` output projection, PyTorch linear order.
    pub query_gate_projection: &'a [f32],
    pub key_projection: &'a [f32],
    pub value_projection: &'a [f32],
    pub query_norm_offset: &'a [f32],
    pub key_norm_offset: &'a [f32],
    pub output_projection: &'a [f32],
}

#[derive(Clone, Debug, PartialEq)]
pub struct QsaAttentionOutput {
    pub output: Vec<f32>,
    /// Updated token-major, head-interleaved cache, including this token.
    pub keys: Vec<f32>,
    pub values: Vec<f32>,
}

/// One-token QSA main-attention reference. Caller visibility combines causal,
/// padding, and indexer selection for the appended current token.
#[allow(clippy::too_many_arguments)]
pub fn qsa_attention_step(
    hidden: &[f32],
    past_keys: &[f32],
    past_values: &[f32],
    visible: &[bool],
    query_heads: usize,
    kv_heads: usize,
    head_dim: usize,
    epsilon: f32,
    rope: &TextRotaryEmbedding,
    position: u32,
    weights: QsaAttentionWeights<'_>,
) -> Result<QsaAttentionOutput> {
    if hidden.is_empty()
        || query_heads == 0
        || kv_heads == 0
        || !query_heads.is_multiple_of(kv_heads)
        || head_dim == 0
        || rope.head_dim() != head_dim
        || weights.query_norm_offset.len() != head_dim
        || weights.key_norm_offset.len() != head_dim
        || !epsilon.is_finite()
        || epsilon <= 0.0
    {
        return Err(Error::Invalid("QSA attention geometry mismatch"));
    }
    let query_width = query_heads.checked_mul(head_dim).ok_or(Error::Overflow)?;
    let kv_width = kv_heads.checked_mul(head_dim).ok_or(Error::Overflow)?;
    let packed_query_width = query_width.checked_mul(2).ok_or(Error::Overflow)?;
    let expected_query_projection = packed_query_width
        .checked_mul(hidden.len())
        .ok_or(Error::Overflow)?;
    let expected_kv_projection = kv_width.checked_mul(hidden.len()).ok_or(Error::Overflow)?;
    let past_tokens = past_keys
        .len()
        .checked_div(kv_width)
        .ok_or(Error::Overflow)?;
    if past_keys.len() != past_tokens.checked_mul(kv_width).ok_or(Error::Overflow)?
        || past_values.len() != past_keys.len()
        || visible.len() != past_tokens.checked_add(1).ok_or(Error::Overflow)?
        || weights.query_gate_projection.len() != expected_query_projection
        || weights.key_projection.len() != expected_kv_projection
        || weights.value_projection.len() != expected_kv_projection
        || !weights.output_projection.len().is_multiple_of(query_width)
    {
        return Err(Error::Invalid("QSA attention tensor shape mismatch"));
    }
    finite(weights.query_norm_offset)?;
    finite(weights.key_norm_offset)?;

    let packed = linear(hidden, weights.query_gate_projection, None)?;
    let raw_keys = linear(hidden, weights.key_projection, None)?;
    let current_values = linear(hidden, weights.value_projection, None)?;
    qsa_attention_finish(
        hidden,
        past_keys,
        past_values,
        visible,
        query_heads,
        kv_heads,
        head_dim,
        epsilon,
        rope,
        position,
        weights.query_norm_offset,
        weights.key_norm_offset,
        packed,
        raw_keys,
        current_values,
        |attended| linear(attended, weights.output_projection, None),
    )
}

/// Row-backed equivalent of `qsa_attention_step` for quantized GGUF matrices.
#[allow(clippy::too_many_arguments)]
pub fn qsa_attention_step_from_rows<Q, K, V, O>(
    hidden: &[f32],
    past_keys: &[f32],
    past_values: &[f32],
    visible: &[bool],
    query_heads: usize,
    kv_heads: usize,
    head_dim: usize,
    epsilon: f32,
    rope: &TextRotaryEmbedding,
    position: u32,
    query_gate_projection: &mut Q,
    key_projection: &mut K,
    value_projection: &mut V,
    query_norm_offset: &[f32],
    key_norm_offset: &[f32],
    output_projection: &mut O,
) -> Result<QsaAttentionOutput>
where
    Q: LinearWeightRows,
    K: LinearWeightRows,
    V: LinearWeightRows,
    O: LinearWeightRows,
{
    if hidden.is_empty()
        || query_heads == 0
        || kv_heads == 0
        || !query_heads.is_multiple_of(kv_heads)
        || head_dim == 0
        || rope.head_dim() != head_dim
        || query_norm_offset.len() != head_dim
        || key_norm_offset.len() != head_dim
        || !epsilon.is_finite()
        || epsilon <= 0.0
    {
        return Err(Error::Invalid("QSA attention geometry mismatch"));
    }
    let query_width = query_heads.checked_mul(head_dim).ok_or(Error::Overflow)?;
    let kv_width = kv_heads.checked_mul(head_dim).ok_or(Error::Overflow)?;
    let packed_query_width = query_width.checked_mul(2).ok_or(Error::Overflow)?;
    let past_tokens = past_keys
        .len()
        .checked_div(kv_width)
        .ok_or(Error::Overflow)?;
    if past_keys.len() != past_tokens.checked_mul(kv_width).ok_or(Error::Overflow)?
        || past_values.len() != past_keys.len()
        || visible.len() != past_tokens.checked_add(1).ok_or(Error::Overflow)?
        || query_gate_projection.input_features() != hidden.len()
        || usize::try_from(query_gate_projection.output_features()).ok() != Some(packed_query_width)
        || key_projection.input_features() != hidden.len()
        || usize::try_from(key_projection.output_features()).ok() != Some(kv_width)
        || value_projection.input_features() != hidden.len()
        || usize::try_from(value_projection.output_features()).ok() != Some(kv_width)
        || output_projection.input_features() != query_width
        || usize::try_from(output_projection.output_features()).ok() != Some(hidden.len())
    {
        return Err(Error::Invalid("QSA attention tensor shape mismatch"));
    }
    finite(query_norm_offset)?;
    finite(key_norm_offset)?;
    let packed = linear_from_rows(hidden, query_gate_projection, None)?;
    let raw_keys = linear_from_rows(hidden, key_projection, None)?;
    let current_values = linear_from_rows(hidden, value_projection, None)?;
    qsa_attention_finish(
        hidden,
        past_keys,
        past_values,
        visible,
        query_heads,
        kv_heads,
        head_dim,
        epsilon,
        rope,
        position,
        query_norm_offset,
        key_norm_offset,
        packed,
        raw_keys,
        current_values,
        |attended| linear_from_rows(attended, output_projection, None),
    )
}

#[allow(clippy::too_many_arguments)]
fn qsa_attention_finish<F>(
    hidden: &[f32],
    past_keys: &[f32],
    past_values: &[f32],
    visible: &[bool],
    query_heads: usize,
    kv_heads: usize,
    head_dim: usize,
    epsilon: f32,
    rope: &TextRotaryEmbedding,
    position: u32,
    query_norm_offset: &[f32],
    key_norm_offset: &[f32],
    packed: Vec<f32>,
    raw_keys: Vec<f32>,
    current_values: Vec<f32>,
    project_output: F,
) -> Result<QsaAttentionOutput>
where
    F: FnOnce(&[f32]) -> Result<Vec<f32>>,
{
    let query_width = query_heads.checked_mul(head_dim).ok_or(Error::Overflow)?;
    let kv_width = kv_heads.checked_mul(head_dim).ok_or(Error::Overflow)?;
    let packed_head_width = head_dim.checked_mul(2).ok_or(Error::Overflow)?;
    if packed.len() != query_width.checked_mul(2).ok_or(Error::Overflow)?
        || raw_keys.len() != kv_width
        || current_values.len() != kv_width
        || past_keys.len() != past_values.len()
        || !past_keys.len().is_multiple_of(kv_width)
        || visible.len() != past_keys.len() / kv_width + 1
        || query_norm_offset.len() != head_dim
        || key_norm_offset.len() != head_dim
    {
        return Err(Error::Invalid("QSA projected attention shape mismatch"));
    }
    let mut raw_queries = Vec::new();
    let mut gates = Vec::new();
    raw_queries
        .try_reserve_exact(query_width)
        .map_err(|_| Error::Capacity("QSA attention queries"))?;
    gates
        .try_reserve_exact(query_width)
        .map_err(|_| Error::Capacity("QSA attention gates"))?;
    for head in packed.chunks_exact(packed_head_width) {
        raw_queries.extend_from_slice(&head[..head_dim]);
        gates.extend_from_slice(&head[head_dim..]);
    }
    let mut normalized_queries = Vec::new();
    normalized_queries
        .try_reserve_exact(query_width)
        .map_err(|_| Error::Capacity("QSA normalized attention queries"))?;
    for query in raw_queries.chunks_exact(head_dim) {
        normalized_queries.extend(rms_norm(query, query_norm_offset, head_dim, epsilon)?);
    }
    let normalized_queries = rope.apply(&normalized_queries, position)?;

    let mut normalized_keys = Vec::new();
    normalized_keys
        .try_reserve_exact(kv_width)
        .map_err(|_| Error::Capacity("QSA normalized attention keys"))?;
    for key in raw_keys.chunks_exact(head_dim) {
        normalized_keys.extend(rms_norm(key, key_norm_offset, head_dim, epsilon)?);
    }
    let current_keys = rope.apply(&normalized_keys, position)?;
    let cache_len = visible.len().checked_mul(kv_width).ok_or(Error::Overflow)?;
    let mut keys = Vec::new();
    let mut values = Vec::new();
    keys.try_reserve_exact(cache_len)
        .map_err(|_| Error::Capacity("QSA attention key cache"))?;
    values
        .try_reserve_exact(cache_len)
        .map_err(|_| Error::Capacity("QSA attention value cache"))?;
    keys.extend_from_slice(past_keys);
    keys.extend_from_slice(&current_keys);
    values.extend_from_slice(past_values);
    values.extend_from_slice(&current_values);

    let attended = grouped_query_attention(
        &normalized_queries,
        &gates,
        &keys,
        &values,
        visible,
        query_heads,
        kv_heads,
        head_dim,
    )?;
    let output = project_output(&attended)?;
    if output.len() != hidden.len() {
        return Err(Error::Invalid(
            "QSA attention output projection shape mismatch",
        ));
    }
    Ok(QsaAttentionOutput {
        output,
        keys,
        values,
    })
}

/// Text-only partial RoPE used by both QSA indexer heads and main attention.
/// The checkpoint's three MRoPE position axes are equal for text, reducing its
/// frequency recomposition to the standard duplicated half-frequency layout.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRotaryEmbedding {
    head_dim: usize,
    rotary_dim: usize,
    inverse_frequencies: Vec<f32>,
}

impl TextRotaryEmbedding {
    pub fn new(head_dim: usize, rotary_dim: usize, theta: f32) -> Result<Self> {
        if head_dim == 0
            || rotary_dim == 0
            || rotary_dim > head_dim
            || !rotary_dim.is_multiple_of(2)
            || !theta.is_finite()
            || theta <= 0.0
        {
            return Err(Error::Invalid("text RoPE geometry or theta is invalid"));
        }
        let half = rotary_dim / 2;
        let mut inverse_frequencies = Vec::new();
        inverse_frequencies
            .try_reserve_exact(half)
            .map_err(|_| Error::Capacity("text RoPE frequencies"))?;
        for index in 0..half {
            let exponent = (2 * index) as f32 / rotary_dim as f32;
            let inverse = 1.0 / theta.powf(exponent);
            if !inverse.is_finite() {
                return Err(Error::Invalid("text RoPE frequency overflow"));
            }
            inverse_frequencies.push(inverse);
        }
        Ok(Self {
            head_dim,
            rotary_dim,
            inverse_frequencies,
        })
    }

    pub fn rotary_dim(&self) -> usize {
        self.rotary_dim
    }

    pub fn head_dim(&self) -> usize {
        self.head_dim
    }

    /// Returns `cos`/`sin` repeated across both halves, matching the pinned
    /// Transformers `rotate_half` implementation.
    pub fn cos_sin(&self, position: u32) -> Result<(Vec<f32>, Vec<f32>)> {
        let half = self.rotary_dim / 2;
        let mut cos = Vec::new();
        let mut sin = Vec::new();
        cos.try_reserve_exact(self.rotary_dim)
            .map_err(|_| Error::Capacity("text RoPE cosine"))?;
        sin.try_reserve_exact(self.rotary_dim)
            .map_err(|_| Error::Capacity("text RoPE sine"))?;
        for &inverse in &self.inverse_frequencies {
            let phase = position as f32 * inverse;
            if !phase.is_finite() {
                return Err(Error::Invalid("text RoPE phase overflow"));
            }
            cos.push(phase.cos());
            sin.push(phase.sin());
        }
        cos.extend_from_within(0..half);
        sin.extend_from_within(0..half);
        debug_assert_eq!(cos.len(), half * 2);
        Ok((cos, sin))
    }

    /// Rotates one or more flattened head vectors, preserving dimensions after
    /// the configured partial rotary prefix.
    pub fn apply(&self, input: &[f32], position: u32) -> Result<Vec<f32>> {
        if input.is_empty() || !input.len().is_multiple_of(self.head_dim) {
            return Err(Error::Invalid("text RoPE input shape mismatch"));
        }
        finite(input)?;
        let (cos, sin) = self.cos_sin(position)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(input.len())
            .map_err(|_| Error::Capacity("text RoPE output"))?;
        output.extend_from_slice(input);
        let half = self.rotary_dim / 2;
        for head in 0..input.len() / self.head_dim {
            let start = head * self.head_dim;
            for index in 0..half {
                let first = input[start + index];
                let second = input[start + index + half];
                output[start + index] = first * cos[index] - second * sin[index];
                output[start + index + half] =
                    second * cos[index + half] + first * sin[index + half];
            }
        }
        finite(&output)?;
        Ok(output)
    }
}

/// Projects one hidden vector through Qwen's fused QSA indexer projection,
/// applies its per-query-head RMSNorm and text RoPE, and returns the raw
/// single-head token key. Norm weights use the source parameter convention:
/// the effective scale is `1 + query_norm_offset`.
#[allow(clippy::too_many_arguments)]
pub fn qsa_indexer_project(
    hidden: &[f32],
    projection: &[f32],
    query_norm_offset: &[f32],
    query_head_count: usize,
    kv_head_count: usize,
    head_dim: usize,
    epsilon: f32,
    rope: &TextRotaryEmbedding,
    position: u32,
) -> Result<(Vec<f32>, Vec<f32>)> {
    if hidden.is_empty()
        || query_head_count == 0
        || kv_head_count != 1
        || head_dim == 0
        || rope.head_dim() != head_dim
        || query_norm_offset.len() != head_dim
        || !epsilon.is_finite()
        || epsilon <= 0.0
    {
        return Err(Error::Invalid("QSA indexer projection geometry mismatch"));
    }
    let q_width = query_head_count
        .checked_mul(head_dim)
        .ok_or(Error::Overflow)?;
    let kv_width = kv_head_count.checked_mul(head_dim).ok_or(Error::Overflow)?;
    let projected_width = q_width.checked_add(kv_width).ok_or(Error::Overflow)?;
    let projected = linear(hidden, projection, None)?;
    if projected.len() != projected_width {
        return Err(Error::Invalid("QSA indexer projection shape mismatch"));
    }
    finite(query_norm_offset)?;
    let mut normalized_queries = Vec::new();
    normalized_queries
        .try_reserve_exact(q_width)
        .map_err(|_| Error::Capacity("QSA normalized queries"))?;
    for query in projected[..q_width].chunks_exact(head_dim) {
        normalized_queries.extend(rms_norm(query, query_norm_offset, head_dim, epsilon)?);
    }
    let queries = rope.apply(&normalized_queries, position)?;
    let mut raw_key = Vec::new();
    raw_key
        .try_reserve_exact(kv_width)
        .map_err(|_| Error::Capacity("QSA raw index keys"))?;
    raw_key.extend_from_slice(&projected[q_width..]);
    finite(&raw_key)?;
    Ok((queries, raw_key))
}

/// Projects a QSA indexer query and raw key from separate streamed matrices.
#[allow(clippy::too_many_arguments)]
pub fn qsa_indexer_project_from_rows<Q, K>(
    hidden: &[f32],
    query_projection: &mut Q,
    key_projection: &mut K,
    query_norm_offset: &[f32],
    query_head_count: usize,
    head_dim: usize,
    epsilon: f32,
    rope: &TextRotaryEmbedding,
    position: u32,
) -> Result<(Vec<f32>, Vec<f32>)>
where
    Q: LinearWeightRows,
    K: LinearWeightRows,
{
    if hidden.is_empty()
        || query_head_count == 0
        || head_dim == 0
        || rope.head_dim() != head_dim
        || query_norm_offset.len() != head_dim
        || !epsilon.is_finite()
        || epsilon <= 0.0
        || query_projection.input_features() != hidden.len()
        || usize::try_from(query_projection.output_features()).ok()
            != Some(
                query_head_count
                    .checked_mul(head_dim)
                    .ok_or(Error::Overflow)?,
            )
        || key_projection.input_features() != hidden.len()
        || usize::try_from(key_projection.output_features()).ok() != Some(head_dim)
    {
        return Err(Error::Invalid(
            "row-backed QSA indexer projection geometry mismatch",
        ));
    }
    finite(query_norm_offset)?;
    let projected_queries = linear_from_rows(hidden, query_projection, None)?;
    let raw_key = linear_from_rows(hidden, key_projection, None)?;
    let mut normalized_queries = Vec::new();
    normalized_queries
        .try_reserve_exact(projected_queries.len())
        .map_err(|_| Error::Capacity("QSA normalized indexer queries"))?;
    for query in projected_queries.chunks_exact(head_dim) {
        normalized_queries.extend(rms_norm(query, query_norm_offset, head_dim, epsilon)?);
    }
    let queries = rope.apply(&normalized_queries, position)?;
    Ok((queries, raw_key))
}

#[derive(Clone, Debug, PartialEq)]
pub struct QsaIndexerOutput {
    pub selected_token_indices: Vec<usize>,
    /// Updated token-major raw-key cache, including the current token.
    pub raw_key_cache: Vec<f32>,
}

#[derive(Clone, Copy)]
pub struct QsaIndexerWeights<'a> {
    pub projection: &'a [f32],
    pub query_norm_offset: &'a [f32],
    pub key_norm_offset: &'a [f32],
}

/// One-token QSA indexer reference for decode. It projects the current query
/// and raw key, appends the key to an immutable cache copy, builds text RoPE
/// angles for the supplied absolute key positions, and selects visible blocks.
#[allow(clippy::too_many_arguments)]
pub fn qsa_indexer_step(
    hidden: &[f32],
    past_raw_keys: &[f32],
    past_positions: &[u32],
    visible: &[bool],
    projection: &[f32],
    query_norm_offset: &[f32],
    key_norm_offset: &[f32],
    query_heads: usize,
    head_dim: usize,
    compress_ratio: usize,
    token_budget: usize,
    epsilon: f32,
    rope: &TextRotaryEmbedding,
    position: u32,
) -> Result<QsaIndexerOutput> {
    if head_dim == 0
        || compress_ratio == 0
        || token_budget < compress_ratio
        || rope.head_dim() != head_dim
        || visible.len() != past_positions.len().checked_add(1).ok_or(Error::Overflow)?
        || !visible.iter().any(|&is_visible| is_visible)
    {
        return Err(Error::Invalid("QSA indexer step geometry mismatch"));
    }
    let past_width = past_positions
        .len()
        .checked_mul(head_dim)
        .ok_or(Error::Overflow)?;
    if past_raw_keys.len() != past_width {
        return Err(Error::Invalid("QSA raw-key cache shape mismatch"));
    }
    let (query, current_raw_key) = qsa_indexer_project(
        hidden,
        projection,
        query_norm_offset,
        query_heads,
        1,
        head_dim,
        epsilon,
        rope,
        position,
    )?;
    qsa_indexer_select_cached(
        &query,
        &current_raw_key,
        past_raw_keys,
        past_positions,
        visible,
        key_norm_offset,
        compress_ratio,
        token_budget,
        epsilon,
        rope,
        position,
    )
}

#[allow(clippy::too_many_arguments)]
fn qsa_indexer_select_cached(
    query: &[f32],
    current_raw_key: &[f32],
    past_raw_keys: &[f32],
    past_positions: &[u32],
    visible: &[bool],
    key_norm_offset: &[f32],
    compress_ratio: usize,
    token_budget: usize,
    epsilon: f32,
    rope: &TextRotaryEmbedding,
    position: u32,
) -> Result<QsaIndexerOutput> {
    let head_dim = rope.head_dim();
    if query.is_empty()
        || !query.len().is_multiple_of(head_dim)
        || current_raw_key.len() != head_dim
        || past_raw_keys.len()
            != past_positions
                .len()
                .checked_mul(head_dim)
                .ok_or(Error::Overflow)?
        || visible.len() != past_positions.len().checked_add(1).ok_or(Error::Overflow)?
        || key_norm_offset.len() != head_dim
        || compress_ratio == 0
        || token_budget < compress_ratio
    {
        return Err(Error::Invalid("QSA indexer cache selection shape mismatch"));
    }
    let cache_width = visible.len().checked_mul(head_dim).ok_or(Error::Overflow)?;
    let mut raw_key_cache = Vec::new();
    raw_key_cache
        .try_reserve_exact(cache_width)
        .map_err(|_| Error::Capacity("QSA raw-key cache"))?;
    raw_key_cache.extend_from_slice(past_raw_keys);
    raw_key_cache.extend_from_slice(current_raw_key);

    let mut rope_cos = Vec::new();
    let mut rope_sin = Vec::new();
    let angle_width = visible
        .len()
        .checked_mul(rope.rotary_dim())
        .ok_or(Error::Overflow)?;
    rope_cos
        .try_reserve_exact(angle_width)
        .map_err(|_| Error::Capacity("QSA key RoPE cosine"))?;
    rope_sin
        .try_reserve_exact(angle_width)
        .map_err(|_| Error::Capacity("QSA key RoPE sine"))?;
    for &key_position in past_positions.iter().chain(std::iter::once(&position)) {
        let (cos, sin) = rope.cos_sin(key_position)?;
        rope_cos.extend(cos);
        rope_sin.extend(sin);
    }
    let selected_token_indices = qsa_select_tokens(
        query,
        &raw_key_cache,
        key_norm_offset,
        visible,
        &rope_cos,
        &rope_sin,
        head_dim,
        rope.rotary_dim(),
        compress_ratio,
        token_budget / compress_ratio,
        epsilon,
    )?;
    Ok(QsaIndexerOutput {
        selected_token_indices,
        raw_key_cache,
    })
}

/// One-token QSA indexer step from separate streamed query and key matrices.
#[allow(clippy::too_many_arguments)]
pub fn qsa_indexer_step_from_rows<Q, K>(
    hidden: &[f32],
    past_raw_keys: &[f32],
    past_positions: &[u32],
    visible: &[bool],
    query_projection: &mut Q,
    key_projection: &mut K,
    query_norm_offset: &[f32],
    key_norm_offset: &[f32],
    query_heads: usize,
    head_dim: usize,
    compress_ratio: usize,
    token_budget: usize,
    epsilon: f32,
    rope: &TextRotaryEmbedding,
    position: u32,
) -> Result<QsaIndexerOutput>
where
    Q: LinearWeightRows,
    K: LinearWeightRows,
{
    if head_dim == 0
        || compress_ratio == 0
        || token_budget < compress_ratio
        || rope.head_dim() != head_dim
        || visible.len() != past_positions.len().checked_add(1).ok_or(Error::Overflow)?
        || !visible.iter().any(|&is_visible| is_visible)
    {
        return Err(Error::Invalid("QSA indexer step geometry mismatch"));
    }
    let past_width = past_positions
        .len()
        .checked_mul(head_dim)
        .ok_or(Error::Overflow)?;
    if past_raw_keys.len() != past_width {
        return Err(Error::Invalid("QSA raw-key cache shape mismatch"));
    }
    let (query, current_raw_key) = qsa_indexer_project_from_rows(
        hidden,
        query_projection,
        key_projection,
        query_norm_offset,
        query_heads,
        head_dim,
        epsilon,
        rope,
        position,
    )?;
    qsa_indexer_select_cached(
        &query,
        &current_raw_key,
        past_raw_keys,
        past_positions,
        visible,
        key_norm_offset,
        compress_ratio,
        token_budget,
        epsilon,
        rope,
        position,
    )
}

#[derive(Clone, Copy)]
pub struct QsaDecodeWeights<'a> {
    pub indexer: QsaIndexerWeights<'a>,
    pub attention: QsaAttentionWeights<'a>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QsaDecodeOutput {
    pub output: Vec<f32>,
    pub selected_token_indices: Vec<usize>,
    pub raw_index_key_cache: Vec<f32>,
    pub attention_keys: Vec<f32>,
    pub attention_values: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QsaDecodeGeometry {
    pub indexer_query_heads: usize,
    pub indexer_head_dim: usize,
    pub query_heads: usize,
    pub kv_heads: usize,
    pub head_dim: usize,
    pub compress_ratio: usize,
    pub token_budget: usize,
    pub epsilon: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QsaPrefillOutput {
    pub outputs: Vec<f32>,
    pub selected_token_indices: Vec<Vec<usize>>,
    pub raw_index_key_cache: Vec<f32>,
    pub attention_keys: Vec<f32>,
    pub attention_values: Vec<f32>,
}

pub struct QsaPrefillInput<'a> {
    pub hidden_states: &'a [f32],
    pub hidden_width: usize,
    pub positions: &'a [u32],
    /// Row-major lower-triangular visibility rows, each containing `t+1` keys.
    pub triangular_mask: &'a [bool],
}

pub struct QsaCachedPrefillInput<'a> {
    pub hidden_states: &'a [f32],
    pub hidden_width: usize,
    pub prefix_positions: &'a [u32],
    pub prefix_raw_index_keys: &'a [f32],
    pub prefix_attention_keys: &'a [f32],
    pub prefix_attention_values: &'a [f32],
    pub positions: &'a [u32],
    /// Row-major `[new_tokens, prefix_tokens + new_tokens]` visibility. Future
    /// positions in each row must be masked out.
    pub visibility_mask: &'a [bool],
}

/// Sequential CPU QSA prefill reference for a new sequence. `triangular_mask`
/// stores visible keys in row-major lower-triangular rows: row `t` contains
/// exactly `t+1` entries for keys `0..=t`.
pub fn qsa_prefill(
    input: QsaPrefillInput<'_>,
    geometry: QsaDecodeGeometry,
    indexer_rope: &TextRotaryEmbedding,
    attention_rope: &TextRotaryEmbedding,
    weights: QsaDecodeWeights<'_>,
) -> Result<QsaPrefillOutput> {
    let QsaPrefillInput {
        hidden_states,
        hidden_width,
        positions,
        triangular_mask,
    } = input;
    if hidden_width == 0
        || hidden_states.is_empty()
        || !hidden_states.len().is_multiple_of(hidden_width)
    {
        return Err(Error::Invalid("QSA prefill hidden-state shape mismatch"));
    }
    let token_count = hidden_states.len() / hidden_width;
    let triangular_elements = token_count
        .checked_mul(token_count.checked_add(1).ok_or(Error::Overflow)?)
        .and_then(|elements| elements.checked_div(2))
        .ok_or(Error::Overflow)?;
    if positions.len() != token_count || triangular_mask.len() != triangular_elements {
        return Err(Error::Invalid(
            "QSA prefill positions or mask shape mismatch",
        ));
    }
    let total_tokens = token_count;
    let rectangular_elements = token_count
        .checked_mul(total_tokens)
        .ok_or(Error::Overflow)?;
    let mut visibility_mask = Vec::new();
    visibility_mask
        .try_reserve_exact(rectangular_elements)
        .map_err(|_| Error::Capacity("QSA prefill visibility"))?;
    for token in 0..token_count {
        let row_start = token
            .checked_mul(token.checked_add(1).ok_or(Error::Overflow)?)
            .and_then(|offset| offset.checked_div(2))
            .ok_or(Error::Overflow)?;
        let row_end = row_start.checked_add(token + 1).ok_or(Error::Overflow)?;
        visibility_mask.extend_from_slice(&triangular_mask[row_start..row_end]);
        visibility_mask.resize(
            visibility_mask
                .len()
                .checked_add(total_tokens - token - 1)
                .ok_or(Error::Overflow)?,
            false,
        );
    }
    qsa_prefill_with_cache(
        QsaCachedPrefillInput {
            hidden_states,
            hidden_width,
            prefix_positions: &[],
            prefix_raw_index_keys: &[],
            prefix_attention_keys: &[],
            prefix_attention_values: &[],
            positions,
            visibility_mask: &visibility_mask,
        },
        geometry,
        indexer_rope,
        attention_rope,
        weights,
    )
}

/// Sequential QSA prefill that continues an existing indexer and attention
/// cache. Visibility rows are rectangular so callers can represent prefix,
/// causal, padding, and sparse selection masks explicitly.
pub fn qsa_prefill_with_cache(
    input: QsaCachedPrefillInput<'_>,
    geometry: QsaDecodeGeometry,
    indexer_rope: &TextRotaryEmbedding,
    attention_rope: &TextRotaryEmbedding,
    weights: QsaDecodeWeights<'_>,
) -> Result<QsaPrefillOutput> {
    let QsaCachedPrefillInput {
        hidden_states,
        hidden_width,
        prefix_positions,
        prefix_raw_index_keys,
        prefix_attention_keys,
        prefix_attention_values,
        positions,
        visibility_mask,
    } = input;
    if hidden_width == 0
        || hidden_states.is_empty()
        || !hidden_states.len().is_multiple_of(hidden_width)
    {
        return Err(Error::Invalid("QSA prefill hidden-state shape mismatch"));
    }
    let token_count = hidden_states.len() / hidden_width;
    let prefix_tokens = prefix_positions.len();
    let total_tokens = prefix_tokens
        .checked_add(token_count)
        .ok_or(Error::Overflow)?;
    let visibility_elements = token_count
        .checked_mul(total_tokens)
        .ok_or(Error::Overflow)?;
    let kv_width = geometry
        .kv_heads
        .checked_mul(geometry.head_dim)
        .ok_or(Error::Overflow)?;
    if positions.len() != token_count
        || visibility_mask.len() != visibility_elements
        || prefix_raw_index_keys.len()
            != prefix_tokens
                .checked_mul(geometry.indexer_head_dim)
                .ok_or(Error::Overflow)?
        || prefix_attention_keys.len()
            != prefix_tokens.checked_mul(kv_width).ok_or(Error::Overflow)?
        || prefix_attention_values.len() != prefix_attention_keys.len()
        || indexer_rope.head_dim() != geometry.indexer_head_dim
        || attention_rope.head_dim() != geometry.head_dim
    {
        return Err(Error::Invalid(
            "QSA cached prefill cache or mask shape mismatch",
        ));
    }
    for token in 0..token_count {
        let visible_len = prefix_tokens
            .checked_add(token + 1)
            .ok_or(Error::Overflow)?;
        let row_start = token.checked_mul(total_tokens).ok_or(Error::Overflow)?;
        let row_end = row_start.checked_add(total_tokens).ok_or(Error::Overflow)?;
        if visibility_mask[row_start + visible_len..row_end]
            .iter()
            .any(|&visible| visible)
        {
            return Err(Error::Invalid("QSA prefill mask exposes a future token"));
        }
    }
    let query_width = geometry
        .query_heads
        .checked_mul(geometry.head_dim)
        .ok_or(Error::Overflow)?;
    if query_width == 0
        || !weights
            .attention
            .output_projection
            .len()
            .is_multiple_of(query_width)
    {
        return Err(Error::Invalid(
            "QSA prefill output projection shape mismatch",
        ));
    }
    let output_width = weights.attention.output_projection.len() / query_width;
    let output_elements = token_count
        .checked_mul(output_width)
        .ok_or(Error::Overflow)?;
    let mut outputs = Vec::new();
    outputs
        .try_reserve_exact(output_elements)
        .map_err(|_| Error::Capacity("QSA prefill outputs"))?;
    let mut selected_token_indices = Vec::new();
    selected_token_indices
        .try_reserve_exact(token_count)
        .map_err(|_| Error::Capacity("QSA prefill selection records"))?;
    let mut raw_index_key_cache = Vec::new();
    let mut attention_keys = Vec::new();
    let mut attention_values = Vec::new();
    raw_index_key_cache
        .try_reserve_exact(
            prefix_tokens
                .checked_add(token_count)
                .and_then(|tokens| tokens.checked_mul(geometry.indexer_head_dim))
                .ok_or(Error::Overflow)?,
        )
        .map_err(|_| Error::Capacity("QSA prefill index cache"))?;
    attention_keys
        .try_reserve_exact(total_tokens.checked_mul(kv_width).ok_or(Error::Overflow)?)
        .map_err(|_| Error::Capacity("QSA prefill attention key cache"))?;
    attention_values
        .try_reserve_exact(total_tokens.checked_mul(kv_width).ok_or(Error::Overflow)?)
        .map_err(|_| Error::Capacity("QSA prefill attention value cache"))?;
    raw_index_key_cache.extend_from_slice(prefix_raw_index_keys);
    attention_keys.extend_from_slice(prefix_attention_keys);
    attention_values.extend_from_slice(prefix_attention_values);
    let all_positions_len = prefix_tokens
        .checked_add(token_count)
        .ok_or(Error::Overflow)?;
    let mut all_positions = Vec::new();
    all_positions
        .try_reserve_exact(all_positions_len)
        .map_err(|_| Error::Capacity("QSA prefill positions"))?;
    all_positions.extend_from_slice(prefix_positions);
    all_positions.extend_from_slice(positions);

    for token in 0..token_count {
        let row_start = token.checked_mul(total_tokens).ok_or(Error::Overflow)?;
        let visible_len = prefix_tokens
            .checked_add(token + 1)
            .ok_or(Error::Overflow)?;
        let row_end = row_start.checked_add(visible_len).ok_or(Error::Overflow)?;
        let token_start = token.checked_mul(hidden_width).ok_or(Error::Overflow)?;
        let token_end = token_start
            .checked_add(hidden_width)
            .ok_or(Error::Overflow)?;
        let result = qsa_decode_step(
            &hidden_states[token_start..token_end],
            &raw_index_key_cache,
            &all_positions[..prefix_tokens + token],
            &visibility_mask[row_start..row_end],
            &attention_keys,
            &attention_values,
            geometry.indexer_query_heads,
            geometry.indexer_head_dim,
            geometry.query_heads,
            geometry.kv_heads,
            geometry.head_dim,
            geometry.compress_ratio,
            geometry.token_budget,
            geometry.epsilon,
            indexer_rope,
            attention_rope,
            positions[token],
            weights,
        )?;
        outputs.extend(result.output);
        selected_token_indices.push(result.selected_token_indices);
        raw_index_key_cache = result.raw_index_key_cache;
        attention_keys = result.attention_keys;
        attention_values = result.attention_values;
    }
    Ok(QsaPrefillOutput {
        outputs,
        selected_token_indices,
        raw_index_key_cache,
        attention_keys,
        attention_values,
    })
}

/// Runs one decode token through the QSA indexer and sparse main attention.
/// Both caches are returned as new vectors and remain unchanged on failure.
#[allow(clippy::too_many_arguments)]
pub fn qsa_decode_step(
    hidden: &[f32],
    past_raw_index_keys: &[f32],
    past_positions: &[u32],
    base_visible: &[bool],
    past_attention_keys: &[f32],
    past_attention_values: &[f32],
    indexer_query_heads: usize,
    indexer_head_dim: usize,
    query_heads: usize,
    kv_heads: usize,
    head_dim: usize,
    compress_ratio: usize,
    token_budget: usize,
    epsilon: f32,
    indexer_rope: &TextRotaryEmbedding,
    attention_rope: &TextRotaryEmbedding,
    position: u32,
    weights: QsaDecodeWeights<'_>,
) -> Result<QsaDecodeOutput> {
    if kv_heads == 0 || head_dim == 0 {
        return Err(Error::Invalid("QSA decode attention geometry mismatch"));
    }
    let kv_width = kv_heads.checked_mul(head_dim).ok_or(Error::Overflow)?;
    let past_attention_tokens = past_attention_keys
        .len()
        .checked_div(kv_width)
        .ok_or(Error::Overflow)?;
    if past_attention_tokens
        .checked_mul(kv_width)
        .ok_or(Error::Overflow)?
        != past_attention_keys.len()
        || past_positions.len() != past_attention_tokens
        || past_attention_values.len() != past_attention_keys.len()
        || base_visible.len()
            != past_attention_tokens
                .checked_add(1)
                .ok_or(Error::Overflow)?
    {
        return Err(Error::Invalid("QSA decode cache lengths mismatch"));
    }
    let indexer = qsa_indexer_step(
        hidden,
        past_raw_index_keys,
        past_positions,
        base_visible,
        weights.indexer.projection,
        weights.indexer.query_norm_offset,
        weights.indexer.key_norm_offset,
        indexer_query_heads,
        indexer_head_dim,
        compress_ratio,
        token_budget,
        epsilon,
        indexer_rope,
        position,
    )?;
    let mut selected_visible = Vec::new();
    selected_visible
        .try_reserve_exact(base_visible.len())
        .map_err(|_| Error::Capacity("QSA selected visibility"))?;
    selected_visible.resize(base_visible.len(), false);
    for token in &indexer.selected_token_indices {
        let Some(is_visible) = selected_visible.get_mut(*token) else {
            return Err(Error::Invalid("QSA selected token index is out of range"));
        };
        *is_visible = true;
    }
    let attention = qsa_attention_step(
        hidden,
        past_attention_keys,
        past_attention_values,
        &selected_visible,
        query_heads,
        kv_heads,
        head_dim,
        epsilon,
        attention_rope,
        position,
        weights.attention,
    )?;
    Ok(QsaDecodeOutput {
        output: attention.output,
        selected_token_indices: indexer.selected_token_indices,
        raw_index_key_cache: indexer.raw_key_cache,
        attention_keys: attention.keys,
        attention_values: attention.values,
    })
}

/// Row-backed one-token QSA decode with separate streamed indexer matrices.
#[allow(clippy::too_many_arguments)]
pub fn qsa_decode_step_from_rows<IQ, IK, AQ, AK, AV, AO>(
    hidden: &[f32],
    past_raw_index_keys: &[f32],
    past_positions: &[u32],
    base_visible: &[bool],
    past_attention_keys: &[f32],
    past_attention_values: &[f32],
    geometry: QsaDecodeGeometry,
    indexer_rope: &TextRotaryEmbedding,
    attention_rope: &TextRotaryEmbedding,
    position: u32,
    indexer_query_projection: &mut IQ,
    indexer_key_projection: &mut IK,
    indexer_query_norm_offset: &[f32],
    indexer_key_norm_offset: &[f32],
    attention_query_gate_projection: &mut AQ,
    attention_key_projection: &mut AK,
    attention_value_projection: &mut AV,
    attention_query_norm_offset: &[f32],
    attention_key_norm_offset: &[f32],
    attention_output_projection: &mut AO,
) -> Result<QsaDecodeOutput>
where
    IQ: LinearWeightRows,
    IK: LinearWeightRows,
    AQ: LinearWeightRows,
    AK: LinearWeightRows,
    AV: LinearWeightRows,
    AO: LinearWeightRows,
{
    if geometry.kv_heads == 0 || geometry.head_dim == 0 {
        return Err(Error::Invalid("QSA decode attention geometry mismatch"));
    }
    let kv_width = geometry
        .kv_heads
        .checked_mul(geometry.head_dim)
        .ok_or(Error::Overflow)?;
    let past_attention_tokens = past_attention_keys
        .len()
        .checked_div(kv_width)
        .ok_or(Error::Overflow)?;
    if past_attention_tokens
        .checked_mul(kv_width)
        .ok_or(Error::Overflow)?
        != past_attention_keys.len()
        || past_positions.len() != past_attention_tokens
        || past_attention_values.len() != past_attention_keys.len()
        || past_raw_index_keys.len()
            != past_positions
                .len()
                .checked_mul(geometry.indexer_head_dim)
                .ok_or(Error::Overflow)?
        || base_visible.len()
            != past_attention_tokens
                .checked_add(1)
                .ok_or(Error::Overflow)?
    {
        return Err(Error::Invalid("QSA decode cache lengths mismatch"));
    }
    let indexer = qsa_indexer_step_from_rows(
        hidden,
        past_raw_index_keys,
        past_positions,
        base_visible,
        indexer_query_projection,
        indexer_key_projection,
        indexer_query_norm_offset,
        indexer_key_norm_offset,
        geometry.indexer_query_heads,
        geometry.indexer_head_dim,
        geometry.compress_ratio,
        geometry.token_budget,
        geometry.epsilon,
        indexer_rope,
        position,
    )?;
    let mut selected_visible = Vec::new();
    selected_visible
        .try_reserve_exact(base_visible.len())
        .map_err(|_| Error::Capacity("QSA selected visibility"))?;
    selected_visible.resize(base_visible.len(), false);
    for &token in &indexer.selected_token_indices {
        let Some(is_visible) = selected_visible.get_mut(token) else {
            return Err(Error::Invalid("QSA selected token index is out of range"));
        };
        *is_visible = true;
    }
    let attention = qsa_attention_step_from_rows(
        hidden,
        past_attention_keys,
        past_attention_values,
        &selected_visible,
        geometry.query_heads,
        geometry.kv_heads,
        geometry.head_dim,
        geometry.epsilon,
        attention_rope,
        position,
        attention_query_gate_projection,
        attention_key_projection,
        attention_value_projection,
        attention_query_norm_offset,
        attention_key_norm_offset,
        attention_output_projection,
    )?;
    Ok(QsaDecodeOutput {
        output: attention.output,
        selected_token_indices: indexer.selected_token_indices,
        raw_index_key_cache: indexer.raw_key_cache,
        attention_keys: attention.keys,
        attention_values: attention.values,
    })
}

fn normalized_groups(input: &[f32], group_size: usize, epsilon: f32) -> Result<Vec<f32>> {
    if input.is_empty()
        || group_size == 0
        || !input.len().is_multiple_of(group_size)
        || !epsilon.is_finite()
        || epsilon <= 0.0
    {
        return Err(Error::Invalid(
            "invalid RMS normalization geometry or epsilon",
        ));
    }
    finite(input)?;
    let mut output = Vec::with_capacity(input.len());
    for group in input.chunks_exact(group_size) {
        let variance = group.iter().map(|x| x * x).sum::<f32>() / group_size as f32;
        if !variance.is_finite() {
            return Err(Error::Invalid("RMS variance overflow"));
        }
        let denominator = variance + epsilon;
        if !denominator.is_finite() {
            return Err(Error::Invalid("RMS denominator overflow"));
        }
        let inverse = denominator.sqrt().recip();
        output.extend(group.iter().map(|x| x * inverse));
    }
    finite(&output)?;
    Ok(output)
}

pub fn rms_norm(
    input: &[f32],
    weight: &[f32],
    group_size: usize,
    epsilon: f32,
) -> Result<Vec<f32>> {
    if input.len() != weight.len() {
        return Err(Error::Invalid("RMS weight shape mismatch"));
    }
    finite(weight)?;
    let mut output = normalized_groups(input, group_size, epsilon)?;
    for (x, w) in output.iter_mut().zip(weight) {
        *x *= 1.0 + w;
    }
    finite(&output)?;
    Ok(output)
}

/// The selected checkpoint uses sigmoid gating and direct (not offset) weights.
pub fn gated_rms_norm(
    input: &[f32],
    weight: &[f32],
    gate: &[f32],
    epsilon: f32,
) -> Result<Vec<f32>> {
    if input.len() != weight.len() || input.len() != gate.len() {
        return Err(Error::Invalid("gated RMS shape mismatch"));
    }
    finite(weight)?;
    finite(gate)?;
    let mut output = normalized_groups(input, input.len(), epsilon)?;
    for ((x, w), g) in output.iter_mut().zip(weight).zip(gate) {
        *x *= w * sigmoid(*g);
    }
    finite(&output)?;
    Ok(output)
}

/// Applies a dense PyTorch-order linear projection `[out_features, in_features]`.
pub fn linear(input: &[f32], weight: &[f32], bias: Option<&[f32]>) -> Result<Vec<f32>> {
    if input.is_empty() || weight.is_empty() || !weight.len().is_multiple_of(input.len()) {
        return Err(Error::Invalid("linear projection shape mismatch"));
    }
    let output_size = weight.len() / input.len();
    if bias.is_some_and(|values| values.len() != output_size) {
        return Err(Error::Invalid("linear bias shape mismatch"));
    }
    finite(input)?;
    finite(weight)?;
    if let Some(bias) = bias {
        finite(bias)?;
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_size)
        .map_err(|_| Error::Capacity("linear output"))?;
    for row in 0..output_size {
        let start = row * input.len();
        let mut sum = weight[start..start + input.len()]
            .iter()
            .zip(input)
            .map(|(weight, input)| weight * input)
            .sum::<f32>();
        if let Some(bias) = bias {
            sum += bias[row];
        }
        output.push(sum);
    }
    finite(&output)?;
    Ok(output)
}

/// Supplies one output-feature row at a time for a bounded linear projection.
pub trait LinearWeightRows {
    fn output_features(&self) -> u64;
    fn input_features(&self) -> usize;
    fn read_weight_row(&mut self, row: u64, output: &mut [f32]) -> Result<()>;
}

/// Applies `[out_features, in_features]` weights without materializing the matrix.
pub fn linear_from_rows<R: LinearWeightRows>(
    input: &[f32],
    weights: &mut R,
    bias: Option<&[f32]>,
) -> Result<Vec<f32>> {
    let output_size = usize::try_from(weights.output_features()).map_err(|_| Error::Overflow)?;
    if input.is_empty()
        || output_size == 0
        || weights.input_features() != input.len()
        || bias.is_some_and(|values| values.len() != output_size)
    {
        return Err(Error::Invalid(
            "row-backed linear projection shape mismatch",
        ));
    }
    finite(input)?;
    if let Some(bias) = bias {
        finite(bias)?;
    }
    let mut weight_row = Vec::new();
    weight_row
        .try_reserve_exact(input.len())
        .map_err(|_| Error::Capacity("linear weight row"))?;
    weight_row.resize(input.len(), 0.0);
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_size)
        .map_err(|_| Error::Capacity("linear output"))?;
    for row in 0..weights.output_features() {
        weights.read_weight_row(row, &mut weight_row)?;
        finite(&weight_row)?;
        let mut sum = weight_row
            .iter()
            .zip(input)
            .map(|(weight, input)| weight * input)
            .sum::<f32>();
        if let Some(bias) = bias {
            sum += bias[usize::try_from(row).map_err(|_| Error::Overflow)?];
        }
        output.push(sum);
    }
    finite(&output)?;
    Ok(output)
}

#[derive(Clone, Debug, PartialEq)]
pub struct GatedResidualOutput {
    pub mixed_input: Vec<f32>,
    pub hyper_input: Vec<f32>,
    /// One scale per residual stream; absent for the final combine-free mixer.
    pub injection_weights: Option<Vec<f32>>,
}

impl GatedResidualOutput {
    /// Adds a block result back into each original residual stream.
    pub fn inject(
        &self,
        block_output: &[f32],
        streams: usize,
        hidden_size: usize,
    ) -> Result<Vec<f32>> {
        let expected_width = streams.checked_mul(hidden_size).ok_or(Error::Overflow)?;
        let Some(injection_weights) = &self.injection_weights else {
            return Err(Error::Invalid("gated residual has no injection projection"));
        };
        if streams == 0
            || hidden_size == 0
            || self.hyper_input.len() != expected_width
            || injection_weights.len() != streams
            || block_output.len() != hidden_size
        {
            return Err(Error::Invalid("gated residual injection shape mismatch"));
        }
        finite(block_output)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(expected_width)
            .map_err(|_| Error::Capacity("gated residual output"))?;
        for (stream, scale) in injection_weights.iter().enumerate() {
            let start = stream * hidden_size;
            output.extend(
                self.hyper_input[start..start + hidden_size]
                    .iter()
                    .zip(block_output)
                    .map(|(residual, block)| residual + scale * block),
            );
        }
        finite(&output)?;
        Ok(output)
    }
}

/// Qwen's multi-stream gated residual input mixer. Weights use PyTorch linear
/// order: down `[rank, streams*hidden]`, up `[streams*hidden, rank]`, and
/// optional injection `[streams, streams*hidden]`.
#[allow(clippy::too_many_arguments)]
pub fn gated_residual(
    hyper_input: &[f32],
    norm_weight: &[f32],
    down_weight: &[f32],
    up_weight: &[f32],
    injection_weight: Option<&[f32]>,
    streams: usize,
    hidden_size: usize,
    rank: usize,
    epsilon: f32,
) -> Result<GatedResidualOutput> {
    let width = streams.checked_mul(hidden_size).ok_or(Error::Overflow)?;
    let down_size = rank.checked_mul(width).ok_or(Error::Overflow)?;
    let up_size = width.checked_mul(rank).ok_or(Error::Overflow)?;
    if streams == 0
        || hidden_size == 0
        || rank == 0
        || hyper_input.len() != width
        || norm_weight.len() != width
        || down_weight.len() != down_size
        || up_weight.len() != up_size
    {
        return Err(Error::Invalid("gated residual tensor shape mismatch"));
    }
    let injection_size = streams.checked_mul(width).ok_or(Error::Overflow)?;
    if injection_weight.is_some_and(|weight| weight.len() != injection_size) {
        return Err(Error::Invalid("gated residual injection shape mismatch"));
    }
    gated_residual_inner(
        hyper_input,
        norm_weight,
        down_weight,
        up_weight,
        injection_weight,
        streams,
        hidden_size,
        epsilon,
    )
}

/// Row-backed Qwen gated residual mixer. Large down/up/injection matrices stay
/// lazy while the low-rank activations and stream gates remain materialized.
#[allow(clippy::too_many_arguments)]
pub fn gated_residual_from_rows<D, U, I>(
    hyper_input: &[f32],
    norm_weight: &[f32],
    down_weight: &mut D,
    up_weight: &mut U,
    mut injection_weight: Option<&mut I>,
    streams: usize,
    hidden_size: usize,
    epsilon: f32,
) -> Result<GatedResidualOutput>
where
    D: LinearWeightRows,
    U: LinearWeightRows,
    I: LinearWeightRows,
{
    let width = streams.checked_mul(hidden_size).ok_or(Error::Overflow)?;
    let rank = usize::try_from(down_weight.output_features()).map_err(|_| Error::Overflow)?;
    if streams == 0
        || hidden_size == 0
        || rank == 0
        || hyper_input.len() != width
        || norm_weight.len() != width
        || down_weight.input_features() != width
        || up_weight.input_features() != rank
        || usize::try_from(up_weight.output_features()).ok() != Some(width)
        || injection_weight.as_ref().is_some_and(|weight| {
            weight.input_features() != width || weight.output_features() != streams as u64
        })
    {
        return Err(Error::Invalid(
            "row-backed gated residual tensor shape mismatch",
        ));
    }
    if !epsilon.is_finite() || epsilon <= 0.0 {
        return Err(Error::Invalid("invalid gated residual epsilon"));
    }
    finite(hyper_input)?;
    finite(norm_weight)?;
    let normalized = rms_norm(hyper_input, norm_weight, hidden_size, epsilon)?;
    let mut lowrank = linear_from_rows(&normalized, down_weight, None)?;
    for value in &mut lowrank {
        *value = silu(*value / streams as f32);
    }
    finite(&lowrank)?;
    let raw_gates = linear_from_rows(&lowrank, up_weight, None)?;
    let mut gates = Vec::new();
    gates
        .try_reserve_exact(raw_gates.len())
        .map_err(|_| Error::Capacity("row-backed gated residual gates"))?;
    gates.extend(raw_gates.into_iter().map(sigmoid));
    finite(&gates)?;
    let mut mixed_input = Vec::new();
    mixed_input
        .try_reserve_exact(hidden_size)
        .map_err(|_| Error::Capacity("row-backed gated residual input"))?;
    for column in 0..hidden_size {
        let mut sum = 0.0_f32;
        for stream in 0..streams {
            let index = stream * hidden_size + column;
            sum += gates[index] * normalized[index];
        }
        mixed_input.push(sum / streams as f32);
    }
    finite(&mixed_input)?;
    let injection_weights = injection_weight
        .as_mut()
        .map(|weight| {
            let mut values = linear_from_rows(&normalized, *weight, None)?;
            for value in &mut values {
                *value = 2.0 * sigmoid(*value / streams as f32);
            }
            finite(&values)?;
            Ok::<Vec<f32>, Error>(values)
        })
        .transpose()?;
    let mut residual_snapshot = Vec::new();
    residual_snapshot
        .try_reserve_exact(hyper_input.len())
        .map_err(|_| Error::Capacity("row-backed residual input snapshot"))?;
    residual_snapshot.extend_from_slice(hyper_input);
    Ok(GatedResidualOutput {
        mixed_input,
        hyper_input: residual_snapshot,
        injection_weights,
    })
}

/// One routed expert's fused gate/up projection and down projection.
#[derive(Clone, Copy, Debug)]
pub struct ExpertWeights<'a> {
    pub gate_up: &'a [f32],
    pub down: &'a [f32],
}

/// Applies one routed SwiGLU expert using streamed gate, up, and down matrices.
pub fn fused_swiglu_expert_from_rows<G, U, D>(
    input: &[f32],
    gate_weights: &mut G,
    up_weights: &mut U,
    down_weights: &mut D,
) -> Result<Vec<f32>>
where
    G: LinearWeightRows,
    U: LinearWeightRows,
    D: LinearWeightRows,
{
    let intermediate =
        usize::try_from(gate_weights.output_features()).map_err(|_| Error::Overflow)?;
    if input.is_empty()
        || intermediate == 0
        || gate_weights.input_features() != input.len()
        || up_weights.input_features() != input.len()
        || up_weights.output_features() != gate_weights.output_features()
        || down_weights.input_features() != intermediate
        || usize::try_from(down_weights.output_features()).ok() != Some(input.len())
    {
        return Err(Error::Invalid("row-backed expert tensor shape mismatch"));
    }
    let gate = linear_from_rows(input, gate_weights, None)?;
    let up = linear_from_rows(input, up_weights, None)?;
    let mut activation = Vec::new();
    activation
        .try_reserve_exact(intermediate)
        .map_err(|_| Error::Capacity("row-backed expert activation"))?;
    activation.extend(gate.into_iter().zip(up).map(|(gate, up)| silu(gate) * up));
    finite(&activation)?;
    linear_from_rows(&activation, down_weights, None)
}

fn fused_swiglu_expert(
    input: &[f32],
    weights: ExpertWeights<'_>,
    intermediate_size: usize,
) -> Result<Vec<f32>> {
    let fused_width = intermediate_size.checked_mul(2).ok_or(Error::Overflow)?;
    let fused_size = fused_width
        .checked_mul(input.len())
        .ok_or(Error::Overflow)?;
    let down_size = input
        .len()
        .checked_mul(intermediate_size)
        .ok_or(Error::Overflow)?;
    if intermediate_size == 0
        || weights.gate_up.len() != fused_size
        || weights.down.len() != down_size
    {
        return Err(Error::Invalid("routed expert tensor shape mismatch"));
    }
    let projection = linear(input, weights.gate_up, None)?;
    let mut activated = Vec::new();
    activated
        .try_reserve_exact(intermediate_size)
        .map_err(|_| Error::Capacity("routed expert activation"))?;
    for index in 0..intermediate_size {
        activated.push(silu(projection[index]) * projection[intermediate_size + index]);
    }
    finite(&activated)?;
    linear(&activated, weights.down, None)
}

/// Computes the routed experts, shared SwiGLU expert and shared sigmoid gate
/// for one hidden vector. Weight matrices follow PyTorch `[out, in]` order.
#[allow(clippy::too_many_arguments)]
pub fn moe_block(
    input: &[f32],
    router_weight: &[f32],
    experts: &[ExpertWeights<'_>],
    expert_intermediate_size: usize,
    top_k: usize,
    normalize_selected: bool,
    shared_gate_proj: &[f32],
    shared_up_proj: &[f32],
    shared_down_proj: &[f32],
    shared_expert_gate: &[f32],
    shared_intermediate_size: usize,
) -> Result<Vec<f32>> {
    if input.is_empty()
        || router_weight.is_empty()
        || !router_weight.len().is_multiple_of(input.len())
        || experts.len() != router_weight.len() / input.len()
        || shared_intermediate_size == 0
    {
        return Err(Error::Invalid("MoE block tensor shape mismatch"));
    }
    let router_logits = linear(input, router_weight, None)?;
    let routes = route(&router_logits, top_k, normalize_selected)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(input.len())
        .map_err(|_| Error::Capacity("MoE output"))?;
    output.resize(input.len(), 0.0);
    for route in routes {
        let expert = experts
            .get(route.expert)
            .ok_or(Error::Invalid("router selected a missing expert"))?;
        let contribution = fused_swiglu_expert(input, *expert, expert_intermediate_size)?;
        for (total, value) in output.iter_mut().zip(contribution) {
            *total += route.weight * value;
        }
    }
    finite(&output)?;

    let shared_gate_size = shared_intermediate_size
        .checked_mul(input.len())
        .ok_or(Error::Overflow)?;
    let shared_down_size = input
        .len()
        .checked_mul(shared_intermediate_size)
        .ok_or(Error::Overflow)?;
    if shared_gate_proj.len() != shared_gate_size
        || shared_up_proj.len() != shared_gate_size
        || shared_down_proj.len() != shared_down_size
        || shared_expert_gate.len() != input.len()
    {
        return Err(Error::Invalid("shared expert tensor shape mismatch"));
    }
    let gate = linear(input, shared_gate_proj, None)?;
    let up = linear(input, shared_up_proj, None)?;
    let mut activation = Vec::new();
    activation
        .try_reserve_exact(shared_intermediate_size)
        .map_err(|_| Error::Capacity("shared expert activation"))?;
    activation.extend(gate.into_iter().zip(up).map(|(gate, up)| silu(gate) * up));
    finite(&activation)?;
    let mut shared = linear(&activation, shared_down_proj, None)?;
    let shared_scale = sigmoid(linear(input, shared_expert_gate, None)?[0]);
    for (total, value) in output.iter_mut().zip(&mut shared) {
        *total += shared_scale * *value;
    }
    finite(&output)?;
    Ok(output)
}

/// Executes one routed MoE token using bounded row-backed expert matrices.
/// The provider opens only the selected expert's matrices for each route.
#[allow(clippy::too_many_arguments)]
pub fn moe_block_from_rows<F, R, G, U, D, SG, SU, SD, SE>(
    input: &[f32],
    router_weight: &mut R,
    top_k: usize,
    normalize_selected: bool,
    mut expert_rows: F,
    shared_gate_proj: &mut SG,
    shared_up_proj: &mut SU,
    shared_down_proj: &mut SD,
    shared_expert_gate: &mut SE,
) -> Result<Vec<f32>>
where
    F: FnMut(usize) -> Result<(G, U, D)>,
    R: LinearWeightRows,
    G: LinearWeightRows,
    U: LinearWeightRows,
    D: LinearWeightRows,
    SG: LinearWeightRows,
    SU: LinearWeightRows,
    SD: LinearWeightRows,
    SE: LinearWeightRows,
{
    if input.is_empty() || router_weight.input_features() != input.len() {
        return Err(Error::Invalid("row-backed MoE tensor shape mismatch"));
    }
    let expert_count =
        usize::try_from(router_weight.output_features()).map_err(|_| Error::Overflow)?;
    if expert_count == 0 {
        return Err(Error::Invalid("row-backed MoE has no experts"));
    }
    let router_logits = linear_from_rows(input, router_weight, None)?;
    let routes = route(&router_logits, top_k, normalize_selected)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(input.len())
        .map_err(|_| Error::Capacity("row-backed MoE output"))?;
    output.resize(input.len(), 0.0);
    for route in routes {
        if route.expert >= expert_count {
            return Err(Error::Invalid("router selected an invalid expert"));
        }
        let (mut gate, mut up, mut down) = expert_rows(route.expert)?;
        let contribution = fused_swiglu_expert_from_rows(input, &mut gate, &mut up, &mut down)?;
        for (total, value) in output.iter_mut().zip(contribution) {
            *total += route.weight * value;
        }
    }
    finite(&output)?;

    if shared_gate_proj.input_features() != input.len()
        || shared_up_proj.input_features() != input.len()
        || shared_up_proj.output_features() != shared_gate_proj.output_features()
        || shared_down_proj.input_features()
            != usize::try_from(shared_gate_proj.output_features()).map_err(|_| Error::Overflow)?
        || usize::try_from(shared_down_proj.output_features()).ok() != Some(input.len())
        || shared_expert_gate.input_features() != input.len()
        || shared_expert_gate.output_features() != 1
    {
        return Err(Error::Invalid("row-backed shared expert shape mismatch"));
    }
    let gate = linear_from_rows(input, shared_gate_proj, None)?;
    let up = linear_from_rows(input, shared_up_proj, None)?;
    let mut activation = Vec::new();
    activation
        .try_reserve_exact(gate.len())
        .map_err(|_| Error::Capacity("row-backed shared expert activation"))?;
    activation.extend(gate.into_iter().zip(up).map(|(gate, up)| silu(gate) * up));
    finite(&activation)?;
    let mut shared = linear_from_rows(&activation, shared_down_proj, None)?;
    let shared_scale = sigmoid(linear_from_rows(input, shared_expert_gate, None)?[0]);
    for (total, value) in output.iter_mut().zip(&mut shared) {
        *total += shared_scale * *value;
    }
    finite(&output)?;
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn gated_residual_inner(
    hyper_input: &[f32],
    norm_weight: &[f32],
    down_weight: &[f32],
    up_weight: &[f32],
    injection_weight: Option<&[f32]>,
    streams: usize,
    hidden_size: usize,
    epsilon: f32,
) -> Result<GatedResidualOutput> {
    let normalized = rms_norm(hyper_input, norm_weight, hidden_size, epsilon)?;
    let mut lowrank = linear(&normalized, down_weight, None)?;
    for value in &mut lowrank {
        *value = silu(*value / streams as f32);
    }
    finite(&lowrank)?;
    let raw_gates = linear(&lowrank, up_weight, None)?;
    let mut gates = Vec::new();
    gates
        .try_reserve_exact(raw_gates.len())
        .map_err(|_| Error::Capacity("gated residual gates"))?;
    gates.extend(raw_gates.into_iter().map(sigmoid));
    finite(&gates)?;
    let mut mixed_input = Vec::new();
    mixed_input
        .try_reserve_exact(hidden_size)
        .map_err(|_| Error::Capacity("gated residual mixed input"))?;
    for column in 0..hidden_size {
        let mut sum = 0.0_f32;
        for stream in 0..streams {
            let index = stream * hidden_size + column;
            sum += gates[index] * normalized[index];
        }
        mixed_input.push(sum / streams as f32);
    }
    finite(&mixed_input)?;
    let injection_weights = injection_weight
        .map(|weight| {
            let mut values = linear(&normalized, weight, None)?;
            for value in &mut values {
                *value = 2.0 * sigmoid(*value / streams as f32);
            }
            finite(&values)?;
            Ok::<Vec<f32>, Error>(values)
        })
        .transpose()?;
    let mut residual_input = Vec::new();
    residual_input
        .try_reserve_exact(hyper_input.len())
        .map_err(|_| Error::Capacity("gated residual input snapshot"))?;
    residual_input.extend_from_slice(hyper_input);
    Ok(GatedResidualOutput {
        mixed_input,
        hyper_input: residual_input,
        injection_weights,
    })
}

fn silu(value: f32) -> f32 {
    value * sigmoid(value)
}

fn sigmoid(x: f32) -> f32 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PleGateOutput {
    /// Unnormalized gated values, flattened as `[streams, hidden_size]`.
    pub gated_value: Vec<f32>,
    /// Groupwise RMS-normalized values consumed by the depthwise convolution.
    pub conv_input: Vec<f32>,
}

/// Applies the Qwen PLE per-stream gate after key/value projections and before
/// the causal depthwise convolution. This does not perform n-gram lookup,
/// projections, convolution, or convolution-state updates.
#[allow(clippy::too_many_arguments)]
pub fn ple_gate(
    key: &[f32],
    key_weight: &[f32],
    query: &[f32],
    query_weight: &[f32],
    value: &[f32],
    conv_weight: &[f32],
    hidden_size: usize,
    epsilon: f32,
) -> Result<PleGateOutput> {
    if hidden_size == 0
        || key.is_empty()
        || !key.len().is_multiple_of(hidden_size)
        || key.len() != query.len()
        || key.len() != key_weight.len()
        || key.len() != query_weight.len()
        || key.len() != conv_weight.len()
        || value.len() != hidden_size
    {
        return Err(Error::Invalid("PLE gate tensor shape mismatch"));
    }
    if !epsilon.is_finite() || epsilon <= 0.0 {
        return Err(Error::Invalid("invalid PLE gate normalization epsilon"));
    }
    let streams = key.len() / hidden_size;
    let key_norm = rms_norm(key, key_weight, hidden_size, epsilon)?;
    let query_norm = rms_norm(query, query_weight, hidden_size, epsilon)?;
    let mut gated_value = Vec::new();
    gated_value
        .try_reserve_exact(key.len())
        .map_err(|_| Error::Capacity("PLE gated values"))?;
    let scale = (hidden_size as f32).sqrt().recip();
    for stream in 0..streams {
        let start = stream * hidden_size;
        let end = start + hidden_size;
        let dot = key_norm[start..end]
            .iter()
            .zip(&query_norm[start..end])
            .map(|(key, query)| key * query)
            .sum::<f32>()
            * scale;
        if !dot.is_finite() {
            return Err(Error::Invalid("PLE gate dot product overflow"));
        }
        let sign = if dot > 0.0 {
            1.0
        } else if dot < 0.0 {
            -1.0
        } else {
            0.0
        };
        let signed_root = sign * dot.abs().max(1e-6).sqrt();
        let gate = sigmoid(signed_root);
        gated_value.extend(value.iter().map(|element| gate * element));
    }
    finite(&gated_value)?;
    let conv_input = rms_norm(&gated_value, conv_weight, hidden_size, epsilon)?;
    Ok(PleGateOutput {
        gated_value,
        conv_input,
    })
}

#[derive(Clone, Copy)]
pub struct PleLayerWeights<'a> {
    pub key_projection: &'a [f32],
    pub value_projection: &'a [f32],
    pub key_norm_offset: &'a [f32],
    pub query_norm_offset: &'a [f32],
    pub conv_norm_offset: &'a [f32],
    /// PyTorch depthwise convolution order `[channels, 1, kernel]`.
    pub conv_weights: &'a [f32],
}

/// One-token PLE reference from an already looked-up n-gram embedding through
/// projections, per-stream gating, and dilated depthwise convolution. The
/// convolution state commits only after the final residual sum is finite.
pub fn ple_step(
    embedding: &[f32],
    residual_streams: &[f32],
    hidden_size: usize,
    epsilon: f32,
    is_padding: bool,
    conv_state: &mut DepthwiseConvState,
    weights: PleLayerWeights<'_>,
) -> Result<Vec<f32>> {
    if embedding.is_empty()
        || hidden_size == 0
        || residual_streams.is_empty()
        || !residual_streams.len().is_multiple_of(hidden_size)
    {
        return Err(Error::Invalid("PLE step input geometry mismatch"));
    }
    let key = linear(embedding, weights.key_projection, None)?;
    let value = linear(embedding, weights.value_projection, None)?;
    if key.len() != residual_streams.len() || conv_state.channels != key.len() {
        return Err(Error::Invalid(
            "PLE step projection or state shape mismatch",
        ));
    }
    let mut gated = ple_gate(
        &key,
        weights.key_norm_offset,
        residual_streams,
        weights.query_norm_offset,
        &value,
        weights.conv_norm_offset,
        hidden_size,
        epsilon,
    )?;
    if is_padding {
        gated.gated_value.fill(0.0);
        gated.conv_input.fill(0.0);
    }
    let mut staged_state = conv_state.try_clone_state()?;
    let convolution = staged_state.process(&gated.conv_input, weights.conv_weights)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(gated.gated_value.len())
        .map_err(|_| Error::Capacity("PLE step output"))?;
    for (value, convolution) in gated.gated_value.iter().zip(convolution) {
        output.push(value + convolution);
    }
    finite(&output)?;
    *conv_state = staged_state;
    Ok(output)
}

/// One-token PLE step with bounded output-row readers for its key/value
/// projections. The caller owns those readers; convolution state commits only
/// after both projections and the final residual sum succeed.
#[allow(clippy::too_many_arguments)]
pub fn ple_step_from_rows<K, V>(
    embedding: &[f32],
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
    K: LinearWeightRows,
    V: LinearWeightRows,
{
    if embedding.is_empty()
        || hidden_size == 0
        || residual_streams.is_empty()
        || !residual_streams.len().is_multiple_of(hidden_size)
    {
        return Err(Error::Invalid("PLE step input geometry mismatch"));
    }
    let key = linear_from_rows(embedding, key_projection, None)?;
    let value = linear_from_rows(embedding, value_projection, None)?;
    if key.len() != residual_streams.len() || conv_state.channels != key.len() {
        return Err(Error::Invalid(
            "PLE step projection or state shape mismatch",
        ));
    }
    let mut gated = ple_gate(
        &key,
        key_norm_offset,
        residual_streams,
        query_norm_offset,
        &value,
        conv_norm_offset,
        hidden_size,
        epsilon,
    )?;
    if is_padding {
        gated.gated_value.fill(0.0);
        gated.conv_input.fill(0.0);
    }
    let mut staged_state = conv_state.try_clone_state()?;
    let convolution = staged_state.process(&gated.conv_input, conv_weights)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(gated.gated_value.len())
        .map_err(|_| Error::Capacity("PLE step output"))?;
    for (value, convolution) in gated.gated_value.iter().zip(convolution) {
        output.push(value + convolution);
    }
    finite(&output)?;
    *conv_state = staged_state;
    Ok(output)
}

/// Causal depthwise convolution state used by PLE's short convolution.
/// History is stored in time-major `[history, channels]` order.
#[derive(Clone, Debug, PartialEq)]
pub struct DepthwiseConvState {
    channels: usize,
    kernel_size: usize,
    dilation: usize,
    history: Vec<f32>,
}

impl DepthwiseConvState {
    pub fn new(channels: usize, kernel_size: usize, dilation: usize) -> Result<Self> {
        if channels == 0 || kernel_size == 0 || dilation == 0 {
            return Err(Error::Invalid("zero depthwise convolution dimension"));
        }
        let history_len = (kernel_size - 1)
            .checked_mul(dilation)
            .ok_or(Error::Overflow)?;
        let history_size = history_len.checked_mul(channels).ok_or(Error::Overflow)?;
        let mut history = Vec::new();
        history
            .try_reserve_exact(history_size)
            .map_err(|_| Error::Capacity("PLE convolution history"))?;
        history.resize(history_size, 0.0);
        Ok(Self {
            channels,
            kernel_size,
            dilation,
            history,
        })
    }

    pub fn history(&self) -> &[f32] {
        &self.history
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub(crate) fn try_clone_state(&self) -> Result<Self> {
        let mut history = Vec::new();
        history
            .try_reserve_exact(self.history.len())
            .map_err(|_| Error::Capacity("PLE convolution state snapshot"))?;
        history.extend_from_slice(&self.history);
        Ok(Self {
            channels: self.channels,
            kernel_size: self.kernel_size,
            dilation: self.dilation,
            history,
        })
    }

    /// Processes one or more time-major tokens and commits history only after
    /// every convolution result is finite. `weights` use PyTorch Conv1d order
    /// `[channels, 1, kernel_size]`, flattened to `[channels, kernel_size]`.
    pub fn process(&mut self, inputs: &[f32], weights: &[f32]) -> Result<Vec<f32>> {
        let weight_count = self
            .channels
            .checked_mul(self.kernel_size)
            .ok_or(Error::Overflow)?;
        if !inputs.len().is_multiple_of(self.channels) || weights.len() != weight_count {
            return Err(Error::Invalid("depthwise convolution shape mismatch"));
        }
        finite(inputs)?;
        finite(weights)?;
        let tokens = inputs.len() / self.channels;
        let history_tokens = self.history.len() / self.channels;
        let total_tokens = history_tokens.checked_add(tokens).ok_or(Error::Overflow)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(inputs.len())
            .map_err(|_| Error::Capacity("PLE convolution output"))?;
        for token in 0..tokens {
            for channel in 0..self.channels {
                let mut sum = 0.0_f32;
                for kernel in 0..self.kernel_size {
                    let time = token + kernel * self.dilation;
                    let sample = if time < history_tokens {
                        self.history[time * self.channels + channel]
                    } else {
                        let input_token = time - history_tokens;
                        inputs[input_token * self.channels + channel]
                    };
                    sum += sample * weights[channel * self.kernel_size + kernel];
                }
                if !sum.is_finite() {
                    return Err(Error::Invalid("depthwise convolution overflow"));
                }
                output.push(sum * sigmoid(sum));
            }
        }
        finite(&output)?;

        let keep_tokens = history_tokens.min(total_tokens);
        let keep_values = keep_tokens
            .checked_mul(self.channels)
            .ok_or(Error::Overflow)?;
        let mut next_history = Vec::new();
        next_history
            .try_reserve_exact(keep_values)
            .map_err(|_| Error::Capacity("PLE convolution history"))?;
        let skip_tokens = total_tokens - keep_tokens;
        for token in skip_tokens..total_tokens {
            if token < history_tokens {
                let start = token * self.channels;
                next_history.extend_from_slice(&self.history[start..start + self.channels]);
            } else {
                let input_token = token - history_tokens;
                let start = input_token * self.channels;
                next_history.extend_from_slice(&inputs[start..start + self.channels]);
            }
        }
        self.history = next_history;
        Ok(output)
    }
}

/// One value head's key-major state. Clone together with all other decoder state
/// when taking a speculative snapshot; this alone is not a decoder snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct DeltaState {
    key_width: usize,
    value_width: usize,
    values: Vec<f32>,
}

impl DeltaState {
    pub fn new(key_width: usize, value_width: usize) -> Result<Self> {
        if key_width == 0 || value_width == 0 {
            return Err(Error::Invalid("zero recurrent state dimension"));
        }
        let size = key_width.checked_mul(value_width).ok_or(Error::Overflow)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(size)
            .map_err(|_| Error::Capacity("recurrent state"))?;
        values.resize(size, 0.0);
        Ok(Self {
            key_width,
            value_width,
            values,
        })
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }

    fn try_clone_state(&self) -> Result<Self> {
        let mut values = Vec::new();
        values
            .try_reserve_exact(self.values.len())
            .map_err(|_| Error::Capacity("recurrent state snapshot"))?;
        values.extend_from_slice(&self.values);
        Ok(Self {
            key_width: self.key_width,
            value_width: self.value_width,
            values,
        })
    }

    /// One token, with raw q/k and already transformed log-decay and beta.
    /// Only commits state once the entire update and output are finite.
    pub fn step(
        &mut self,
        query: &[f32],
        key: &[f32],
        value: &[f32],
        log_decay: f32,
        beta: f32,
    ) -> Result<Vec<f32>> {
        if query.len() != self.key_width
            || key.len() != self.key_width
            || value.len() != self.value_width
        {
            return Err(Error::Invalid("recurrent vector shape mismatch"));
        }
        if !log_decay.is_finite()
            || log_decay > 0.0
            || !beta.is_finite()
            || !(0.0..=1.0).contains(&beta)
        {
            return Err(Error::Invalid("invalid recurrent decay or update gate"));
        }
        finite(value)?;
        let q = l2_normalize(query)?;
        let k = l2_normalize(key)?;
        let scale = (self.key_width as f32).sqrt().recip();
        let decay = log_decay.exp();
        let mut next: Vec<f32> = self.values.iter().map(|s| s * decay).collect();
        let mut output = vec![0.0; self.value_width];
        for column in 0..self.value_width {
            let predicted: f32 = k
                .iter()
                .enumerate()
                .map(|(row, k)| k * next[row * self.value_width + column])
                .sum();
            let delta = beta * (value[column] - predicted);
            for row in 0..self.key_width {
                let cell = &mut next[row * self.value_width + column];
                *cell += k[row] * delta;
                output[column] += q[row] * scale * *cell;
            }
        }
        finite(&next)?;
        finite(&output)?;
        self.values = next;
        Ok(output)
    }
}

/// Multi-head GatedDeltaNet recurrence. Key heads are repeated across
/// contiguous value-head groups as in the pinned Qwen implementation.
#[derive(Clone, Debug, PartialEq)]
pub struct GatedDeltaState {
    key_heads: usize,
    value_heads: usize,
    key_dim: usize,
    value_dim: usize,
    states: Vec<DeltaState>,
}

impl GatedDeltaState {
    pub fn new(
        key_heads: usize,
        value_heads: usize,
        key_dim: usize,
        value_dim: usize,
    ) -> Result<Self> {
        if key_heads == 0
            || value_heads == 0
            || key_dim == 0
            || value_dim == 0
            || !value_heads.is_multiple_of(key_heads)
        {
            return Err(Error::Invalid("GatedDeltaNet head geometry mismatch"));
        }
        let mut states = Vec::new();
        states
            .try_reserve_exact(value_heads)
            .map_err(|_| Error::Capacity("GatedDeltaNet head states"))?;
        for _ in 0..value_heads {
            states.push(DeltaState::new(key_dim, value_dim)?);
        }
        Ok(Self {
            key_heads,
            value_heads,
            key_dim,
            value_dim,
            states,
        })
    }

    pub fn states(&self) -> &[DeltaState] {
        &self.states
    }

    fn try_clone_state(&self) -> Result<Self> {
        let mut states = Vec::new();
        states
            .try_reserve_exact(self.states.len())
            .map_err(|_| Error::Capacity("GatedDeltaNet state snapshot"))?;
        for state in &self.states {
            states.push(state.try_clone_state()?);
        }
        Ok(Self {
            key_heads: self.key_heads,
            value_heads: self.value_heads,
            key_dim: self.key_dim,
            value_dim: self.value_dim,
            states,
        })
    }

    /// Advances every value head for one token. `log_decay` and `beta` are the
    /// already transformed per-value-head projections. All head states commit
    /// together, including failures discovered while processing a later head.
    pub fn step(
        &mut self,
        query: &[f32],
        key: &[f32],
        values: &[f32],
        log_decay: &[f32],
        beta: &[f32],
    ) -> Result<Vec<f32>> {
        let key_width = self
            .key_heads
            .checked_mul(self.key_dim)
            .ok_or(Error::Overflow)?;
        let value_width = self
            .value_heads
            .checked_mul(self.value_dim)
            .ok_or(Error::Overflow)?;
        if query.len() != key_width
            || key.len() != key_width
            || values.len() != value_width
            || log_decay.len() != self.value_heads
            || beta.len() != self.value_heads
        {
            return Err(Error::Invalid("GatedDeltaNet input shape mismatch"));
        }
        finite(query)?;
        finite(key)?;
        finite(values)?;
        finite(log_decay)?;
        finite(beta)?;
        if log_decay.iter().any(|&gate| gate > 0.0)
            || beta.iter().any(|&gate| !(0.0..=1.0).contains(&gate))
        {
            return Err(Error::Invalid("invalid GatedDeltaNet decay or beta"));
        }

        let mut staged = Vec::new();
        staged
            .try_reserve_exact(self.value_heads)
            .map_err(|_| Error::Capacity("GatedDeltaNet staged states"))?;
        for state in &self.states {
            staged.push(state.try_clone_state()?);
        }
        let mut output = Vec::new();
        output
            .try_reserve_exact(value_width)
            .map_err(|_| Error::Capacity("GatedDeltaNet output"))?;
        let repeats = self.value_heads / self.key_heads;
        for value_head in 0..self.value_heads {
            let key_head = value_head / repeats;
            let key_start = key_head * self.key_dim;
            let value_start = value_head * self.value_dim;
            let head_output = staged[value_head].step(
                &query[key_start..key_start + self.key_dim],
                &key[key_start..key_start + self.key_dim],
                &values[value_start..value_start + self.value_dim],
                log_decay[value_head],
                beta[value_head],
            )?;
            output.extend(head_output);
        }
        finite(&output)?;
        self.states = staged;
        Ok(output)
    }
}

/// Learned tensors for one Qwen GatedDeltaNet layer, in PyTorch linear order.
pub struct GatedDeltaNetWeights<'a> {
    pub qkv_projection: &'a [f32],
    pub z_projection: &'a [f32],
    pub beta_projection: &'a [f32],
    pub decay_projection: &'a [f32],
    /// `[qkv_channels, kernel_size]`, in depthwise Conv1d order.
    pub convolution: &'a [f32],
    pub a_log: &'a [f32],
    pub dt_bias: &'a [f32],
    pub norm: &'a [f32],
    pub output_projection: &'a [f32],
}

/// Stateful single-token Qwen GatedDeltaNet reference. Prefill is defined by
/// repeated calls in token order; batched/chunk kernels are intentionally not
/// implemented here.
#[derive(Clone, Debug, PartialEq)]
pub struct GatedDeltaNetState {
    hidden_size: usize,
    key_heads: usize,
    value_heads: usize,
    key_dim: usize,
    value_dim: usize,
    convolution: DepthwiseConvState,
    recurrence: GatedDeltaState,
}

impl GatedDeltaNetState {
    pub fn new(
        hidden_size: usize,
        key_heads: usize,
        value_heads: usize,
        key_dim: usize,
        value_dim: usize,
        kernel_size: usize,
    ) -> Result<Self> {
        if hidden_size == 0
            || key_heads == 0
            || value_heads == 0
            || key_dim == 0
            || value_dim == 0
            || !value_heads.is_multiple_of(key_heads)
        {
            return Err(Error::Invalid("GatedDeltaNet dimensions must be nonzero"));
        }
        let qkv_channels = key_heads
            .checked_mul(key_dim)
            .and_then(|width| width.checked_mul(2))
            .and_then(|width| {
                value_heads
                    .checked_mul(value_dim)
                    .and_then(|value_width| width.checked_add(value_width))
            })
            .ok_or(Error::Overflow)?;
        Ok(Self {
            hidden_size,
            key_heads,
            value_heads,
            key_dim,
            value_dim,
            convolution: DepthwiseConvState::new(qkv_channels, kernel_size, 1)?,
            recurrence: GatedDeltaState::new(key_heads, value_heads, key_dim, value_dim)?,
        })
    }

    pub fn convolution_history(&self) -> &[f32] {
        self.convolution.history()
    }

    pub fn recurrent_states(&self) -> &[DeltaState] {
        self.recurrence.states()
    }

    fn try_clone_state(&self) -> Result<Self> {
        Ok(Self {
            hidden_size: self.hidden_size,
            key_heads: self.key_heads,
            value_heads: self.value_heads,
            key_dim: self.key_dim,
            value_dim: self.value_dim,
            convolution: self.convolution.try_clone_state()?,
            recurrence: self.recurrence.try_clone_state()?,
        })
    }

    /// Runs projections, causal convolution + SiLU, per-head beta/decay,
    /// repeated-key recurrent attention, gated RMS normalization, and output
    /// projection for one token. State commits only after the output succeeds.
    pub fn step(
        &mut self,
        hidden: &[f32],
        weights: &GatedDeltaNetWeights<'_>,
        epsilon: f32,
    ) -> Result<Vec<f32>> {
        if hidden.len() != self.hidden_size {
            return Err(Error::Invalid("GatedDeltaNet hidden input shape mismatch"));
        }
        finite(hidden)?;
        let key_width = self
            .key_heads
            .checked_mul(self.key_dim)
            .ok_or(Error::Overflow)?;
        let value_width = self
            .value_heads
            .checked_mul(self.value_dim)
            .ok_or(Error::Overflow)?;
        let qkv_width = key_width
            .checked_mul(2)
            .and_then(|width| width.checked_add(value_width))
            .ok_or(Error::Overflow)?;
        if weights.qkv_projection.len()
            != qkv_width
                .checked_mul(self.hidden_size)
                .ok_or(Error::Overflow)?
            || weights.z_projection.len()
                != value_width
                    .checked_mul(self.hidden_size)
                    .ok_or(Error::Overflow)?
            || weights.beta_projection.len()
                != self
                    .value_heads
                    .checked_mul(self.hidden_size)
                    .ok_or(Error::Overflow)?
            || weights.decay_projection.len() != weights.beta_projection.len()
            || weights.a_log.len() != self.value_heads
            || weights.dt_bias.len() != self.value_heads
            || weights.norm.len() != self.value_dim
            || weights.output_projection.len()
                != self
                    .hidden_size
                    .checked_mul(value_width)
                    .ok_or(Error::Overflow)?
        {
            return Err(Error::Invalid("GatedDeltaNet weight shape mismatch"));
        }
        if !epsilon.is_finite() || epsilon <= 0.0 {
            return Err(Error::Invalid(
                "invalid GatedDeltaNet normalization epsilon",
            ));
        }

        let qkv = linear(hidden, weights.qkv_projection, None)?;
        let z = linear(hidden, weights.z_projection, None)?;
        let beta_logits = linear(hidden, weights.beta_projection, None)?;
        let decay_logits = linear(hidden, weights.decay_projection, None)?;
        let (log_decay, beta) =
            gated_delta_parameters(&decay_logits, &beta_logits, weights.a_log, weights.dt_bias)?;

        let mut next_convolution = self.convolution.try_clone_state()?;
        let convolved = next_convolution.process(&qkv, weights.convolution)?;
        let q_width = self.key_heads * self.key_dim;
        let v_width = self.value_heads * self.value_dim;
        let query = &convolved[..q_width];
        let key = &convolved[q_width..2 * q_width];
        let value = &convolved[2 * q_width..2 * q_width + v_width];
        let mut next_recurrence = self.recurrence.try_clone_state()?;
        let core = next_recurrence.step(query, key, value, &log_decay, &beta)?;
        let normalized = gated_rms_norm_grouped(&core, weights.norm, &z, self.value_dim, epsilon)?;
        let output = linear(&normalized, weights.output_projection, None)?;
        self.convolution = next_convolution;
        self.recurrence = next_recurrence;
        Ok(output)
    }

    /// Row-backed counterpart to `step`, keeping quantized GGUF matrices out
    /// of memory while preserving the same state-commit boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn step_from_rows<Q, Z, B, D, O>(
        &mut self,
        hidden: &[f32],
        qkv_projection: &mut Q,
        z_projection: &mut Z,
        beta_projection: &mut B,
        decay_projection: &mut D,
        convolution: &[f32],
        a_log: &[f32],
        dt_bias: &[f32],
        norm: &[f32],
        output_projection: &mut O,
        epsilon: f32,
    ) -> Result<Vec<f32>>
    where
        Q: LinearWeightRows,
        Z: LinearWeightRows,
        B: LinearWeightRows,
        D: LinearWeightRows,
        O: LinearWeightRows,
    {
        if hidden.len() != self.hidden_size {
            return Err(Error::Invalid("GatedDeltaNet hidden input shape mismatch"));
        }
        finite(hidden)?;
        let qkv_width = self.key_heads * self.key_dim * 2 + self.value_heads * self.value_dim;
        let value_width = self.value_heads * self.value_dim;
        if qkv_projection.input_features() != self.hidden_size
            || usize::try_from(qkv_projection.output_features()).map_err(|_| Error::Overflow)?
                != qkv_width
            || z_projection.input_features() != self.hidden_size
            || usize::try_from(z_projection.output_features()).map_err(|_| Error::Overflow)?
                != value_width
            || beta_projection.input_features() != self.hidden_size
            || usize::try_from(beta_projection.output_features()).map_err(|_| Error::Overflow)?
                != self.value_heads
            || decay_projection.input_features() != self.hidden_size
            || usize::try_from(decay_projection.output_features()).map_err(|_| Error::Overflow)?
                != self.value_heads
            || output_projection.input_features() != value_width
            || usize::try_from(output_projection.output_features()).map_err(|_| Error::Overflow)?
                != self.hidden_size
            || convolution.len() != qkv_width * self.convolution.kernel_size
            || a_log.len() != self.value_heads
            || dt_bias.len() != self.value_heads
            || norm.len() != self.value_dim
        {
            return Err(Error::Invalid("GatedDeltaNet weight shape mismatch"));
        }
        if !epsilon.is_finite() || epsilon <= 0.0 {
            return Err(Error::Invalid(
                "invalid GatedDeltaNet normalization epsilon",
            ));
        }
        finite(convolution)?;
        finite(a_log)?;
        finite(dt_bias)?;
        finite(norm)?;
        let qkv = linear_from_rows(hidden, qkv_projection, None)?;
        let z = linear_from_rows(hidden, z_projection, None)?;
        let beta_logits = linear_from_rows(hidden, beta_projection, None)?;
        let decay_logits = linear_from_rows(hidden, decay_projection, None)?;
        let (log_decay, beta) =
            gated_delta_parameters(&decay_logits, &beta_logits, a_log, dt_bias)?;
        let mut next_convolution = self.convolution.try_clone_state()?;
        let convolved = next_convolution.process(&qkv, convolution)?;
        let q_width = self.key_heads * self.key_dim;
        let v_width = self.value_heads * self.value_dim;
        let query = &convolved[..q_width];
        let key = &convolved[q_width..2 * q_width];
        let value = &convolved[2 * q_width..2 * q_width + v_width];
        let mut next_recurrence = self.recurrence.try_clone_state()?;
        let core = next_recurrence.step(query, key, value, &log_decay, &beta)?;
        let normalized = gated_rms_norm_grouped(&core, norm, &z, self.value_dim, epsilon)?;
        let output = linear_from_rows(&normalized, output_projection, None)?;
        self.convolution = next_convolution;
        self.recurrence = next_recurrence;
        Ok(output)
    }

    /// Processes a token-major chunk atomically using row-backed weights.
    #[allow(clippy::too_many_arguments)]
    pub fn process_from_rows<Q, Z, B, D, O>(
        &mut self,
        hidden_tokens: &[f32],
        qkv_projection: &mut Q,
        z_projection: &mut Z,
        beta_projection: &mut B,
        decay_projection: &mut D,
        convolution: &[f32],
        a_log: &[f32],
        dt_bias: &[f32],
        norm: &[f32],
        output_projection: &mut O,
        epsilon: f32,
    ) -> Result<Vec<f32>>
    where
        Q: LinearWeightRows,
        Z: LinearWeightRows,
        B: LinearWeightRows,
        D: LinearWeightRows,
        O: LinearWeightRows,
    {
        if !hidden_tokens.len().is_multiple_of(self.hidden_size) {
            return Err(Error::Invalid("GatedDeltaNet prefill shape mismatch"));
        }
        finite(hidden_tokens)?;
        if hidden_tokens.is_empty() {
            return Ok(Vec::new());
        }
        let mut staged = self.try_clone_state()?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(hidden_tokens.len())
            .map_err(|_| Error::Capacity("GatedDeltaNet row-backed prefill output"))?;
        for token in hidden_tokens.chunks_exact(self.hidden_size) {
            output.extend(staged.step_from_rows(
                token,
                qkv_projection,
                z_projection,
                beta_projection,
                decay_projection,
                convolution,
                a_log,
                dt_bias,
                norm,
                output_projection,
                epsilon,
            )?);
        }
        self.convolution = staged.convolution;
        self.recurrence = staged.recurrence;
        Ok(output)
    }

    /// Processes a token-major prefill chunk atomically. Results match
    /// repeated calls to `step`; on any failure, no token in the chunk commits.
    pub fn process(
        &mut self,
        hidden_tokens: &[f32],
        weights: &GatedDeltaNetWeights<'_>,
        epsilon: f32,
    ) -> Result<Vec<f32>> {
        if !hidden_tokens.len().is_multiple_of(self.hidden_size) {
            return Err(Error::Invalid("GatedDeltaNet prefill shape mismatch"));
        }
        finite(hidden_tokens)?;
        let tokens = hidden_tokens.len() / self.hidden_size;
        if tokens == 0 {
            return Ok(Vec::new());
        }
        let mut staged = self.try_clone_state()?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(hidden_tokens.len())
            .map_err(|_| Error::Capacity("GatedDeltaNet prefill output"))?;
        for token in hidden_tokens.chunks_exact(self.hidden_size) {
            output.extend(staged.step(token, weights, epsilon)?);
        }
        self.convolution = staged.convolution;
        self.recurrence = staged.recurrence;
        Ok(output)
    }
}

/// Returns Qwen's per-value-head `g = -exp(A_log)*softplus(a+dt_bias)` and
/// `beta = sigmoid(b)` transforms.
pub fn gated_delta_parameters(
    decay_logits: &[f32],
    beta_logits: &[f32],
    a_log: &[f32],
    dt_bias: &[f32],
) -> Result<(Vec<f32>, Vec<f32>)> {
    if decay_logits.is_empty()
        || decay_logits.len() != beta_logits.len()
        || decay_logits.len() != a_log.len()
        || decay_logits.len() != dt_bias.len()
    {
        return Err(Error::Invalid("GatedDeltaNet gate shape mismatch"));
    }
    finite(decay_logits)?;
    finite(beta_logits)?;
    finite(a_log)?;
    finite(dt_bias)?;
    let mut log_decay = Vec::new();
    let mut beta = Vec::new();
    log_decay
        .try_reserve_exact(decay_logits.len())
        .map_err(|_| Error::Capacity("GatedDeltaNet decay gates"))?;
    beta.try_reserve_exact(beta_logits.len())
        .map_err(|_| Error::Capacity("GatedDeltaNet beta gates"))?;
    for (((&a, &a_log), &bias), &b) in decay_logits.iter().zip(a_log).zip(dt_bias).zip(beta_logits)
    {
        let x = a + bias;
        let coefficient = a_log.exp();
        let softplus = x.max(0.0) + (-x.abs()).exp().ln_1p();
        let decay = -coefficient * softplus;
        if !x.is_finite() || !decay.is_finite() || decay > 0.0 {
            return Err(Error::Invalid("GatedDeltaNet decay transform overflow"));
        }
        log_decay.push(decay);
        beta.push(sigmoid(b));
    }
    finite(&beta)?;
    Ok((log_decay, beta))
}

/// Direct-weight sigmoid-gated RMS normalization over independent head groups.
pub fn gated_rms_norm_grouped(
    input: &[f32],
    weight: &[f32],
    gate: &[f32],
    group_size: usize,
    epsilon: f32,
) -> Result<Vec<f32>> {
    if input.len() != gate.len()
        || group_size == 0
        || !input.len().is_multiple_of(group_size)
        || weight.len() != group_size
    {
        return Err(Error::Invalid("grouped gated RMS shape mismatch"));
    }
    finite(weight)?;
    finite(gate)?;
    let mut output = normalized_groups(input, group_size, epsilon)?;
    for (index, value) in output.iter_mut().enumerate() {
        *value *= weight[index % group_size] * sigmoid(gate[index]);
    }
    finite(&output)?;
    Ok(output)
}

fn l2_normalize(input: &[f32]) -> Result<Vec<f32>> {
    finite(input)?;
    let squared: f32 = input.iter().map(|x| x * x).sum();
    if !squared.is_finite() {
        return Err(Error::Invalid("L2 norm overflow"));
    }
    let inverse = (squared + 1e-6).sqrt().recip();
    Ok(input.iter().map(|x| x * inverse).collect())
}

/// Reorders GatedDeltaNet value heads from key-grouped to tiled order.
///
/// Input is row-major `[key_group, value_head_per_group, head_dim, row_width]`;
/// output is `[value_head_per_group, key_group, head_dim, row_width]`.
pub fn reorder_gdn_value_heads_axis0(
    input: &[f32],
    key_groups: usize,
    value_heads_per_group: usize,
    head_dim: usize,
    row_width: usize,
) -> Result<Vec<f32>> {
    if key_groups == 0 || value_heads_per_group == 0 || head_dim == 0 || row_width == 0 {
        return Err(Error::Invalid(
            "invalid GatedDeltaNet head reorder geometry",
        ));
    }
    let values_per_head = head_dim.checked_mul(row_width).ok_or(Error::Overflow)?;
    let head_count = key_groups
        .checked_mul(value_heads_per_group)
        .ok_or(Error::Overflow)?;
    let expected = head_count
        .checked_mul(values_per_head)
        .ok_or(Error::Overflow)?;
    if input.len() != expected {
        return Err(Error::Invalid(
            "GatedDeltaNet head reorder input shape mismatch",
        ));
    }
    finite(input)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(expected)
        .map_err(|_| Error::Capacity("GatedDeltaNet reordered tensor"))?;
    output.resize(expected, 0.0);
    for group in 0..key_groups {
        for value_head in 0..value_heads_per_group {
            let source_head = group * value_heads_per_group + value_head;
            let target_head = value_head * key_groups + group;
            let source = source_head * values_per_head;
            let target = target_head * values_per_head;
            output[target..target + values_per_head]
                .copy_from_slice(&input[source..source + values_per_head]);
        }
    }
    finite(&output)?;
    Ok(output)
}

/// Reorders only the value rows of a fused `[Q | K | V]` projection matrix.
pub fn reorder_gdn_qkv_value_rows(
    input: &[f32],
    query_key_rows: usize,
    key_groups: usize,
    value_heads_per_group: usize,
    value_head_dim: usize,
    input_width: usize,
) -> Result<Vec<f32>> {
    if input_width == 0 {
        return Err(Error::Invalid(
            "GatedDeltaNet projection input width is zero",
        ));
    }
    let value_rows = key_groups
        .checked_mul(value_heads_per_group)
        .and_then(|count| count.checked_mul(value_head_dim))
        .ok_or(Error::Overflow)?;
    let total_rows = query_key_rows
        .checked_add(value_rows)
        .ok_or(Error::Overflow)?;
    let expected = total_rows.checked_mul(input_width).ok_or(Error::Overflow)?;
    if input.len() != expected {
        return Err(Error::Invalid("GatedDeltaNet QKV matrix shape mismatch"));
    }
    finite(input)?;
    let prefix_values = query_key_rows
        .checked_mul(input_width)
        .ok_or(Error::Overflow)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(expected)
        .map_err(|_| Error::Capacity("GatedDeltaNet reordered QKV matrix"))?;
    output.extend_from_slice(&input[..prefix_values]);
    output.extend(reorder_gdn_value_heads_axis0(
        &input[prefix_values..],
        key_groups,
        value_heads_per_group,
        value_head_dim,
        input_width,
    )?);
    Ok(output)
}

/// Reorders the input columns of a row-major GatedDeltaNet output projection.
pub fn reorder_gdn_value_head_columns(
    input: &[f32],
    output_rows: usize,
    key_groups: usize,
    value_heads_per_group: usize,
    head_dim: usize,
) -> Result<Vec<f32>> {
    if output_rows == 0 || key_groups == 0 || value_heads_per_group == 0 || head_dim == 0 {
        return Err(Error::Invalid(
            "invalid GatedDeltaNet output projection geometry",
        ));
    }
    let input_columns = key_groups
        .checked_mul(value_heads_per_group)
        .and_then(|count| count.checked_mul(head_dim))
        .ok_or(Error::Overflow)?;
    let expected = output_rows
        .checked_mul(input_columns)
        .ok_or(Error::Overflow)?;
    if input.len() != expected {
        return Err(Error::Invalid(
            "GatedDeltaNet output projection shape mismatch",
        ));
    }
    finite(input)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(expected)
        .map_err(|_| Error::Capacity("GatedDeltaNet reordered output projection"))?;
    output.resize(expected, 0.0);
    for row in 0..output_rows {
        let row_start = row * input_columns;
        for group in 0..key_groups {
            for value_head in 0..value_heads_per_group {
                for feature in 0..head_dim {
                    let source_column =
                        (group * value_heads_per_group + value_head) * head_dim + feature;
                    let target_column = (value_head * key_groups + group) * head_dim + feature;
                    output[row_start + target_column] = input[row_start + source_column];
                }
            }
        }
    }
    finite(&output)?;
    Ok(output)
}

/// Splits a fused indexer projection into its query and key row ranges.
pub fn split_indexer_qk_rows(
    input: &[f32],
    query_rows: usize,
    key_rows: usize,
    input_width: usize,
) -> Result<(Vec<f32>, Vec<f32>)> {
    if query_rows == 0 || key_rows == 0 || input_width == 0 {
        return Err(Error::Invalid("invalid indexer Q/K split geometry"));
    }
    let rows = query_rows.checked_add(key_rows).ok_or(Error::Overflow)?;
    let expected = rows.checked_mul(input_width).ok_or(Error::Overflow)?;
    if input.len() != expected {
        return Err(Error::Invalid("fused indexer Q/K matrix shape mismatch"));
    }
    finite(input)?;
    let query_values = query_rows.checked_mul(input_width).ok_or(Error::Overflow)?;
    let mut query = Vec::new();
    query
        .try_reserve_exact(query_values)
        .map_err(|_| Error::Capacity("indexer query projection"))?;
    query.extend_from_slice(&input[..query_values]);
    let mut key = Vec::new();
    key.try_reserve_exact(input.len() - query_values)
        .map_err(|_| Error::Capacity("indexer key projection"))?;
    key.extend_from_slice(&input[query_values..]);
    Ok((query, key))
}

/// Splits each expert's fused `[gate | up]` rows into separate stacked tensors.
pub fn split_expert_gate_up(
    input: &[f32],
    experts: usize,
    intermediate_width: usize,
    input_width: usize,
) -> Result<(Vec<f32>, Vec<f32>)> {
    if experts == 0 || intermediate_width == 0 || input_width == 0 {
        return Err(Error::Invalid("invalid expert gate/up split geometry"));
    }
    let half_size = intermediate_width
        .checked_mul(input_width)
        .ok_or(Error::Overflow)?;
    let per_expert = half_size.checked_mul(2).ok_or(Error::Overflow)?;
    let expected = experts.checked_mul(per_expert).ok_or(Error::Overflow)?;
    if input.len() != expected {
        return Err(Error::Invalid("fused expert gate/up tensor shape mismatch"));
    }
    finite(input)?;
    let mut gate = Vec::new();
    let mut up = Vec::new();
    gate.try_reserve_exact(experts.checked_mul(half_size).ok_or(Error::Overflow)?)
        .map_err(|_| Error::Capacity("expert gate tensor"))?;
    up.try_reserve_exact(experts.checked_mul(half_size).ok_or(Error::Overflow)?)
        .map_err(|_| Error::Capacity("expert up tensor"))?;
    for expert in 0..experts {
        let start = expert.checked_mul(per_expert).ok_or(Error::Overflow)?;
        gate.extend_from_slice(&input[start..start + half_size]);
        up.extend_from_slice(&input[start + half_size..start + per_expert]);
    }
    Ok((gate, up))
}

/// Applies the converter's zero-centered normalization offset.
pub fn add_one_to_norm(input: &[f32]) -> Result<Vec<f32>> {
    if input.is_empty() {
        return Err(Error::Invalid("normalization tensor is empty"));
    }
    finite(input)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(input.len())
        .map_err(|_| Error::Capacity("offset normalization tensor"))?;
    output.extend(input.iter().map(|value| value + 1.0));
    finite(&output)?;
    Ok(output)
}

/// Validates and flattens PLE Conv1d weights from `[channels, 1, kernel]`.
pub fn squeeze_ple_convolution(
    input: &[f32],
    channels: usize,
    singleton_width: usize,
    kernel_size: usize,
) -> Result<Vec<f32>> {
    if channels == 0 || singleton_width != 1 || kernel_size == 0 {
        return Err(Error::Invalid("invalid PLE convolution squeeze geometry"));
    }
    let expected = channels.checked_mul(kernel_size).ok_or(Error::Overflow)?;
    if input.len() != expected {
        return Err(Error::Invalid("PLE convolution tensor shape mismatch"));
    }
    finite(input)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(expected)
        .map_err(|_| Error::Capacity("squeezed PLE convolution tensor"))?;
    output.extend_from_slice(input);
    Ok(output)
}
pub mod source {
    //! Agentix-owned source tensor mapping and dense CPU transform dispatch.

    use super::{
        add_one_to_norm, reorder_gdn_qkv_value_rows, reorder_gdn_value_head_columns,
        reorder_gdn_value_heads_axis0, split_expert_gate_up, split_indexer_qk_rows,
        squeeze_ple_convolution,
    };
    use crate::{
        config::QwenTextConfig,
        schema::{map_qwen4exp_source_tensor, SourceTensorTransform},
        Error, Result,
    };

    /// One mapped float tensor in source row-major dimension order.
    #[derive(Clone, Debug, PartialEq)]
    pub struct ConvertedTensor {
        pub name: String,
        pub dimensions: Vec<usize>,
        pub values: Vec<f32>,
    }

    /// Exact integer PLE constants for a later native GGUF metadata writer.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PleHashMetadataKind {
        LayerMultipliers,
        HeadOffsets,
        HeadVocabSizes,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ConvertedPleHashMetadata {
        pub kind: PleHashMetadataKind,
        pub values: Vec<i64>,
    }

    /// Checks the three converted PLE arrays agree on table layout and config.
    pub fn validate_qwen4exp_ple_hash_metadata(
        arrays: &[ConvertedPleHashMetadata],
        config: &QwenTextConfig,
    ) -> Result<()> {
        let array_for = |kind| {
            arrays
                .iter()
                .find(|array| array.kind == kind)
                .map(|array| array.values.as_slice())
                .ok_or(Error::Invalid("PLE hash metadata array is missing"))
        };
        if arrays.len() != 3 {
            return Err(Error::Invalid("PLE hash metadata array count mismatch"));
        }
        let multipliers = array_for(PleHashMetadataKind::LayerMultipliers)?;
        let offsets = array_for(PleHashMetadataKind::HeadOffsets)?;
        let sizes = array_for(PleHashMetadataKind::HeadVocabSizes)?;
        let head_count = config
            .ngram_size
            .checked_sub(1)
            .and_then(|sizes| sizes.checked_mul(config.heads_per_ngram))
            .ok_or(Error::Overflow)?;
        if multipliers.len() != config.ngram_size
            || offsets.len() != head_count
            || sizes.len() != head_count
            || multipliers
                .iter()
                .any(|value| *value <= 0 || value % 2 == 0)
        {
            return Err(Error::Invalid("PLE hash metadata values are invalid"));
        }
        let mut end = 0_i64;
        for (&offset, &size) in offsets.iter().zip(sizes) {
            if offset != end || size <= 0 {
                return Err(Error::Invalid("PLE hash metadata head ranges are invalid"));
            }
            end = end.checked_add(size).ok_or(Error::Overflow)?;
        }
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
        let matches_layout = |actual: &[i64], expected: &[u64]| {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(&actual, &expected)| u64::try_from(actual).ok() == Some(expected))
        };
        if u64::try_from(end).map_err(|_| Error::Invalid("PLE hash metadata extent is invalid"))?
            != ngram.total_rows()
            || !matches_layout(multipliers, ngram.hash_multipliers())
            || !matches_layout(offsets, ngram.head_offsets())
            || !matches_layout(sizes, ngram.head_vocab_sizes())
        {
            return Err(Error::Invalid("PLE hash metadata table extent mismatch"));
        }
        Ok(())
    }

    /// Maps one pinned PLE int64 checkpoint vector while preserving every bit.
    /// The resulting typed value still requires emission as GGUF integer-array
    /// metadata; it must never pass through an FP32 tensor writer.
    pub fn transform_qwen4exp_i64_metadata(
        source_name: &str,
        source_dimensions: &[usize],
        source_values: &[i64],
        config: &QwenTextConfig,
    ) -> Result<ConvertedPleHashMetadata> {
        let mapping = map_qwen4exp_source_tensor(source_name, config)?
            .ok_or(Error::Invalid("unsupported Qwen source tensor name"))?;
        if mapping.transform != SourceTensorTransform::PLEHashMetadata
            || !mapping.gguf_names.is_empty()
            || source_dimensions.len() != 1
            || source_dimensions[0] != source_values.len()
        {
            return Err(Error::Invalid("PLE hash metadata geometry mismatch"));
        }
        let (kind, expected_len, positive) =
            if source_name.ends_with("ple_embedding.layer_multipliers") {
                (
                    PleHashMetadataKind::LayerMultipliers,
                    config.ngram_size,
                    true,
                )
            } else if source_name.ends_with("ple_embedding.ngram_heads_offsets") {
                (
                    PleHashMetadataKind::HeadOffsets,
                    config
                        .ngram_size
                        .checked_sub(1)
                        .and_then(|sizes| sizes.checked_mul(config.heads_per_ngram))
                        .ok_or(Error::Overflow)?,
                    false,
                )
            } else if source_name.ends_with("ple_embedding.ngram_heads_vocab_sizes") {
                (
                    PleHashMetadataKind::HeadVocabSizes,
                    config
                        .ngram_size
                        .checked_sub(1)
                        .and_then(|sizes| sizes.checked_mul(config.heads_per_ngram))
                        .ok_or(Error::Overflow)?,
                    true,
                )
            } else {
                return Err(Error::Invalid("unknown PLE hash metadata vector"));
            };
        if source_dimensions != [expected_len]
            || source_values
                .iter()
                .any(|&value| if positive { value <= 0 } else { value < 0 })
        {
            return Err(Error::Invalid("PLE hash metadata values are invalid"));
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(source_values.len())
            .map_err(|_| Error::Capacity("PLE hash metadata"))?;
        values.extend_from_slice(source_values);
        Ok(ConvertedPleHashMetadata { kind, values })
    }

    fn element_count(dimensions: &[usize]) -> Result<usize> {
        if dimensions.is_empty() || dimensions.contains(&0) {
            return Err(Error::Invalid("source tensor has empty geometry"));
        }
        dimensions.iter().try_fold(1_usize, |count, &dimension| {
            count.checked_mul(dimension).ok_or(Error::Overflow)
        })
    }

    fn mapped_tensor(
        name: &str,
        dimensions: &[usize],
        values: Vec<f32>,
    ) -> Result<ConvertedTensor> {
        if element_count(dimensions)? != values.len() {
            return Err(Error::Invalid(
                "converted tensor shape does not match values",
            ));
        }
        Ok(ConvertedTensor {
            name: name.to_owned(),
            dimensions: dimensions.to_vec(),
            values,
        })
    }

    fn push_tensor(
        output: &mut Vec<ConvertedTensor>,
        name: &str,
        dimensions: &[usize],
        values: Vec<f32>,
    ) -> Result<()> {
        output
            .try_reserve(1)
            .map_err(|_| Error::Capacity("converted source tensor list"))?;
        output.push(mapped_tensor(name, dimensions, values)?);
        Ok(())
    }

    fn gdn_geometry(config: &QwenTextConfig) -> Result<(usize, usize)> {
        if config.gdn_key_heads == 0
            || config.gdn_value_heads == 0
            || !config.gdn_value_heads.is_multiple_of(config.gdn_key_heads)
            || config.gdn_key_head_dim == 0
            || config.gdn_value_head_dim == 0
            || config.hidden_size == 0
        {
            return Err(Error::Invalid("invalid GatedDeltaNet source geometry"));
        }
        Ok((
            config.gdn_key_heads,
            config.gdn_value_heads / config.gdn_key_heads,
        ))
    }

    /// Maps and transforms one decoded float32 text-checkpoint tensor.
    ///
    /// The returned dimensions stay in source/PyTorch order. GGUF dimension
    /// reversal and quantization are later loader stages. Metadata constants must
    /// retain their integer dtype and PLE row shards are consumed by the indexed,
    /// file-backed PLE reader, so those two transforms are rejected here.
    pub fn transform_qwen4exp_f32_tensor(
        source_name: &str,
        source_dimensions: &[usize],
        source_values: &[f32],
        config: &QwenTextConfig,
    ) -> Result<Vec<ConvertedTensor>> {
        if element_count(source_dimensions)? != source_values.len() {
            return Err(Error::Invalid("source tensor shape does not match values"));
        }
        if source_values.iter().any(|value| !value.is_finite()) {
            return Err(Error::Invalid("source tensor contains non-finite values"));
        }
        let mapping = map_qwen4exp_source_tensor(source_name, config)?
            .ok_or(Error::Invalid("unsupported Qwen source tensor name"))?;
        let mut output = Vec::new();
        match mapping.transform {
            SourceTensorTransform::Direct => {
                if mapping.gguf_names.len() != 1 {
                    return Err(Error::Invalid("direct Qwen mapping has invalid fan-out"));
                }
                push_tensor(
                    &mut output,
                    &mapping.gguf_names[0],
                    source_dimensions,
                    copy_values(source_values, "mapped source tensor")?,
                )?;
            }
            SourceTensorTransform::SplitIndexerQueryKey => {
                let query_rows = config
                    .indexer_query_heads
                    .checked_mul(config.indexer_head_dim)
                    .ok_or(Error::Overflow)?;
                let key_rows = config
                    .indexer_kv_heads
                    .checked_mul(config.indexer_head_dim)
                    .ok_or(Error::Overflow)?;
                let total_rows = query_rows.checked_add(key_rows).ok_or(Error::Overflow)?;
                if source_dimensions.len() != 2
                    || source_dimensions[0] != total_rows
                    || source_dimensions[1] != config.hidden_size
                    || mapping.gguf_names.len() != 2
                {
                    return Err(Error::Invalid("fused indexer source geometry mismatch"));
                }
                let (query, key) =
                    split_indexer_qk_rows(source_values, query_rows, key_rows, config.hidden_size)?;
                push_tensor(
                    &mut output,
                    &mapping.gguf_names[0],
                    &[query_rows, config.hidden_size],
                    query,
                )?;
                push_tensor(
                    &mut output,
                    &mapping.gguf_names[1],
                    &[key_rows, config.hidden_size],
                    key,
                )?;
            }
            SourceTensorTransform::SplitExpertGateUp => {
                let expected_intermediate = config
                    .expert_intermediate_size
                    .checked_mul(2)
                    .ok_or(Error::Overflow)?;
                if source_dimensions
                    != [
                        config.expert_count,
                        expected_intermediate,
                        config.hidden_size,
                    ]
                    || mapping.gguf_names.len() != 2
                {
                    return Err(Error::Invalid("fused expert source geometry mismatch"));
                }
                let (gate, up) = split_expert_gate_up(
                    source_values,
                    config.expert_count,
                    config.expert_intermediate_size,
                    config.hidden_size,
                )?;
                let output_shape = [
                    config.expert_count,
                    config.expert_intermediate_size,
                    config.hidden_size,
                ];
                push_tensor(&mut output, &mapping.gguf_names[0], &output_shape, gate)?;
                push_tensor(&mut output, &mapping.gguf_names[1], &output_shape, up)?;
            }
            SourceTensorTransform::AddOneToNorm => {
                if source_dimensions.len() != 1 {
                    return Err(Error::Invalid("Qwen norm source tensor must be a vector"));
                }
                push_tensor(
                    &mut output,
                    single_target(&mapping.gguf_names)?,
                    source_dimensions,
                    add_one_to_norm(source_values)?,
                )?;
            }
            SourceTensorTransform::SqueezePleConvolution => {
                if source_dimensions.len() != 3
                    || source_dimensions[0]
                        != config
                            .hidden_size
                            .checked_mul(config.residual_streams)
                            .ok_or(Error::Overflow)?
                    || source_dimensions[1] != 1
                    || source_dimensions[2] != config.ple_conv_kernel_size
                {
                    return Err(Error::Invalid("PLE convolution source geometry mismatch"));
                }
                let dimensions = [source_dimensions[0], source_dimensions[2]];
                push_tensor(
                    &mut output,
                    single_target(&mapping.gguf_names)?,
                    &dimensions,
                    squeeze_ple_convolution(source_values, dimensions[0], 1, dimensions[1])?,
                )?;
            }
            SourceTensorTransform::ReorderGatedDeltaValueHeads => {
                reorder_gdn_tensor(
                    source_name,
                    source_dimensions,
                    source_values,
                    config,
                    single_target(&mapping.gguf_names)?,
                    &mut output,
                )?;
            }
            SourceTensorTransform::ConcatenatePleRowShard => {
                return Err(Error::Invalid(
                    "PLE row shards must use the indexed file-backed PLE reader",
                ));
            }
            SourceTensorTransform::PLEHashMetadata => {
                return Err(Error::Invalid(
                    "PLE hash constants require an integer-preserving metadata path",
                ));
            }
        }
        Ok(output)
    }

    fn single_target(names: &[String]) -> Result<&str> {
        if names.len() != 1 {
            return Err(Error::Invalid("Qwen mapping must have one tensor target"));
        }
        Ok(&names[0])
    }

    fn copy_values(values: &[f32], label: &'static str) -> Result<Vec<f32>> {
        let mut copy = Vec::new();
        copy.try_reserve_exact(values.len())
            .map_err(|_| Error::Capacity(label))?;
        copy.extend_from_slice(values);
        Ok(copy)
    }

    fn reorder_gdn_tensor(
        source_name: &str,
        dimensions: &[usize],
        values: &[f32],
        config: &QwenTextConfig,
        target: &str,
        output: &mut Vec<ConvertedTensor>,
    ) -> Result<()> {
        let (key_groups, value_heads_per_group) = gdn_geometry(config)?;
        let value_rows = config
            .gdn_value_heads
            .checked_mul(config.gdn_value_head_dim)
            .ok_or(Error::Overflow)?;
        let qk_rows = key_groups
            .checked_mul(config.gdn_key_head_dim)
            .and_then(|rows| rows.checked_mul(2))
            .ok_or(Error::Overflow)?;
        let qkv_rows = qk_rows.checked_add(value_rows).ok_or(Error::Overflow)?;
        let suffix = source_name.strip_suffix(".weight").unwrap_or(source_name);

        if suffix.ends_with("linear_attn.in_proj_qkv") {
            if dimensions != [qkv_rows, config.hidden_size] {
                return Err(Error::Invalid("GatedDeltaNet QKV source geometry mismatch"));
            }
            push_tensor(
                output,
                target,
                dimensions,
                reorder_gdn_qkv_value_rows(
                    values,
                    qk_rows,
                    key_groups,
                    value_heads_per_group,
                    config.gdn_value_head_dim,
                    config.hidden_size,
                )?,
            )
        } else if suffix.ends_with("linear_attn.in_proj_z") {
            if dimensions != [value_rows, config.hidden_size] {
                return Err(Error::Invalid("GatedDeltaNet Z source geometry mismatch"));
            }
            push_tensor(
                output,
                target,
                dimensions,
                reorder_gdn_value_heads_axis0(
                    values,
                    key_groups,
                    value_heads_per_group,
                    config.gdn_value_head_dim,
                    config.hidden_size,
                )?,
            )
        } else if suffix.ends_with("linear_attn.in_proj_a")
            || suffix.ends_with("linear_attn.in_proj_b")
        {
            if dimensions != [config.gdn_value_heads, config.hidden_size] {
                return Err(Error::Invalid(
                    "GatedDeltaNet alpha/beta source geometry mismatch",
                ));
            }
            push_tensor(
                output,
                target,
                dimensions,
                reorder_gdn_value_heads_axis0(
                    values,
                    key_groups,
                    value_heads_per_group,
                    1,
                    config.hidden_size,
                )?,
            )
        } else if suffix.ends_with("linear_attn.A_log") || suffix.ends_with("linear_attn.dt_bias") {
            if dimensions != [config.gdn_value_heads] {
                return Err(Error::Invalid(
                    "GatedDeltaNet decay vector geometry mismatch",
                ));
            }
            let reordered_values = if suffix.ends_with("linear_attn.A_log") {
                let mut transformed = Vec::new();
                transformed
                    .try_reserve_exact(values.len())
                    .map_err(|_| Error::Capacity("GatedDeltaNet A_log transform"))?;
                transformed.extend(values.iter().map(|value| -value.exp()));
                reorder_gdn_value_heads_axis0(
                    &transformed,
                    key_groups,
                    value_heads_per_group,
                    1,
                    1,
                )?
            } else {
                reorder_gdn_value_heads_axis0(values, key_groups, value_heads_per_group, 1, 1)?
            };
            push_tensor(output, target, dimensions, reordered_values)
        } else if suffix.ends_with("linear_attn.out_proj") {
            if dimensions != [config.hidden_size, value_rows] {
                return Err(Error::Invalid(
                    "GatedDeltaNet output source geometry mismatch",
                ));
            }
            push_tensor(
                output,
                target,
                dimensions,
                reorder_gdn_value_head_columns(
                    values,
                    config.hidden_size,
                    key_groups,
                    value_heads_per_group,
                    config.gdn_value_head_dim,
                )?,
            )
        } else if suffix.ends_with("linear_attn.conv1d") {
            let channels = qk_rows.checked_add(value_rows).ok_or(Error::Overflow)?;
            if dimensions != [channels, 1, config.gdn_conv_kernel_size] {
                return Err(Error::Invalid(
                    "GatedDeltaNet convolution source geometry mismatch",
                ));
            }
            let kernel_size = config.gdn_conv_kernel_size;
            let split_values = qk_rows.checked_mul(kernel_size).ok_or(Error::Overflow)?;
            let mut reordered = copy_values(&values[..split_values], "GatedDeltaNet convolution")?;
            let reordered_values = reorder_gdn_value_heads_axis0(
                &values[split_values..],
                key_groups,
                value_heads_per_group,
                1,
                kernel_size,
            )?;
            reordered
                .try_reserve_exact(reordered_values.len())
                .map_err(|_| Error::Capacity("GatedDeltaNet convolution"))?;
            reordered.extend(reordered_values);
            push_tensor(output, target, &[channels, kernel_size], reordered)
        } else {
            Err(Error::Invalid(
                "unsupported GatedDeltaNet tensor for head reorder",
            ))
        }
    }
}
