use agentix_qwen::{
    engine::{Engine, Options},
    tokenizer::greedy_next_token,
};
use std::{
    error::Error,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

fn options() -> Options {
    Options {
        backend: "cpu".into(),
        context: 256,
        chunk: 8,
        ..Options::default()
    }
}
fn fixture() -> Result<PathBuf, Box<dyn Error>> {
    Ok(std::env::var_os("AGENTIX_QWEN_TEST_MODEL")
        .ok_or("missing Nix-pinned Qwen fixture")?
        .into())
}

#[test]
fn quantized_model_prefill_matches_steps_and_resets_sequences() -> Result<(), Box<dyn Error>> {
    let engine = Engine::open(fixture()?, options())?;
    assert_eq!(engine.info.architecture, "qwen4exp");
    assert_eq!(engine.info.vocabulary, 248320);
    let first = engine.session()?.prefill(&[1, 2, 3])?;
    assert_eq!(greedy_next_token(&first)?, 42);
    assert!(first[42] > 1.0);
    assert_eq!(first.iter().filter(|&&v| v != 0.0).count(), 1);
    {
        let mut sequence = engine.session()?;
        sequence.step(1)?;
        sequence.step(2)?;
        assert_eq!(sequence.step(3)?, first);
        sequence.prefill(&[1, 1, 1, 1])?;
    }
    assert_eq!(engine.session()?.prefill(&[1, 2, 3])?, first);
    let one = engine.session()?.step(1)?;
    let two = engine.session()?.step(2)?;
    assert_ne!(one, two);
    Ok(())
}

#[test]
fn cancellation_and_rejected_tokens_do_not_poison_next_request() -> Result<(), Box<dyn Error>> {
    let engine = Engine::open(fixture()?, options())?;
    let baseline = engine.session()?.step(1)?;
    {
        let mut session = engine.session()?;
        assert!(session.step(248320).is_err());
        assert!(session.prefill(&[]).is_err());
        assert!(session.prefill(&[1; 257]).is_err());
        assert_eq!(session.step(1)?, baseline);
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancelled);
        session.set_cancellation_check(move || signal.load(Ordering::Acquire));
        cancelled.store(true, Ordering::Release);
        assert!(session.step(2).is_err());
    }
    assert_eq!(engine.session()?.step(1)?, baseline);
    Ok(())
}

#[test]
fn initialization_fails_without_silent_backend_fallback() -> Result<(), Box<dyn Error>> {
    assert!(Engine::open(
        fixture()?,
        Options {
            backend: "auto".into(),
            ..options()
        }
    )
    .is_err());
    assert!(Engine::open(
        fixture()?,
        Options {
            context: 2049,
            ..options()
        }
    )
    .is_err());
    assert!(Engine::open(PathBuf::from("/nonexistent/agentix-model.gguf"), options()).is_err());
    Ok(())
}

#[test]
fn benchmark_cli_runs_the_pinned_fixture_and_emits_measured_records() -> Result<(), Box<dyn Error>>
{
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agentix-qwen-bench"))
        .arg(fixture()?)
        .env("AGENTIX_QWEN_COMPUTE", "cpu")
        .env("AGENTIX_QWEN_CONTEXT", "256")
        .env("AGENTIX_QWEN_BENCH_TOKENS", "4")
        .env("AGENTIX_QWEN_BENCH_RUNS", "1")
        .env("RAYON_NUM_THREADS", "2")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["architecture"], "qwen4exp");
    assert_eq!(records[1]["warmup"], true);
    assert_eq!(records[2]["warmup"], false);
    assert_eq!(records[2]["token_ids"], serde_json::json!([42, 42, 42, 42]));
    assert!(records[2]["ttft_seconds"].as_f64().is_some_and(|v| v > 0.0));
    assert!(records[2]["decode_tokens_per_second"]
        .as_f64()
        .is_some_and(|v| v > 0.0));
    Ok(())
}

#[test]
#[ignore = "requires explicit GPU access; run with AGENTIX_QWEN_GPU_TEST_BACKEND=cuda or vulkan"]
fn gpu_fixture_matches_cpu_prefill_decode_and_reset() -> Result<(), Box<dyn Error>> {
    let backend = std::env::var("AGENTIX_QWEN_GPU_TEST_BACKEND")?;
    if !matches!(backend.as_str(), "cuda" | "vulkan") {
        return Err("AGENTIX_QWEN_GPU_TEST_BACKEND must be cuda or vulkan".into());
    }
    let fixture_options = Options {
        chunk: 64,
        ..options()
    };
    let cpu = Engine::open(fixture()?, fixture_options.clone())?;
    let gpu = Engine::open(
        fixture()?,
        Options {
            backend,
            ..fixture_options
        },
    )?;
    println!("GPU parity device: {}", gpu.info.device);
    let prompt = [1, 2, 3, 1, 2, 3, 4, 5, 1, 2];
    let mut cpu_session = cpu.session()?;
    let mut gpu_session = gpu.session()?;
    let reference = cpu_session.prefill(&prompt)?;
    let actual = gpu_session.prefill(&prompt)?;
    // GPU GEMV quantizes activations; allow Q8 rounding while requiring every
    // logit to agree within 1% + 1e-3 and the greedy token to match exactly.
    let compare = |reference: &[f32], actual: &[f32]| -> Result<(), Box<dyn Error>> {
        assert_eq!(reference.len(), actual.len());
        let mut maximum_difference = 0_f32;
        for (&expected, &observed) in reference.iter().zip(actual) {
            assert!(observed.is_finite());
            let difference = (expected - observed).abs();
            maximum_difference = maximum_difference.max(difference);
            assert!(
                difference <= 1e-3 + 0.01 * expected.abs(),
                "CPU {expected}, GPU {observed}"
            );
        }
        assert_eq!(greedy_next_token(reference)?, 42);
        assert_eq!(greedy_next_token(actual)?, 42);
        println!("max absolute logit difference: {maximum_difference}");
        Ok(())
    };
    compare(&reference, &actual)?;
    for token in [42, 1, 2, 42] {
        compare(&cpu_session.step(token)?, &gpu_session.step(token)?)?;
    }
    drop(cpu_session);
    drop(gpu_session);
    compare(&reference, &gpu.session()?.prefill(&prompt)?)?;
    // Exceed the upstream short-prompt threshold to exercise batched prefill too.
    let batched: Vec<u32> = prompt.into_iter().cycle().take(64).collect();
    let batched_reference = cpu.session()?.prefill(&batched)?;
    compare(&batched_reference, &gpu.session()?.prefill(&batched)?)?;
    Ok(())
}
