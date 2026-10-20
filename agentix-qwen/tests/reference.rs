use agentix_qwen::{
    reference::{
        add_one_to_norm, fused_swiglu_expert_from_rows, gated_delta_parameters, gated_residual,
        gated_residual_from_rows, gated_rms_norm, grouped_query_attention, linear,
        linear_from_rows, moe_block, moe_block_from_rows, ple_gate, ple_step, ple_step_from_rows,
        qsa_attention_step, qsa_attention_step_from_rows, qsa_decode_step,
        qsa_decode_step_from_rows, qsa_indexer_project, qsa_indexer_step,
        qsa_indexer_step_from_rows, qsa_prefill, qsa_prefill_with_cache, qsa_select_tokens,
        reorder_gdn_qkv_value_rows, reorder_gdn_value_head_columns, reorder_gdn_value_heads_axis0,
        rms_norm, route, split_expert_gate_up, split_indexer_qk_rows, squeeze_ple_convolution,
        DeltaState, DepthwiseConvState, ExpertWeights, GatedDeltaNetState, GatedDeltaNetWeights,
        GatedDeltaState, PleLayerWeights, QsaAttentionWeights, QsaCachedPrefillInput,
        QsaDecodeGeometry, QsaDecodeWeights, QsaIndexerWeights, QsaPrefillInput,
        TextRotaryEmbedding,
    },
    Error, Result,
};

fn close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() <= 1e-6, "{a} != {e}");
    }
}

struct TestLinearRows {
    values: Vec<f32>,
    output_features: u64,
    input_features: usize,
}

impl agentix_qwen::reference::LinearWeightRows for TestLinearRows {
    fn output_features(&self) -> u64 {
        self.output_features
    }

    fn input_features(&self) -> usize {
        self.input_features
    }

    fn read_weight_row(&mut self, row: u64, output: &mut [f32]) -> Result<()> {
        if output.len() != self.input_features {
            return Err(Error::Invalid("test linear row buffer shape mismatch"));
        }
        if row >= self.output_features {
            return Err(Error::Invalid("test linear row is out of range"));
        }
        let row = usize::try_from(row).map_err(|_| Error::Overflow)?;
        let start = row
            .checked_mul(self.input_features)
            .ok_or(Error::Overflow)?;
        let end = start
            .checked_add(self.input_features)
            .ok_or(Error::Overflow)?;
        output.copy_from_slice(
            self.values
                .get(start..end)
                .ok_or(Error::Invalid("test linear row data is truncated"))?,
        );
        Ok(())
    }
}

struct FailAfterLinearRows {
    inner: TestLinearRows,
    reads: usize,
    fail_at: usize,
}

impl agentix_qwen::reference::LinearWeightRows for FailAfterLinearRows {
    fn output_features(&self) -> u64 {
        self.inner.output_features
    }

    fn input_features(&self) -> usize {
        self.inner.input_features
    }

    fn read_weight_row(&mut self, row: u64, output: &mut [f32]) -> Result<()> {
        if self.reads == self.fail_at {
            return Err(Error::Invalid("injected later-token row failure"));
        }
        self.reads += 1;
        self.inner.read_weight_row(row, output)
    }
}

#[test]
fn row_backed_linear_matches_materialized_projection() -> Result<()> {
    let input = [0.5, -1.0];
    let weight = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let bias = [0.25, -0.5, 1.0];
    let expected = linear(&input, &weight, Some(&bias))?;
    let mut rows = TestLinearRows {
        values: weight.to_vec(),
        output_features: 3,
        input_features: 2,
    };
    let actual = linear_from_rows(&input, &mut rows, Some(&bias))?;
    close(&actual, &expected);
    Ok(())
}

#[test]
fn native_projection_dispatch_validates_outputs_bias_and_errors() -> Result<()> {
    struct NativeRows(Result<Option<Vec<f32>>>);
    impl agentix_qwen::reference::LinearWeightRows for NativeRows {
        fn output_features(&self) -> u64 {
            2
        }
        fn input_features(&self) -> usize {
            2
        }
        fn read_weight_row(&mut self, _: u64, _: &mut [f32]) -> Result<()> {
            panic!("native dispatch must not fall back to row reads");
        }
        fn project(&mut self, _: &[f32]) -> Result<Option<Vec<f32>>> {
            std::mem::replace(&mut self.0, Err(Error::Invalid("called twice")))
        }
    }
    let mut rows = NativeRows(Ok(Some(vec![2.0, 3.0])));
    assert_eq!(
        linear_from_rows(&[1.0, 2.0], &mut rows, Some(&[0.5, -0.5]))?,
        [2.5, 2.5]
    );
    for values in [vec![1.0], vec![1.0, f32::NAN], vec![f32::INFINITY, 0.0]] {
        assert!(linear_from_rows(&[1.0, 2.0], &mut NativeRows(Ok(Some(values))), None).is_err());
    }
    assert!(linear_from_rows(
        &[1.0, 2.0],
        &mut NativeRows(Err(Error::Capacity("device"))),
        None
    )
    .is_err());
    assert!(linear_from_rows(
        &[1.0, f32::NAN],
        &mut NativeRows(Ok(Some(vec![0.0; 2]))),
        None
    )
    .is_err());
    Ok(())
}

#[test]
fn row_backed_swiglu_expert_matches_materialized_equations() -> Result<()> {
    let input = [0.5, -1.0];
    let gate_weight = [0.5, -1.0, 1.5, 0.25];
    let up_weight = [1.0, 2.0, -0.5, 1.0];
    let down_weight = [1.0, 0.5, -1.0, 2.0];
    let gate = linear(&input, &gate_weight, None)?;
    let up = linear(&input, &up_weight, None)?;
    let activated = gate
        .iter()
        .zip(up)
        .map(|(&gate, up)| {
            let sigmoid = 1.0 / (1.0 + (-gate).exp());
            gate * sigmoid * up
        })
        .collect::<Vec<_>>();
    let expected = linear(&activated, &down_weight, None)?;
    let mut gate_rows = TestLinearRows {
        values: gate_weight.to_vec(),
        output_features: 2,
        input_features: 2,
    };
    let mut up_rows = TestLinearRows {
        values: up_weight.to_vec(),
        output_features: 2,
        input_features: 2,
    };
    let mut down_rows = TestLinearRows {
        values: down_weight.to_vec(),
        output_features: 2,
        input_features: 2,
    };
    let actual =
        fused_swiglu_expert_from_rows(&input, &mut gate_rows, &mut up_rows, &mut down_rows)?;
    close(&actual, &expected);
    Ok(())
}

