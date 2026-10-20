use agentix_qwen::{
    budget::ContextAdmission,
    config::QwenTextConfig,
    reference::{
        linear_from_rows, qsa_decode_step_from_rows, QsaDecodeGeometry, TextRotaryEmbedding,
    },
    tokenizer::{PreparedPrompt, PromptRequest, QwenTokenizer},
    Error as QwenError,
};
use agentix_qwen_sys::{
    GatedDeltaNetGgufLayer, MoeGgufLayer, PleGgufLayer, QsaGgufLayer, QwenGgufDecoder,
    QwenGgufTensorStore, ResidualMixerGguf, ResidualMixerKind,
};
use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fs,
    io::{self, Write},
    path::PathBuf,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let config_path = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("usage: validate_gguf_sample_rows <qwen-config.json> <gguf-shard>...")?;
    let paths = arguments.map(PathBuf::from).collect::<Vec<_>>();
    if paths.is_empty() {
        return Err("at least one GGUF shard is required".into());
    }
    let config = QwenTextConfig::parse(&fs::read(config_path)?)
        .map_err(|_| QwenError::Invalid("failed to parse Qwen text config"))?;
    let set = QwenGgufTensorStore::open(&paths, &config)?;
    let mut sampled = BTreeSet::new();
    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    writeln!(output, "name\ttype\trow_width\trows\tfinite")?;
    for tensor in &set.tensor_index().entries {
        let type_name = tensor.storage.type_name();
        if !sampled.insert(type_name) {
            continue;
        }
        let row_width = usize::try_from(tensor.extents[0])?;
        let rows = set.open_rows(&tensor.name)?;
        let values = rows.read_row(0)?;
        let finite = values.iter().all(|value| value.is_finite());
        if !finite {
            return Err(format!("sample row contains non-finite values: {}", tensor.name).into());
        }
        writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}",
            tensor.name,
            type_name,
            row_width,
            rows.row_count(),
            finite
        )?;
    }
    let mut projection_rows = set.open_matrix_rows("output_hc_down.weight")?;
    let input = vec![0.001_f32; projection_rows.input_features()];
    let projected = linear_from_rows(&input, &mut projection_rows, None)
        .map_err(|_| "actual GGUF row-backed projection failed")?;
    if projected.is_empty() || projected.iter().any(|value| !value.is_finite()) {
        return Err("actual GGUF row-backed projection was empty or non-finite".into());
    }
    writeln!(
        output,
        "# projection=tensor:output_hc_down.weight input={} output={} finite=true",
        input.len(),
        projected.len()
    )?;
    let gdn_input = vec![0.001_f32; config.hidden_size];
    let mut gdn_layer = GatedDeltaNetGgufLayer::open(&set, 0, &config)?;
    let gdn_output_values = gdn_layer
        .step(&gdn_input)
        .map_err(|_| "actual streamed GatedDeltaNet block failed")?;
    if gdn_output_values.len() != config.hidden_size
        || gdn_output_values.iter().any(|value| !value.is_finite())
    {
        return Err("actual GatedDeltaNet output was malformed or non-finite".into());
    }
    writeln!(
        output,
        "# decode=gated_delta_net block=0 input={} output={} conv_history_values={} recurrent_heads={} finite=true synthetic_input=true",
        gdn_input.len(),
        gdn_output_values.len(),
        gdn_layer.state().convolution_history().len(),
        gdn_layer.state().recurrent_states().len()
    )?;
    let gdn_prefill_input = vec![0.001_f32; config.hidden_size * 2];
    let mut gdn_prefill_layer = GatedDeltaNetGgufLayer::open(&set, 0, &config)?;
    let gdn_prefill_output = gdn_prefill_layer
        .process(&gdn_prefill_input)
        .map_err(|_| "actual streamed GatedDeltaNet prefill failed")?;
    if gdn_prefill_output.len() != config.hidden_size * 2
        || gdn_prefill_output.iter().any(|value| !value.is_finite())
    {
        return Err("actual GatedDeltaNet prefill output was malformed or non-finite".into());
    }
    writeln!(
        output,
        "# prefill=gated_delta_net block=0 tokens=2 input={} output={} conv_history_values={} recurrent_heads={} finite=true synthetic_input=true",
        gdn_prefill_input.len(),
        gdn_prefill_output.len(),
        gdn_prefill_layer.state().convolution_history().len(),
        gdn_prefill_layer.state().recurrent_states().len()
    )?;
    let expert_input = vec![0.001_f32; config.hidden_size];
    let mut moe_layer = MoeGgufLayer::open(&set, 17, &config)?;
    let expert_projected = moe_layer
        .forward(&expert_input)
        .map_err(|_| "actual streamed GGUF MoE block failed")?;
    if expert_projected.len() != expert_input.len()
        || expert_projected.iter().any(|value| !value.is_finite())
    {
        return Err("actual expert SwiGLU output was malformed or non-finite".into());
    }
    if set
        .open_group_matrix_rows("blk.17.ffn_gate_exps.weight", 512)
        .is_ok()
        || set
            .open_group_matrix_rows("output_hc_down.weight", 0)
            .is_ok()
    {
        return Err("invalid expert group geometry was accepted".into());
    }
    writeln!(
        output,
        "# projection=moe_block block=17 top_k={} input={} output={} finite=true",
        config.experts_per_token,
        expert_input.len(),
        expert_projected.len()
    )?;
    let hyper_width = config
        .hidden_size
        .checked_mul(config.residual_streams)
        .ok_or("residual stream width overflow")?;
    let source_token_id = 1_u64;
    let source_embedding = set
        .open_rows("token_embd.weight")?
        .read_row(source_token_id)?;
    if source_embedding.len() != config.hidden_size
        || source_embedding.iter().any(|value| !value.is_finite())
    {
        return Err("actual token embedding row was malformed or non-finite".into());
    }
    let mut hyper_input = Vec::with_capacity(hyper_width);
    for _ in 0..config.residual_streams {
        hyper_input.extend_from_slice(&source_embedding);
    }
    let mut attention_mixer =
        ResidualMixerGguf::open(&set, 0, ResidualMixerKind::Attention, &config)?;
    let mixed_attention = attention_mixer
        .mix(&hyper_input)
        .map_err(|_| "actual block-0 attention residual mixer failed")?;
    let mut block0_gdn = GatedDeltaNetGgufLayer::open(&set, 0, &config)?;
    let attention_output = block0_gdn
        .step(&mixed_attention.mixed_input)
        .map_err(|_| "actual block-0 GatedDeltaNet failed")?;
    let after_attention = mixed_attention.inject(
        &attention_output,
        config.residual_streams,
        config.hidden_size,
    )?;
    let mut feed_forward_mixer =
        ResidualMixerGguf::open(&set, 0, ResidualMixerKind::FeedForward, &config)?;
    let mixed_feed_forward = feed_forward_mixer
        .mix(&after_attention)
        .map_err(|_| "actual block-0 feed-forward residual mixer failed")?;
    let mut block0_moe = MoeGgufLayer::open(&set, 0, &config)?;
    let feed_forward_output = block0_moe
        .forward(&mixed_feed_forward.mixed_input)
        .map_err(|_| "actual block-0 routed/shared MoE failed")?;
    let block0_output = mixed_feed_forward.inject(
        &feed_forward_output,
        config.residual_streams,
        config.hidden_size,
    )?;
    if block0_output.len() != hyper_width || block0_output.iter().any(|value| !value.is_finite()) {
        return Err("actual block-0 decoder output was malformed or non-finite".into());
    }
    writeln!(
        output,
        "# decoder_block=0 type=gated_delta_net+moe token_id={} input={} output={} streams={} finite=true synthetic_input=false",
        source_token_id,
        hyper_input.len(),
        block0_output.len(),
        config.residual_streams
    )?;
    let mut block1_ple = PleGgufLayer::open(&set, 1, &config)?;
    let ple_output = block1_ple
        .step(source_token_id as u32, &block0_output, false)
        .map_err(|_| "actual block-1 PLE step failed")?;
    let after_ple = block0_output
        .iter()
        .zip(&ple_output)
        .map(|(residual, update)| residual + update)
        .collect::<Vec<_>>();
    if after_ple.len() != hyper_width || after_ple.iter().any(|value| !value.is_finite()) {
        return Err("actual PLE residual was malformed or non-finite".into());
    }
    let mut block1_attention_mixer =
        ResidualMixerGguf::open(&set, 1, ResidualMixerKind::Attention, &config)?;
    let mixed_block1_attention = block1_attention_mixer
        .mix(&after_ple)
        .map_err(|_| "actual block-1 attention residual mixer failed")?;
    let mut block1_gdn = GatedDeltaNetGgufLayer::open(&set, 1, &config)?;
    let block1_attention_output = block1_gdn
        .step(&mixed_block1_attention.mixed_input)
        .map_err(|_| "actual block-1 GatedDeltaNet failed")?;
    let after_block1_attention = mixed_block1_attention.inject(
        &block1_attention_output,
        config.residual_streams,
        config.hidden_size,
    )?;
    let mut block1_feed_forward_mixer =
        ResidualMixerGguf::open(&set, 1, ResidualMixerKind::FeedForward, &config)?;
    let mixed_block1_feed_forward = block1_feed_forward_mixer
        .mix(&after_block1_attention)
        .map_err(|_| "actual block-1 feed-forward residual mixer failed")?;
    let mut block1_moe = MoeGgufLayer::open(&set, 1, &config)?;
    let block1_feed_forward_output = block1_moe
        .forward(&mixed_block1_feed_forward.mixed_input)
        .map_err(|_| "actual block-1 routed/shared MoE failed")?;
    let block1_output = mixed_block1_feed_forward.inject(
        &block1_feed_forward_output,
        config.residual_streams,
        config.hidden_size,
    )?;
    if block1_output.len() != hyper_width || block1_output.iter().any(|value| !value.is_finite()) {
        return Err("actual block-1 decoder output was malformed or non-finite".into());
    }
    writeln!(
        output,
        "# decoder_blocks=0-1 type=gated_delta_net+ple+gated_delta_net+moe token_id={} ple_rows={} output={} finite=true synthetic_input=false",
        source_token_id,
        config.ngram_size.saturating_sub(1) * config.heads_per_ngram,
        block1_output.len()
    )?;
    let mut block2_attention_mixer =
        ResidualMixerGguf::open(&set, 2, ResidualMixerKind::Attention, &config)?;
    let mixed_block2_attention = block2_attention_mixer
        .mix(&block1_output)
        .map_err(|_| "actual block-2 attention residual mixer failed")?;
    let mut block2_gdn = GatedDeltaNetGgufLayer::open(&set, 2, &config)?;
    let block2_attention_output = block2_gdn
        .step(&mixed_block2_attention.mixed_input)
        .map_err(|_| "actual block-2 GatedDeltaNet failed")?;
    let after_block2_attention = mixed_block2_attention.inject(
        &block2_attention_output,
        config.residual_streams,
        config.hidden_size,
    )?;
    let mut block2_feed_forward_mixer =
        ResidualMixerGguf::open(&set, 2, ResidualMixerKind::FeedForward, &config)?;
    let mixed_block2_feed_forward = block2_feed_forward_mixer
        .mix(&after_block2_attention)
        .map_err(|_| "actual block-2 feed-forward residual mixer failed")?;
    let mut block2_moe = MoeGgufLayer::open(&set, 2, &config)?;
    let block2_feed_forward_output = block2_moe
        .forward(&mixed_block2_feed_forward.mixed_input)
        .map_err(|_| "actual block-2 routed/shared MoE failed")?;
    let block2_output = mixed_block2_feed_forward.inject(
        &block2_feed_forward_output,
        config.residual_streams,
        config.hidden_size,
    )?;
    if block2_output.len() != hyper_width || block2_output.iter().any(|value| !value.is_finite()) {
        return Err("actual block-2 decoder output was malformed or non-finite".into());
    }
    let qsa_block_hyper_input = block2_output;
    let mut qsa_attention_mixer =
        ResidualMixerGguf::open(&set, 3, ResidualMixerKind::Attention, &config)?;
    let mixed_qsa_attention = qsa_attention_mixer
        .mix(&qsa_block_hyper_input)
        .map_err(|_| "actual block-3 QSA attention residual mixer failed")?;
    let mut block3_qsa = QsaGgufLayer::open(&set, 3, &config)?;
    let qsa_attention_output = block3_qsa
        .step(&mixed_qsa_attention.mixed_input, 0)
        .map_err(|_| "actual block-3 QSA attention failed")?;
    let after_qsa_attention = mixed_qsa_attention.inject(
        &qsa_attention_output.output,
        config.residual_streams,
        config.hidden_size,
    )?;
    let mut qsa_feed_forward_mixer =
        ResidualMixerGguf::open(&set, 3, ResidualMixerKind::FeedForward, &config)?;
    let mixed_qsa_feed_forward = qsa_feed_forward_mixer
        .mix(&after_qsa_attention)
        .map_err(|_| "actual block-3 feed-forward residual mixer failed")?;
    let mut block3_moe = MoeGgufLayer::open(&set, 3, &config)?;
    let qsa_feed_forward_output = block3_moe
        .forward(&mixed_qsa_feed_forward.mixed_input)
        .map_err(|_| "actual block-3 routed/shared MoE failed")?;
    let block3_output = mixed_qsa_feed_forward.inject(
        &qsa_feed_forward_output,
        config.residual_streams,
        config.hidden_size,
    )?;
    if block3_output.len() != hyper_width || block3_output.iter().any(|value| !value.is_finite()) {
        return Err("actual block-3 decoder output was malformed or non-finite".into());
    }
    let mut full_model = QwenGgufDecoder::open(&set, &config)?;
    let prepared_prompt = if env::var_os("AGENTIX_QWEN_CHAT_SMOKE").is_some() {
        let tokenizer_path = PathBuf::from(
            env::var_os("AGENTIX_QWEN_TOKENIZER_PATH")
                .ok_or("AGENTIX_QWEN_TOKENIZER_PATH is required for chat smoke")?,
        );
        let template_path = PathBuf::from(
            env::var_os("AGENTIX_QWEN_TEMPLATE_PATH")
                .ok_or("AGENTIX_QWEN_TEMPLATE_PATH is required for chat smoke")?,
        );
        let tokenizer = QwenTokenizer::from_files(&tokenizer_path, &template_path)?;
        let messages = [serde_json::json!({
            "role": "user",
            "content": "Reply with OK."
        })];
        tokenizer.prepare_prompt(PromptRequest {
            messages: &messages,
            tools: &[],
            reasoning_effort: "low",
            enable_thinking: false,
            preserve_thinking: false,
            add_generation_prompt: true,
            max_new_tokens: 2,
            context_limit: config.max_position_embeddings,
        })?
    } else {
        let total_tokens = 3;
        PreparedPrompt {
            rendered: String::new(),
            token_ids: vec![u32::try_from(source_token_id)?],
            admission: ContextAdmission {
                prompt_tokens: 1,
                requested_output_tokens: 2,
                total_tokens,
                remaining_tokens: config
                    .max_position_embeddings
                    .checked_sub(total_tokens)
                    .ok_or("Qwen test prompt exceeds model context")?,
            },
        }
    };
    let generated = full_model
        .generate_prepared_greedy(&prepared_prompt, &[])
        .map_err(|_| "actual full-model prepared-prompt generation failed")?;
    let predicted_token = *generated
        .generation
        .generated_token_ids
        .first()
        .ok_or("full-model greedy generation returned no token")?;
    let second_predicted_token = *generated
        .generation
        .generated_token_ids
        .get(1)
        .ok_or("full-model greedy generation returned fewer than two tokens")?;
    if generated.prompt_logits.len() != config.vocab_size
        || generated
            .prompt_logits
            .iter()
            .any(|value| !value.is_finite())
        || generated.generation.generated_token_ids.len() != 2
        || full_model.position() != 2
        || full_model.is_poisoned()
    {
        return Err("stateful full-model decode output was malformed".into());
    }
    writeln!(
        output,
        "# decoder_blocks=0-3 types=gdn_moe,gdn_ple_moe,gdn_moe,qsa_moe token_id={} input={} output={} streams={} cached_tokens={} finite=true synthetic_input=false",
        source_token_id,
        qsa_block_hyper_input.len(),
        block3_output.len(),
        config.residual_streams,
        block3_qsa.cached_tokens()
    )?;
    writeln!(
        output,
        "# decoder_model=full blocks={} prompt_source={} prompt_tokens={} output_logits={} greedy_token_id={} greedy_logit={:.8} second_input_token_id={} second_greedy_token_id={} cached_position={} finite=true synthetic_input=false",
        config.layer_types.len(),
        if prepared_prompt.rendered.is_empty() { "synthetic_token" } else { "pinned_chat_template" },
        prepared_prompt.admission.prompt_tokens,
        generated.prompt_logits.len(),
        predicted_token,
        generated.prompt_logits[predicted_token as usize],
        predicted_token,
        second_predicted_token,
        full_model.position()
    )?;
    let mut qsa_layer = QsaGgufLayer::open(&set, 3, &config)?;
    let qsa_first = qsa_layer
        .step(&expert_input, 0)
        .map_err(|_| "actual row-backed QSA layer first token failed")?;
    let qsa_second = qsa_layer
        .step(&expert_input, 1)
        .map_err(|_| "actual row-backed QSA layer cached token failed")?;
    if qsa_layer.step(&expert_input, 3).is_ok() || qsa_layer.cached_tokens() != 2 {
        return Err("QSA layer accepted a skipped position or changed its cache".into());
    }
    if qsa_first.output.len() != config.hidden_size
        || qsa_second.output.len() != config.hidden_size
        || qsa_layer.cached_tokens() != 2
        || qsa_first.output.iter().any(|value| !value.is_finite())
        || qsa_second.output.iter().any(|value| !value.is_finite())
    {
        return Err("actual row-backed QSA layer output/cache was malformed".into());
    }
    writeln!(
        output,
        "# decode=qsa_layer block=3 tokens=2 input={} output={} selected_last={} cached_tokens={} finite=true synthetic_input=true",
        expert_input.len(),
        qsa_second.output.len(),
        qsa_second.selected_token_indices.len(),
        qsa_layer.cached_tokens()
    )?;
    let indexer_head_dim = config.indexer_head_dim;
    let indexer_rotary_dim =
        usize::try_from((indexer_head_dim as f32 * config.partial_rotary_factor).round() as u64)?;
    let indexer_rope =
        TextRotaryEmbedding::new(indexer_head_dim, indexer_rotary_dim, config.rope_theta)?;
    let mut indexer_query = set.open_matrix_rows("blk.3.indexer.q_proj.weight")?;
    let mut indexer_key = set.open_matrix_rows("blk.3.indexer.k_proj.weight")?;
    let indexer_query_norm = set.open_rows("blk.3.indexer.q_norm.weight")?.read_row(0)?;
    let indexer_key_norm = set.open_rows("blk.3.indexer.k_norm.weight")?.read_row(0)?;
    let synthetic_tokens = config
        .indexer_token_budget
        .checked_mul(2)
        .ok_or("synthetic QSA context length overflow")?;
    let past_index_keys = vec![0.0_f32; synthetic_tokens * indexer_head_dim];
    let past_positions = (0..u32::try_from(synthetic_tokens)?).collect::<Vec<_>>();
    let base_visible = vec![true; synthetic_tokens + 1];
    let head_dim = config.attention_head_dim;
    let rotary_dim =
        usize::try_from((head_dim as f32 * config.partial_rotary_factor).round() as u64)?;
    let attention_rope = TextRotaryEmbedding::new(head_dim, rotary_dim, config.rope_theta)?;
    let mut attention_query = set.open_matrix_rows("blk.3.attn_q.weight")?;
    let mut attention_key = set.open_matrix_rows("blk.3.attn_k.weight")?;
    let mut attention_value = set.open_matrix_rows("blk.3.attn_v.weight")?;
    let mut attention_output = set.open_matrix_rows("blk.3.attn_output.weight")?;
    let query_norm = set.open_rows("blk.3.attn_q_norm.weight")?.read_row(0)?;
    let key_norm = set.open_rows("blk.3.attn_k_norm.weight")?.read_row(0)?;
    let past_attention_tokens = synthetic_tokens;
    let past_attention_keys =
        vec![0.0_f32; past_attention_tokens * config.attention_kv_heads * head_dim];
    let past_attention_values = vec![0.0_f32; past_attention_keys.len()];
    let decode = qsa_decode_step_from_rows(
        &expert_input,
        &past_index_keys,
        &past_positions,
        &base_visible,
        &past_attention_keys,
        &past_attention_values,
        QsaDecodeGeometry {
            indexer_query_heads: config.indexer_query_heads,
            indexer_head_dim,
            query_heads: config.attention_heads,
            kv_heads: config.attention_kv_heads,
            head_dim,
            compress_ratio: config.indexer_compress_ratio,
            token_budget: config.indexer_token_budget,
            epsilon: config.rms_norm_epsilon,
        },
        &indexer_rope,
        &attention_rope,
        u32::try_from(synthetic_tokens)?,
        &mut indexer_query,
        &mut indexer_key,
        &indexer_query_norm,
        &indexer_key_norm,
        &mut attention_query,
        &mut attention_key,
        &mut attention_value,
        &query_norm,
        &key_norm,
        &mut attention_output,
    )
    .map_err(|_| "actual streamed QSA decode failed")?;
    if decode.selected_token_indices.len() >= base_visible.len()
        || !decode.selected_token_indices.contains(&synthetic_tokens)
        || decode.output.len() != config.hidden_size
        || decode.attention_keys.len()
            != (past_attention_tokens + 1) * config.attention_kv_heads * head_dim
        || decode.attention_values.len() != decode.attention_keys.len()
        || decode.output.iter().any(|value| !value.is_finite())
    {
        return Err("actual QSA decode output, sparsity or cache was malformed".into());
    }
    writeln!(
        output,
        "# indexer=qsa_sparse block=3 visible={} selected={} raw_cache_values={} synthetic_past=true",
        base_visible.len(),
        decode.selected_token_indices.len(),
        decode.raw_index_key_cache.len()
    )?;
    writeln!(
        output,
        "# decode=qsa block=3 input={} output={} selected={} kv_cache_values={} finite=true synthetic_past=true",
        expert_input.len(),
        decode.output.len(),
        decode.selected_token_indices.len(),
        decode.attention_keys.len()
    )?;
    writeln!(
        output,
        "# shards={} tensors={} sampled_types={}",
        paths.len(),
        set.tensor_index().entries.len(),
        sampled.len()
    )?;
    Ok(())
}