#[test]
fn routing_softmax_selection_and_ties() -> Result<()> {
    let logits = [0.0, 2.0_f32.ln(), 3.0_f32.ln()];
    let selected = route(&logits, 2, true)?;
    assert_eq!(
        selected.iter().map(|r| r.expert).collect::<Vec<_>>(),
        [2, 1]
    );
    close(
        &selected.iter().map(|r| r.weight).collect::<Vec<_>>(),
        &[0.6, 0.4],
    );
    let unnormalized = route(&logits, 2, false)?;
    close(
        &unnormalized.iter().map(|r| r.weight).collect::<Vec<_>>(),
        &[0.5, 1.0 / 3.0],
    );
    assert_eq!(
        route(&[1000.0, 1000.0, -1000.0], 2, true)?
            .iter()
            .map(|r| r.expert)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    for (values, k) in [
        (&[][..], 1),
        (&[0.0][..], 0),
        (&[0.0][..], 2),
        (&[f32::NAN][..], 1),
    ] {
        assert!(route(values, k, true).is_err());
    }
    Ok(())
}

#[test]
fn grouped_offset_norm_and_direct_gated_norm() -> Result<()> {
    // With epsilon=1, each pair has RMS denominator sqrt(2); the zero stays zero.
    let s = 2.0_f32.sqrt();
    close(
        &rms_norm(&[1.0, -1.0, 0.0, s], &[0.0, 1.0, 9.0, 2.0], 2, 1.0)?,
        &[1.0 / s, -2.0 / s, 0.0, 3.0],
    );
    close(
        &gated_rms_norm(&[1.0, -1.0], &[2.0, 0.0], &[0.0, 1000.0], 1.0)?,
        &[1.0 / s, 0.0],
    );
    assert!(rms_norm(&[1.0], &[0.0], 2, 1e-6).is_err());
    assert!(rms_norm(&[1.0], &[], 1, 1e-6).is_err());
    assert!(rms_norm(&[1.0], &[0.0], 1, 0.0).is_err());
    assert!(rms_norm(&[f32::MAX], &[0.0], 1, 1e-6).is_err());
    assert!(gated_rms_norm(&[1.0], &[1.0], &[f32::NAN], 1e-6).is_err());
    Ok(())
}

#[test]
fn linear_and_gated_residual_match_pinned_mixer_equations() -> Result<()> {
    close(
        &linear(&[1.0, 2.0], &[1.0, 0.0, 0.0, 1.0], Some(&[3.0, -1.0]))?,
        &[4.0, 1.0],
    );
    let input = [1.0, 0.0, 0.0, 1.0];
    let down = [1.0, 1.0, 1.0, 1.0];
    let up = [1.0, -1.0, 1.0, -1.0];
    let injection = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let mixed = gated_residual(
        &input,
        &[0.0; 4],
        &down,
        &up,
        Some(&injection),
        2,
        2,
        1,
        0.5,
    )?;
    let lowrank = 1.0 / (1.0 + (-1.0_f32).exp());
    let gate_positive = 1.0 / (1.0 + (-lowrank).exp());
    let gate_negative = 1.0 / (1.0 + lowrank.exp());
    close(
        &mixed.mixed_input,
        &[gate_positive / 2.0, gate_negative / 2.0],
    );
    let injection_scale = 2.0 / (1.0 + (-0.5_f32).exp());
    close(
        mixed.injection_weights.as_deref().unwrap_or(&[]),
        &[injection_scale; 2],
    );
    close(
        &mixed.inject(&[2.0, 4.0], 2, 2)?,
        &[
            1.0 + injection_scale * 2.0,
            injection_scale * 4.0,
            injection_scale * 2.0,
            1.0 + injection_scale * 4.0,
        ],
    );

    let rows = |values: &[f32], outputs, inputs| TestLinearRows {
        values: values.to_vec(),
        output_features: outputs,
        input_features: inputs,
    };
    let mut down_rows = rows(&down, 1, 4);
    let mut up_rows = rows(&up, 4, 1);
    let mut inject_rows = rows(&injection, 2, 4);
    let streamed = gated_residual_from_rows(
        &input,
        &[0.0; 4],
        &mut down_rows,
        &mut up_rows,
        Some(&mut inject_rows),
        2,
        2,
        0.5,
    )?;
    close(&streamed.mixed_input, &mixed.mixed_input);
    close(
        streamed.injection_weights.as_deref().unwrap_or(&[]),
        mixed.injection_weights.as_deref().unwrap_or(&[]),
    );
    close(
        &streamed.inject(&[2.0, 4.0], 2, 2)?,
        &mixed.inject(&[2.0, 4.0], 2, 2)?,
    );

    let final_mixer = gated_residual(&input, &[0.0; 4], &down, &up, None, 2, 2, 1, 0.5)?;
    assert!(final_mixer.injection_weights.is_none());
    assert!(final_mixer.inject(&[1.0, 1.0], 2, 2).is_err());
    assert!(linear(&[1.0], &[f32::MAX], Some(&[f32::MAX])).is_err());
    assert!(gated_residual(&input, &[0.0; 4], &down, &up, Some(&[0.0]), 2, 2, 1, 0.5).is_err());
    Ok(())
}

#[test]
fn routed_and_shared_swiglu_experts_match_pinned_equations() -> Result<()> {
    let experts = [
        ExpertWeights {
            gate_up: &[1.0, 1.0],
            down: &[2.0],
        },
        ExpertWeights {
            gate_up: &[2.0, 1.0],
            down: &[3.0],
        },
    ];
    let output = moe_block(
        &[1.0],
        &[3.0_f32.ln(), 0.0],
        &experts,
        1,
        2,
        false,
        &[1.0],
        &[2.0],
        &[4.0],
        &[3.0_f32.ln()],
        1,
    )?;
    let silu_one = 1.0 / (1.0 + (-1.0_f32).exp());
    let silu_two = 2.0 / (1.0 + (-2.0_f32).exp());
    close(
        &output,
        &[0.75 * 2.0 * silu_one + 0.25 * 3.0 * silu_two + 0.75 * 8.0 * silu_one],
    );
    assert!(moe_block(
        &[1.0],
        &[1.0],
        &experts,
        1,
        1,
        true,
        &[1.0],
        &[1.0],
        &[1.0],
        &[1.0],
        1,
    )
    .is_err());
    Ok(())
}

#[test]
fn row_backed_moe_matches_materialized_routing_and_shared_expert() -> Result<()> {
    let mut shared_gate = TestLinearRows {
        values: vec![1.0],
        output_features: 1,
        input_features: 1,
    };
    let mut shared_up = TestLinearRows {
        values: vec![2.0],
        output_features: 1,
        input_features: 1,
    };
    let mut shared_down = TestLinearRows {
        values: vec![4.0],
        output_features: 1,
        input_features: 1,
    };
    let mut shared_expert_gate = TestLinearRows {
        values: vec![3.0_f32.ln()],
        output_features: 1,
        input_features: 1,
    };
    let mut router = TestLinearRows {
        values: vec![3.0_f32.ln(), 0.0],
        output_features: 2,
        input_features: 1,
    };
    let actual = moe_block_from_rows(
        &[1.0],
        &mut router,
        2,
        false,
        |expert| {
            let (gate, down) = match expert {
                0 => (1.0, 2.0),
                1 => (2.0, 3.0),
                _ => return Err(Error::Invalid("unexpected test expert")),
            };
            Ok((
                TestLinearRows {
                    values: vec![gate],
                    output_features: 1,
                    input_features: 1,
                },
                TestLinearRows {
                    values: vec![1.0],
                    output_features: 1,
                    input_features: 1,
                },
                TestLinearRows {
                    values: vec![down],
                    output_features: 1,
                    input_features: 1,
                },
            ))
        },
        &mut shared_gate,
        &mut shared_up,
        &mut shared_down,
        &mut shared_expert_gate,
    )?;
    let expected = moe_block(
        &[1.0],
        &[3.0_f32.ln(), 0.0],
        &[
            ExpertWeights {
                gate_up: &[1.0, 1.0],
                down: &[2.0],
            },
            ExpertWeights {
                gate_up: &[2.0, 1.0],
                down: &[3.0],
            },
        ],
        1,
        2,
        false,
        &[1.0],
        &[2.0],
        &[4.0],
        &[3.0_f32.ln()],
        1,
    )?;
    close(&actual, &expected);
    Ok(())
}

#[test]
fn qsa_selects_scored_visible_blocks_and_appends_tail() -> Result<()> {
    let identity_cos = [1.0; 12];
    let identity_sin = [0.0; 12];
    let selected = qsa_select_tokens(
        &[1.0, 0.0],
        &[
            1.0, 0.0, // visible token 0
            99.0, 99.0, // masked token 1 must not affect pooling
            1.0, 0.0, // visible token 2, completes block [0, 2]
            0.0, 1.0, // block [3, 4] scores lower
            0.0, 1.0, 1.0, 0.0, // incomplete visible tail
        ],
        &[0.0; 2],
        &[true, false, true, true, true, true],
        &identity_cos,
        &identity_sin,
        2,
        2,
        2,
        1,
        1e-6,
    )?;
    assert_eq!(selected, [0, 2, 5]);

    let tied = qsa_select_tokens(
        &[1.0, 0.0],
        &[0.0; 8],
        &[0.0; 2],
        &[true; 4],
        &[1.0; 8],
        &[0.0; 8],
        2,
        2,
        2,
        1,
        1e-6,
    )?;
    assert_eq!(tied, [0, 1]);
    assert!(qsa_select_tokens(
        &[1.0],
        &[1.0],
        &[0.0],
        &[true],
        &[1.0],
        &[0.0],
        1,
        1,
        1,
        1,
        1e-6,
    )
    .is_err());
    Ok(())
}

#[test]
fn grouped_query_attention_applies_visibility_and_output_gate() -> Result<()> {
    let output = grouped_query_attention(
        &[1.0, 0.0, 0.0, 1.0],
        &[0.0; 4],
        &[1.0, 0.0, 0.0, 1.0],
        &[2.0, 0.0, 0.0, 4.0],
        &[true, true],
        2,
        1,
        2,
    )?;
    let positive = 1.0 / (1.0 + (-1.0 / 2.0_f32.sqrt()).exp());
    close(
        &output,
        &[
            positive,
            2.0 * (1.0 - positive),
            1.0 - positive,
            2.0 * positive,
        ],
    );

    let masked = grouped_query_attention(
        &[1.0, 0.0, 0.0, 1.0],
        &[0.0; 4],
        &[1.0, 0.0, 0.0, 1.0],
        &[2.0, 0.0, 0.0, 4.0],
        &[true, false],
        2,
        1,
        2,
    )?;
    close(&masked, &[1.0, 0.0, 1.0, 0.0]);
    assert!(grouped_query_attention(
        &[1.0, 0.0],
        &[0.0; 2],
        &[1.0, 0.0],
        &[1.0, 0.0],
        &[false],
        1,
        1,
        2,
    )
    .is_err());
    Ok(())
}

#[test]
fn text_rope_matches_partial_rotate_half_layout() -> Result<()> {
    let rope = TextRotaryEmbedding::new(6, 4, 10_000.0)?;
    assert_eq!(rope.rotary_dim(), 4);
    let (cos, sin) = rope.cos_sin(1)?;
    close(
        &cos,
        &[1.0_f32.cos(), 0.01_f32.cos(), 1.0_f32.cos(), 0.01_f32.cos()],
    );
    close(
        &sin,
        &[1.0_f32.sin(), 0.01_f32.sin(), 1.0_f32.sin(), 0.01_f32.sin()],
    );
    close(
        &rope.apply(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 1)?,
        &[-1.9841106, 1.9599007, 2.4623778, 4.0197997, 5.0, 6.0],
    );
    assert!(TextRotaryEmbedding::new(6, 3, 10_000.0).is_err());
    assert!(rope.apply(&[1.0; 5], 0).is_err());
    Ok(())
}

#[test]
fn qsa_indexer_projects_normalizes_rotates_queries_and_preserves_raw_key() -> Result<()> {
    let rope = TextRotaryEmbedding::new(2, 2, 10_000.0)?;
    let (queries, raw_key) = qsa_indexer_project(
        &[1.0, 2.0],
        &[
            1.0, 0.0, // query dim 0
            0.0, 1.0, // query dim 1
            2.0, 0.0, // raw key dim 0
            0.0, 2.0, // raw key dim 1
        ],
        &[0.0, 0.0],
        1,
        1,
        2,
        1.0,
        &rope,
        0,
    )?;
    close(&queries, &[1.0 / 3.5_f32.sqrt(), 2.0 / 3.5_f32.sqrt()]);
    close(&raw_key, &[2.0, 4.0]);
    assert!(
        qsa_indexer_project(&[1.0, 2.0], &[0.0; 8], &[0.0, 0.0], 1, 2, 2, 1.0, &rope, 0,).is_err()
    );
    Ok(())
}

#[test]
fn qsa_indexer_step_appends_raw_key_and_selects_blocks_with_absolute_positions() -> Result<()> {
    let rope = TextRotaryEmbedding::new(2, 2, 10_000.0)?;
    let projection = [
        1.0, 0.0, // query dim 0
        0.0, 1.0, // query dim 1
        1.0, 0.0, // raw key dim 0
        0.0, 1.0, // raw key dim 1
    ];
    let output = qsa_indexer_step(
        &[1.0, 0.0],
        &[1.0, 0.0, 0.0, 1.0],
        &[0, 1],
        &[true, true, true],
        &projection,
        &[0.0, 0.0],
        &[0.0, 0.0],
        1,
        2,
        2,
        2,
        1e-6,
        &rope,
        2,
    )?;
    assert_eq!(output.selected_token_indices, [0, 1, 2]);
    close(&output.raw_key_cache, &[1.0, 0.0, 0.0, 1.0, 1.0, 0.0]);
    let mut query_rows = TestLinearRows {
        values: projection[..4].to_vec(),
        output_features: 2,
        input_features: 2,
    };
    let mut key_rows = TestLinearRows {
        values: projection[4..].to_vec(),
        output_features: 2,
        input_features: 2,
    };
    let row_output = qsa_indexer_step_from_rows(
        &[1.0, 0.0],
        &[1.0, 0.0, 0.0, 1.0],
        &[0, 1],
        &[true, true, true],
        &mut query_rows,
        &mut key_rows,
        &[0.0, 0.0],
        &[0.0, 0.0],
        1,
        2,
        2,
        2,
        1e-6,
        &rope,
        2,
    )?;
    assert_eq!(row_output, output);
    assert!(qsa_indexer_step(
        &[1.0, 0.0],
        &[],
        &[],
        &[true],
        &projection,
        &[0.0, 0.0],
        &[0.0, 0.0],
        1,
        2,
        2,
        1,
        1e-6,
        &rope,
        0,
    )
    .is_err());
    Ok(())
}

#[test]
fn qsa_attention_step_projects_appends_attends_gates_and_projects_output() -> Result<()> {
    let rope = TextRotaryEmbedding::new(2, 2, 10_000.0)?;
    let weights = QsaAttentionWeights {
        query_gate_projection: &[
            1.0, 0.0, // query dim 0
            0.0, 1.0, // query dim 1
            0.0, 0.0, // gate dim 0
            0.0, 0.0, // gate dim 1
        ],
        key_projection: &[1.0, 0.0, 0.0, 1.0],
        value_projection: &[2.0, 0.0, 0.0, 2.0],
        query_norm_offset: &[0.0, 0.0],
        key_norm_offset: &[0.0, 0.0],
        output_projection: &[1.0, 0.0, 0.0, 1.0],
    };
    let output = qsa_attention_step(
        &[1.0, 0.0],
        &[],
        &[],
        &[true],
        1,
        1,
        2,
        1.0,
        &rope,
        0,
        weights,
    )?;
    close(&output.output, &[1.0, 0.0]);
    close(&output.keys, &[1.0 / 1.5_f32.sqrt(), 0.0]);
    close(&output.values, &[2.0, 0.0]);
    let mut query_rows = TestLinearRows {
        values: weights.query_gate_projection.to_vec(),
        output_features: 4,
        input_features: 2,
    };
    let mut key_rows = TestLinearRows {
        values: weights.key_projection.to_vec(),
        output_features: 2,
        input_features: 2,
    };
    let mut value_rows = TestLinearRows {
        values: weights.value_projection.to_vec(),
        output_features: 2,
        input_features: 2,
    };
    let mut output_rows = TestLinearRows {
        values: weights.output_projection.to_vec(),
        output_features: 2,
        input_features: 2,
    };
    let row_output = qsa_attention_step_from_rows(
        &[1.0, 0.0],
        &[],
        &[],
        &[true],
        1,
        1,
        2,
        1.0,
        &rope,
        0,
        &mut query_rows,
        &mut key_rows,
        &mut value_rows,
        weights.query_norm_offset,
        weights.key_norm_offset,
        &mut output_rows,
    )?;
    assert_eq!(row_output, output);

    let hidden_with_past = qsa_attention_step(
        &[1.0, 0.0],
        &[0.0, 1.0],
        &[0.0, 4.0],
        &[true, true],
        1,
        1,
        2,
        1.0,
        &rope,
        1,
        weights,
    )?;
    assert_eq!(hidden_with_past.keys.len(), 4);
    assert_eq!(hidden_with_past.values.len(), 4);
    Ok(())
}

#[test]
fn qsa_decode_step_connects_indexer_mask_and_main_attention() -> Result<()> {
    let indexer_rope = TextRotaryEmbedding::new(2, 2, 10_000.0)?;
    let attention_rope = TextRotaryEmbedding::new(4, 2, 10_000.0)?;
    let output = qsa_decode_step(
        &[1.0, 0.0],
        &[],
        &[],
        &[true],
        &[],
        &[],
        1,
        2,
        1,
        1,
        4,
        2,
        2,
        1.0,
        &indexer_rope,
        &attention_rope,
        0,
        QsaDecodeWeights {
            indexer: QsaIndexerWeights {
                projection: &[1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
                query_norm_offset: &[0.0, 0.0],
                key_norm_offset: &[0.0, 0.0],
            },
            attention: QsaAttentionWeights {
                query_gate_projection: &[
                    1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, // query head
                    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, // sigmoid gate
                ],
                key_projection: &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                value_projection: &[2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                query_norm_offset: &[0.0; 4],
                key_norm_offset: &[0.0; 4],
                output_projection: &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            },
        },
    )?;
    assert_eq!(output.selected_token_indices, [0]);
    close(&output.output, &[1.0, 0.0]);
    assert_eq!(output.raw_index_key_cache.len(), 2);
    assert_eq!(output.attention_keys.len(), 4);
    assert_eq!(output.attention_values.len(), 4);
    let mut indexer_query_rows = TestLinearRows {
        values: vec![1.0, 0.0, 0.0, 1.0],
        output_features: 2,
        input_features: 2,
    };
    let mut indexer_key_rows = TestLinearRows {
        values: vec![1.0, 0.0, 0.0, 1.0],
        output_features: 2,
        input_features: 2,
    };
    let mut attention_query_rows = TestLinearRows {
        values: vec![
            1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ],
        output_features: 8,
        input_features: 2,
    };
    let mut attention_key_rows = TestLinearRows {
        values: vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        output_features: 4,
        input_features: 2,
    };
    let mut attention_value_rows = TestLinearRows {
        values: vec![2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        output_features: 4,
        input_features: 2,
    };
    let mut attention_output_rows = TestLinearRows {
        values: vec![1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        output_features: 2,
        input_features: 4,
    };
    let row_output = qsa_decode_step_from_rows(
        &[1.0, 0.0],
        &[],
        &[],
        &[true],
        &[],
        &[],
        QsaDecodeGeometry {
            indexer_query_heads: 1,
            indexer_head_dim: 2,
            query_heads: 1,
            kv_heads: 1,
            head_dim: 4,
            compress_ratio: 2,
            token_budget: 2,
            epsilon: 1.0,
        },
        &indexer_rope,
        &attention_rope,
        0,
        &mut indexer_query_rows,
        &mut indexer_key_rows,
        &[0.0; 2],
        &[0.0; 2],
        &mut attention_query_rows,
        &mut attention_key_rows,
        &mut attention_value_rows,
        &[0.0; 4],
        &[0.0; 4],
        &mut attention_output_rows,
    )?;
    assert_eq!(row_output, output);
    Ok(())
}

#[test]
fn qsa_prefill_matches_sequential_decode_with_triangular_padding_masks() -> Result<()> {
    let rope = TextRotaryEmbedding::new(2, 2, 10_000.0)?;
    let weights = QsaDecodeWeights {
        indexer: QsaIndexerWeights {
            projection: &[1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            query_norm_offset: &[0.0, 0.0],
            key_norm_offset: &[0.0, 0.0],
        },
        attention: QsaAttentionWeights {
            query_gate_projection: &[
                1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, // query, then gate
            ],
            key_projection: &[1.0, 0.0, 0.0, 1.0],
            value_projection: &[2.0, 0.0, 0.0, 2.0],
            query_norm_offset: &[0.0, 0.0],
            key_norm_offset: &[0.0, 0.0],
            output_projection: &[1.0, 0.0, 0.0, 1.0],
        },
    };
    let hidden = [1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let positions = [0, 1, 2];
    let mask = [true, false, true, false, true, true];
    let geometry = QsaDecodeGeometry {
        indexer_query_heads: 1,
        indexer_head_dim: 2,
        query_heads: 1,
        kv_heads: 1,
        head_dim: 2,
        compress_ratio: 2,
        token_budget: 2,
        epsilon: 1e-6,
    };
    let prefill = qsa_prefill(
        QsaPrefillInput {
            hidden_states: &hidden,
            hidden_width: 2,
            positions: &positions,
            triangular_mask: &mask,
        },
        geometry,
        &rope,
        &rope,
        weights,
    )?;
    assert_eq!(
        prefill.selected_token_indices,
        [vec![0], vec![1], vec![1, 2]]
    );
    assert_eq!(prefill.raw_index_key_cache.len(), 6);
    assert_eq!(prefill.attention_keys.len(), 6);
    assert_eq!(prefill.attention_values.len(), 6);

    let mut raw_keys = Vec::new();
    let mut keys = Vec::new();
    let mut values = Vec::new();
    let mut outputs = Vec::new();
    for token in 0..3 {
        let start = token * 2;
        let row_start = token * (token + 1) / 2;
        let decoded = qsa_decode_step(
            &hidden[start..start + 2],
            &raw_keys,
            &positions[..token],
            &mask[row_start..row_start + token + 1],
            &keys,
            &values,
            geometry.indexer_query_heads,
            geometry.indexer_head_dim,
            geometry.query_heads,
            geometry.kv_heads,
            geometry.head_dim,
            geometry.compress_ratio,
            geometry.token_budget,
            geometry.epsilon,
            &rope,
            &rope,
            positions[token],
            weights,
        )?;
        outputs.extend(decoded.output);
        raw_keys = decoded.raw_index_key_cache;
        keys = decoded.attention_keys;
        values = decoded.attention_values;
    }
    close(&prefill.outputs, &outputs);
    close(&prefill.raw_index_key_cache, &raw_keys);
    close(&prefill.attention_keys, &keys);
    close(&prefill.attention_values, &values);
    Ok(())
}

#[test]
fn qsa_cached_prefill_matches_continuous_prefill_after_prefix() -> Result<()> {
    let rope = TextRotaryEmbedding::new(2, 2, 10_000.0)?;
    let weights = QsaDecodeWeights {
        indexer: QsaIndexerWeights {
            projection: &[1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            query_norm_offset: &[0.0, 0.0],
            key_norm_offset: &[0.0, 0.0],
        },
        attention: QsaAttentionWeights {
            query_gate_projection: &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
            key_projection: &[1.0, 0.0, 0.0, 1.0],
            value_projection: &[2.0, 0.0, 0.0, 2.0],
            query_norm_offset: &[0.0, 0.0],
            key_norm_offset: &[0.0, 0.0],
            output_projection: &[1.0, 0.0, 0.0, 1.0],
        },
    };
    let hidden = [1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.5, 1.0];
    let positions = [0, 1, 2, 3];
    let triangular_mask = [
        true, false, true, false, true, true, true, false, true, true,
    ];
    let geometry = QsaDecodeGeometry {
        indexer_query_heads: 1,
        indexer_head_dim: 2,
        query_heads: 1,
        kv_heads: 1,
        head_dim: 2,
        compress_ratio: 2,
        token_budget: 2,
        epsilon: 1e-6,
    };
    let full = qsa_prefill(
        QsaPrefillInput {
            hidden_states: &hidden,
            hidden_width: 2,
            positions: &positions,
            triangular_mask: &triangular_mask,
        },
        geometry,
        &rope,
        &rope,
        weights,
    )?;

    let mut raw_keys = Vec::new();
    let mut attention_keys = Vec::new();
    let mut attention_values = Vec::new();
    for token in 0..2 {
        let row_start = token * (token + 1) / 2;
        let decoded = qsa_decode_step(
            &hidden[token * 2..token * 2 + 2],
            &raw_keys,
            &positions[..token],
            &triangular_mask[row_start..row_start + token + 1],
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
            &rope,
            &rope,
            positions[token],
            weights,
        )?;
        raw_keys = decoded.raw_index_key_cache;
        attention_keys = decoded.attention_keys;
        attention_values = decoded.attention_values;
    }
    let suffix_mask = [false, true, true, false, true, false, true, true];
    let cached = qsa_prefill_with_cache(
        QsaCachedPrefillInput {
            hidden_states: &hidden[4..],
            hidden_width: 2,
            prefix_positions: &positions[..2],
            prefix_raw_index_keys: &raw_keys,
            prefix_attention_keys: &attention_keys,
            prefix_attention_values: &attention_values,
            positions: &positions[2..],
            visibility_mask: &suffix_mask,
        },
        geometry,
        &rope,
        &rope,
        weights,
    )?;
    close(&cached.outputs, &full.outputs[4..]);
    assert_eq!(
        cached.selected_token_indices,
        full.selected_token_indices[2..]
    );
    close(&cached.raw_index_key_cache, &full.raw_index_key_cache);
    close(&cached.attention_keys, &full.attention_keys);
    close(&cached.attention_values, &full.attention_values);
    let invalid_future_mask = [false, true, true, true, true, false, true, true];
    assert!(qsa_prefill_with_cache(
        QsaCachedPrefillInput {
            hidden_states: &hidden[4..],
            hidden_width: 2,
            prefix_positions: &positions[..2],
            prefix_raw_index_keys: &raw_keys,
            prefix_attention_keys: &attention_keys,
            prefix_attention_values: &attention_values,
            positions: &positions[2..],
            visibility_mask: &invalid_future_mask,
        },
        geometry,
        &rope,
        &rope,
        weights,
    )
    .is_err());
    Ok(())
}

#[test]
fn ple_gate_matches_signed_root_and_grouped_norm() -> Result<()> {
    let result = ple_gate(
        &[1.0, 0.0, 0.0, 1.0],
        &[0.0; 4],
        &[1.0, 0.0, 0.0, 1.0],
        &[0.0; 4],
        &[2.0, -4.0],
        &[0.0; 4],
        2,
        0.5,
    )?;
    let gate = 1.0 / (1.0 + (-(1.0 / 2.0_f32.sqrt()).sqrt()).exp());
    close(
        &result.gated_value,
        &[2.0 * gate, -4.0 * gate, 2.0 * gate, -4.0 * gate],
    );
    close(&result.conv_input, &{
        let denominator = (10.0 * gate * gate + 0.5).sqrt();
        [
            2.0 * gate / denominator,
            -4.0 * gate / denominator,
            2.0 * gate / denominator,
            -4.0 * gate / denominator,
        ]
    });

    // The source uses sign(0)=0 before the sigmoid, so an orthogonal stream
    // receives exactly a half-strength gate.
    let zero_dot = ple_gate(
        &[1.0, 0.0],
        &[0.0; 2],
        &[0.0, 1.0],
        &[0.0; 2],
        &[2.0, 4.0],
        &[0.0; 2],
        2,
        0.5,
    )?;
    close(&zero_dot.gated_value, &[1.0, 2.0]);
    assert!(ple_gate(&[1.0], &[0.0], &[1.0], &[0.0], &[1.0], &[0.0], 2, 1e-6).is_err());
    assert!(ple_gate(&[1.0], &[0.0], &[1.0], &[0.0], &[1.0], &[0.0], 1, 0.0).is_err());
    Ok(())
}

#[test]
fn ple_step_projects_gates_updates_padding_state_and_commits_atomically() -> Result<()> {
    let weights = PleLayerWeights {
        key_projection: &[1.0],
        value_projection: &[2.0],
        key_norm_offset: &[0.0],
        query_norm_offset: &[0.0],
        conv_norm_offset: &[0.0],
        conv_weights: &[0.0; 4],
    };
    let mut convolution = DepthwiseConvState::new(1, 4, 3)?;
    let output = ple_step(&[1.0], &[1.0], 1, 1.0, false, &mut convolution, weights)?;
    let gate = 1.0 / (1.0 + (-0.5_f32.sqrt()).exp());
    close(&output, &[2.0 * gate]);
    let conv_input = (2.0 * gate) / ((2.0 * gate).powi(2) + 1.0).sqrt();
    assert_eq!(convolution.history().len(), 9);
    close(&convolution.history()[8..], &[conv_input]);

    let padded = ple_step(&[1.0], &[1.0], 1, 1.0, true, &mut convolution, weights)?;
    close(&padded, &[0.0]);
    close(&convolution.history()[7..], &[conv_input, 0.0]);

    let overflow_weights = PleLayerWeights {
        key_projection: &[1.0],
        value_projection: &[f32::MAX],
        key_norm_offset: &[0.0],
        query_norm_offset: &[0.0],
        conv_norm_offset: &[0.0],
        conv_weights: &[0.0, 0.0, 0.0, f32::MAX],
    };
    let mut transactional = DepthwiseConvState::new(1, 4, 3)?;
    let initial = transactional.clone();
    assert!(ple_step(
        &[1.0],
        &[1.0],
        1,
        1e-6,
        false,
        &mut transactional,
        overflow_weights,
    )
    .is_err());
    assert_eq!(transactional, initial);
    Ok(())
}

#[test]
fn row_backed_ple_projection_matches_materialized_step() -> Result<()> {
    let weights = PleLayerWeights {
        key_projection: &[1.0],
        value_projection: &[2.0],
        key_norm_offset: &[0.0],
        query_norm_offset: &[0.0],
        conv_norm_offset: &[0.0],
        conv_weights: &[0.0; 4],
    };
    let mut dense_state = DepthwiseConvState::new(1, 4, 3)?;
    let expected = ple_step(&[1.0], &[1.0], 1, 1.0, false, &mut dense_state, weights)?;
    let mut key_rows = TestLinearRows {
        values: vec![1.0],
        output_features: 1,
        input_features: 1,
    };
    let mut value_rows = TestLinearRows {
        values: vec![2.0],
        output_features: 1,
        input_features: 1,
    };
    let mut row_state = DepthwiseConvState::new(1, 4, 3)?;
    let actual = ple_step_from_rows(
        &[1.0],
        &[1.0],
        1,
        1.0,
        false,
        &mut row_state,
        &mut key_rows,
        &mut value_rows,
        &[0.0],
        &[0.0],
        &[0.0],
        &[0.0; 4],
    )?;
    close(&actual, &expected);
    assert_eq!(row_state, dense_state);
    Ok(())
}

#[test]
fn dilated_depthwise_convolution_preserves_chunk_and_error_state() -> Result<()> {
    let weights = [2.0, 3.0];
    let mut whole = DepthwiseConvState::new(1, 2, 2)?;
    let all = whole.process(&[1.0, 2.0, 3.0], &weights)?;
    let silu = |value: f32| value / (1.0 + (-value).exp());
    close(&all, &[silu(3.0), silu(6.0), silu(11.0)]);

    let mut chunked = DepthwiseConvState::new(1, 2, 2)?;
    let mut split = chunked.process(&[1.0], &weights)?;
    split.extend(chunked.process(&[2.0, 3.0], &weights)?);
    close(&split, &all);
    assert_eq!(chunked.history(), &[2.0, 3.0]);

    let mut state = DepthwiseConvState::new(1, 2, 1)?;
    state.process(&[1.0], &[1.0, 1.0])?;
    let before = state.clone();
    assert!(state.process(&[f32::MAX], &[0.0, 2.0]).is_err());
    assert_eq!(state, before);
    assert!(DepthwiseConvState::new(0, 2, 1).is_err());
    assert!(DepthwiseConvState::new(usize::MAX, 2, 2).is_err());
    Ok(())
}

#[test]
fn delta_rule_decay_prediction_readout_and_snapshot() -> Result<()> {
    let mut state = DeltaState::new(2, 2)?;
    // Orthogonal unit keys make the two rows independently observable.
    let a = 1.0_f64 / (1.0_f64 + 1e-6).sqrt();
    let s = 2.0_f64.sqrt();
    close(
        &state.step(&[1.0, 0.0], &[1.0, 0.0], &[2.0, 4.0], 0.0, 0.5)?,
        &[(a * a / s) as f32, (2.0 * a * a / s) as f32],
    );
    close(state.values(), &[a as f32, (2.0 * a) as f32, 0.0, 0.0]);
    let saved = state.clone();
    let second = state.step(&[1.0, 0.0], &[0.0, 1.0], &[6.0, 8.0], 0.5_f32.ln(), 0.5)?;
    close(&second, &[(0.5 * a * a / s) as f32, (a * a / s) as f32]);
    close(
        state.values(),
        &[
            (0.5 * a) as f32,
            a as f32,
            (3.0 * a) as f32,
            (4.0 * a) as f32,
        ],
    );
    let mut restored = saved.clone();
    assert_eq!(
        restored.step(&[1.0, 0.0], &[0.0, 1.0], &[6.0, 8.0], 0.5_f32.ln(), 0.5)?,
        second
    );
    // Reusing the same key must subtract the predicted value, not just accumulate.
    let mut same_key = saved;
    same_key.step(&[1.0, 0.0], &[1.0, 0.0], &[2.0, 4.0], 0.0, 0.5)?;
    let row = a + a * (2.0 - a * a) * 0.5;
    close(
        same_key.values(),
        &[row as f32, (2.0 * row) as f32, 0.0, 0.0],
    );
    Ok(())
}

#[test]
fn invalid_recurrence_is_atomic() -> Result<()> {
    let mut state = DeltaState::new(1, 1)?;
    state.step(&[1.0], &[1.0], &[2.0], 0.0, 1.0)?;
    let before = state.clone();
    for (q, g, b) in [
        (f32::NAN, 0.0, 1.0),
        (f32::MAX, 0.0, 1.0),
        (1.0, 1.0, 1.0),
        (1.0, 0.0, -0.1),
    ] {
        assert!(state.step(&[q], &[1.0], &[1.0], g, b).is_err());
        assert_eq!(state, before);
    }
    assert!(state.step(&[], &[1.0], &[1.0], 0.0, 0.5).is_err());
    assert_eq!(state, before);
    assert!(DeltaState::new(0, 1).is_err());
    assert!(DeltaState::new(usize::MAX, 2).is_err());
    Ok(())
}

#[test]
fn multihead_delta_repeats_key_heads_and_commits_atomically() -> Result<()> {
    let mut multi = GatedDeltaState::new(1, 2, 1, 1)?;
    let output = multi.step(&[1.0], &[1.0], &[2.0, 4.0], &[0.0, 0.0], &[0.5, 0.5])?;
    let mut first = DeltaState::new(1, 1)?;
    let mut second = DeltaState::new(1, 1)?;
    close(
        &output,
        &[
            first.step(&[1.0], &[1.0], &[2.0], 0.0, 0.5)?[0],
            second.step(&[1.0], &[1.0], &[4.0], 0.0, 0.5)?[0],
        ],
    );

    let mut atomic = GatedDeltaState::new(1, 2, 1, 1)?;
    atomic.step(&[1.0], &[1.0], &[1.0, f32::MAX], &[0.0, 0.0], &[1.0, 1.0])?;
    let before = atomic.clone();
    assert!(atomic
        .step(&[1.0], &[1.0], &[1.0, -f32::MAX], &[0.0, 0.0], &[0.5, 1.0],)
        .is_err());
    assert_eq!(atomic, before);
    assert!(GatedDeltaState::new(2, 3, 1, 1).is_err());
    Ok(())
}

#[test]
fn gated_delta_net_single_token_pipeline_matches_source_equations() -> Result<()> {
    let qkv_projection = [1.0, 1.0, 2.0];
    let z_projection = [0.0];
    let beta_projection = [0.0];
    let decay_projection = [0.0];
    let convolution = [0.5; 6];
    let a_log = [0.0];
    let dt_bias = [0.0];
    let norm = [1.0];
    let output_projection = [3.0];
    let weights = GatedDeltaNetWeights {
        qkv_projection: &qkv_projection,
        z_projection: &z_projection,
        beta_projection: &beta_projection,
        decay_projection: &decay_projection,
        convolution: &convolution,
        a_log: &a_log,
        dt_bias: &dt_bias,
        norm: &norm,
        output_projection: &output_projection,
    };
    let mut state = GatedDeltaNetState::new(1, 1, 1, 1, 1, 2)?;
    let first = state.step(&[1.0], &weights, 1e-6)?[0];
    let second = state.step(&[1.0], &weights, 1e-6)?[0];

    let q1 = 0.5 / (1.0 + (-0.5_f32).exp());
    let v1 = 1.0 / (1.0 + (-1.0_f32).exp());
    let q2 = 1.0 / (1.0 + (-1.0_f32).exp());
    let v2 = 2.0 / (1.0 + (-2.0_f32).exp());
    let q1_norm = q1 / (q1 * q1 + 1e-6).sqrt();
    let q2_norm = q2 / (q2 * q2 + 1e-6).sqrt();
    let state1 = q1_norm * 0.5 * v1;
    let core1 = q1_norm * state1;
    let predicted = q2_norm * (0.5 * state1);
    let state2 = 0.5 * state1 + q2_norm * (0.5 * (v2 - predicted));
    let core2 = q2_norm * state2;
    close(
        &[first, second],
        &[
            1.5 * core1 / (core1 * core1 + 1e-6).sqrt(),
            1.5 * core2 / (core2 * core2 + 1e-6).sqrt(),
        ],
    );
    assert_eq!(state.convolution_history(), &[1.0, 1.0, 2.0]);
    close(state.recurrent_states()[0].values(), &[state2]);

    let (decay, beta) =
        gated_delta_parameters(&[0.0, 1000.0], &[0.0, -1000.0], &[0.0; 2], &[0.0; 2])?;
    close(&decay, &[-std::f32::consts::LN_2, -1000.0]);
    close(&beta, &[0.5, 0.0]);

    let mut atomic = GatedDeltaNetState::new(1, 1, 1, 1, 1, 2)?;
    let bad_norm = [3.0];
    let bad_output = [f32::MAX];
    let bad_weights = GatedDeltaNetWeights {
        norm: &bad_norm,
        output_projection: &bad_output,
        ..weights
    };
    let before = atomic.clone();
    assert!(atomic.step(&[1.0], &bad_weights, 1e-6).is_err());
    assert_eq!(atomic, before);
    Ok(())
}

#[test]
fn gated_delta_net_row_backed_matches_dense_step() -> Result<()> {
    let qkv = [1.0, 1.0, 2.0];
    let z = [0.0];
    let beta = [0.0];
    let decay = [0.0];
    let output = [3.0];
    let convolution = [0.5; 6];
    let a_log = [0.0];
    let dt_bias = [0.0];
    let norm = [1.0];
    let weights = GatedDeltaNetWeights {
        qkv_projection: &qkv,
        z_projection: &z,
        beta_projection: &beta,
        decay_projection: &decay,
        convolution: &convolution,
        a_log: &a_log,
        dt_bias: &dt_bias,
        norm: &norm,
        output_projection: &output,
    };
    let rows = |values: &[f32], out, input| TestLinearRows {
        values: values.to_vec(),
        output_features: out,
        input_features: input,
    };
    let mut qkv_rows = rows(&qkv, 3, 1);
    let mut z_rows = rows(&z, 1, 1);
    let mut beta_rows = rows(&beta, 1, 1);
    let mut decay_rows = rows(&decay, 1, 1);
    let mut output_rows = rows(&output, 1, 1);
    let mut dense = GatedDeltaNetState::new(1, 1, 1, 1, 1, 2)?;
    let mut streamed = dense.clone();
    let dense_output = dense.step(&[1.0], &weights, 1e-6)?;
    let streamed_output = streamed.step_from_rows(
        &[1.0],
        &mut qkv_rows,
        &mut z_rows,
        &mut beta_rows,
        &mut decay_rows,
        &convolution,
        &a_log,
        &dt_bias,
        &norm,
        &mut output_rows,
        1e-6,
    )?;
    close(&streamed_output, &dense_output);
    assert_eq!(streamed, dense);

    let mut qkv_rows = rows(&qkv, 3, 1);
    let mut z_rows = rows(&z, 1, 1);
    let mut beta_rows = rows(&beta, 1, 1);
    let mut decay_rows = rows(&decay, 1, 1);
    let mut output_rows = rows(&output, 1, 1);
    let mut dense_prefill = GatedDeltaNetState::new(1, 1, 1, 1, 1, 2)?;
    let mut streamed_prefill = dense_prefill.clone();
    let dense_values = dense_prefill.process(&[1.0, 1.0], &weights, 1e-6)?;
    let streamed_values = streamed_prefill.process_from_rows(
        &[1.0, 1.0],
        &mut qkv_rows,
        &mut z_rows,
        &mut beta_rows,
        &mut decay_rows,
        &convolution,
        &a_log,
        &dt_bias,
        &norm,
        &mut output_rows,
        1e-6,
    )?;
    close(&streamed_values, &dense_values);
    assert_eq!(streamed_prefill, dense_prefill);

    let mut failing_qkv = FailAfterLinearRows {
        inner: rows(&qkv, 3, 1),
        reads: 0,
        fail_at: 3,
    };
    let mut z_rows = rows(&z, 1, 1);
    let mut beta_rows = rows(&beta, 1, 1);
    let mut decay_rows = rows(&decay, 1, 1);
    let mut output_rows = rows(&output, 1, 1);
    let mut atomic_prefill = GatedDeltaNetState::new(1, 1, 1, 1, 1, 2)?;
    let before = atomic_prefill.clone();
    assert!(atomic_prefill
        .process_from_rows(
            &[1.0, 1.0],
            &mut failing_qkv,
            &mut z_rows,
            &mut beta_rows,
            &mut decay_rows,
            &convolution,
            &a_log,
            &dt_bias,
            &norm,
            &mut output_rows,
            1e-6,
        )
        .is_err());
    assert_eq!(atomic_prefill, before);
    Ok(())
}

#[test]
fn gated_delta_net_prefill_matches_steps_and_rolls_back_failed_chunks() -> Result<()> {
    let weights = GatedDeltaNetWeights {
        qkv_projection: &[1.0, 1.0, 2.0],
        z_projection: &[0.0],
        beta_projection: &[0.0],
        decay_projection: &[0.0],
        convolution: &[0.5; 6],
        a_log: &[0.0],
        dt_bias: &[0.0],
        norm: &[1.0],
        output_projection: &[3.0],
    };
    let mut prefill = GatedDeltaNetState::new(1, 1, 1, 1, 1, 2)?;
    let batched = prefill.process(&[1.0, 1.0], &weights, 1e-6)?;
    let mut stepwise = GatedDeltaNetState::new(1, 1, 1, 1, 1, 2)?;
    let first = stepwise.step(&[1.0], &weights, 1e-6)?;
    let second = stepwise.step(&[1.0], &weights, 1e-6)?;
    close(&batched, &[first[0], second[0]]);
    assert_eq!(prefill, stepwise);

    let mut atomic = GatedDeltaNetState::new(1, 1, 1, 1, 1, 2)?;
    let before = atomic.clone();
    assert!(atomic.process(&[1.0, f32::MAX], &weights, 1e-6).is_err());
    assert_eq!(atomic, before);
    Ok(())
}

#[test]
fn gated_delta_value_head_reorder_matches_pinned_group_to_tiled_order() -> Result<()> {
    let grouped = (0..12).map(|value| value as f32).collect::<Vec<_>>();
    let tiled = reorder_gdn_value_heads_axis0(&grouped, 2, 3, 2, 1)?;
    close(
        &tiled,
        &[0.0, 1.0, 6.0, 7.0, 2.0, 3.0, 8.0, 9.0, 4.0, 5.0, 10.0, 11.0],
    );

    let mut qkv = vec![100.0, 101.0, 102.0, 103.0];
    qkv.extend((0..24).map(|value| value as f32));
    let reordered_qkv = reorder_gdn_qkv_value_rows(&qkv, 2, 2, 3, 2, 2)?;
    close(
        &reordered_qkv,
        &[
            100.0, 101.0, 102.0, 103.0, 0.0, 1.0, 2.0, 3.0, 12.0, 13.0, 14.0, 15.0, 4.0, 5.0, 6.0,
            7.0, 16.0, 17.0, 18.0, 19.0, 8.0, 9.0, 10.0, 11.0, 20.0, 21.0, 22.0, 23.0,
        ],
    );

    let columns = (0..12).map(|value| value as f32).collect::<Vec<_>>();
    let reordered_columns = reorder_gdn_value_head_columns(&columns, 1, 2, 3, 2)?;
    close(
        &reordered_columns,
        &[0.0, 1.0, 6.0, 7.0, 2.0, 3.0, 8.0, 9.0, 4.0, 5.0, 10.0, 11.0],
    );
    Ok(())
}

#[test]
fn gated_delta_value_head_reorder_rejects_bad_geometry_and_non_finite_values() {
    assert!(reorder_gdn_value_heads_axis0(&[1.0], 0, 1, 1, 1).is_err());
    assert!(reorder_gdn_value_heads_axis0(&[1.0], 1, 1, 1, 2).is_err());
    assert!(reorder_gdn_value_heads_axis0(&[f32::NAN], 1, 1, 1, 1).is_err());
    assert!(reorder_gdn_value_head_columns(&[1.0], 0, 1, 1, 1).is_err());
}

#[test]
fn pinned_converter_tensor_splits_and_small_transforms_match_layouts() -> Result<()> {
    let fused_indexer = (0..6).map(|value| value as f32).collect::<Vec<_>>();
    let (query, key) = split_indexer_qk_rows(&fused_indexer, 2, 1, 2)?;
    close(&query, &[0.0, 1.0, 2.0, 3.0]);
    close(&key, &[4.0, 5.0]);

    let fused_experts = (0..16).map(|value| value as f32).collect::<Vec<_>>();
    let (gate, up) = split_expert_gate_up(&fused_experts, 2, 2, 2)?;
    close(&gate, &[0.0, 1.0, 2.0, 3.0, 8.0, 9.0, 10.0, 11.0]);
    close(&up, &[4.0, 5.0, 6.0, 7.0, 12.0, 13.0, 14.0, 15.0]);

    close(&add_one_to_norm(&[-1.0, 0.0, 0.5])?, &[0.0, 1.0, 1.5]);
    let convolution = (0..6).map(|value| value as f32).collect::<Vec<_>>();
    close(
        &squeeze_ple_convolution(&convolution, 2, 1, 3)?,
        &convolution,
    );
    Ok(())
}

#[test]
fn pinned_converter_tensor_transforms_reject_malformed_geometry() {
    assert!(split_indexer_qk_rows(&[0.0], 2, 1, 2).is_err());
    assert!(split_expert_gate_up(&[0.0], 2, 2, 2).is_err());
    assert!(add_one_to_norm(&[f32::INFINITY]).is_err());
    assert!(squeeze_ple_convolution(&[0.0], 1, 2, 1).is_err());
}
