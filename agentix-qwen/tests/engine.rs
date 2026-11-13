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

#[test]
fn management_commands_work_while_a_cpu_session_retains_its_lock() -> Result<(), Box<dyn Error>> {
    let engine = Engine::open(fixture()?, options())?;
    let control = engine.control();
    let mut session = engine.session()?;
    session.prefill(&[1, 2, 3])?;
    assert_eq!(control.set_expert_budget(0)?, 0);
    assert_eq!(control.memory()?.expert_bytes, 0);
    assert!(control.memory()?.free_bytes.is_none());
    assert_eq!(greedy_next_token(&session.step(42)?)?, 42);
    Ok(())
}

#[test]
#[ignore = "requires explicit GPU authorization and external thermal limits"]
fn gpu_expert_release_refill_preserves_the_live_session() -> Result<(), Box<dyn Error>> {
    let backend = std::env::var("AGENTIX_QWEN_GPU_TEST_BACKEND")?;
    if backend != "cuda" {
        return Err("this release acceptance test requires CUDA".into());
    }
    let engine = Engine::open(
        fixture()?,
        Options {
            backend,
            chunk: 64,
            ..options()
        },
    )?;
    let control = engine.control();
    let mut session = engine.session()?;
    let prompt: Vec<u32> = (0..64).map(|i| i % 7 + 1).collect();
    assert_eq!(greedy_next_token(&session.prefill(&prompt)?)?, 42);
    // Warm a captured decode graph before freeing any pointer-bearing buffers.
    assert_eq!(greedy_next_token(&session.step(42)?)?, 42);
    let before = control.memory()?;
    assert!(before.expert_bytes > 0);
    assert_eq!(control.set_expert_budget(0)?, 0);
    let released = control.memory()?;
    assert_eq!(released.expert_bytes, 0);
    assert_eq!(greedy_next_token(&session.step(42)?)?, 42);
    let refilled = control.set_expert_budget(before.expert_bytes)?;
    assert!(refilled > 0 && refilled <= before.expert_bytes);
    assert_eq!(greedy_next_token(&session.step(42)?)?, 42);
    println!(
        "device={}, before={before:?}, released={released:?}, refill_bytes={refilled}",
        engine.info.device
    );
    // Tiny fixture pools may share allocator pages with live weights. Measurable
    // device-level reclamation must also be checked with production expert pools.
    Ok(())
}

#[test]
#[ignore = "requires explicit GPU authorization and a cached production model"]
fn gpu_production_engine_release_refill_continues_session() -> Result<(), Box<dyn Error>> {
    let started = std::time::Instant::now();
    let stage = |name| {
        eprintln!(
            "stage={name}, pid={}, elapsed={:.3}s",
            std::process::id(),
            started.elapsed().as_secs_f64()
        );
    };
    let path = std::env::var_os("AGENTIX_QWEN_PRODUCTION_TEST_MODEL")
        .ok_or("missing explicitly selected production GGUF")?;
    stage("load");
    let engine = Engine::open(
        path.into(),
        Options {
            backend: "cuda".into(),
            context: 128,
            chunk: 64,
            expert_budget: Some(512 * 1024 * 1024),
            ..Options::default()
        },
    )?;
    let control = engine.control();
    let prefix = [1, 2, 3];
    let (first, second, expected) = {
        let mut reference = engine.session()?;
        stage("reference-prefill");
        let first = greedy_next_token(&reference.prefill(&prefix)?)?;
        stage("reference-step-1");
        let second = greedy_next_token(&reference.step(first)?)?;
        stage("reference-step-2");
        (first, second, reference.step(second)?)
    };
    let mut live = engine.session()?;
    stage("live-prefill");
    assert_eq!(greedy_next_token(&live.prefill(&prefix)?)?, first);
    stage("live-step-before-release");
    assert_eq!(greedy_next_token(&live.step(first)?)?, second);
    let before = control.memory()?;
    assert!(before.expert_bytes > 16 * 1024 * 1024);
    stage("release");
    assert_eq!(control.set_expert_budget(0)?, 0);
    let released = control.memory()?;
    eprintln!("before={before:?}, released={released:?}");
    assert_eq!(released.expert_bytes, 0);
    let free_delta = released
        .free_bytes
        .ok_or("missing CUDA free memory")?
        .saturating_sub(before.free_bytes.ok_or("missing CUDA free memory")?);
    assert!(
        free_delta >= 16 * 1024 * 1024,
        "no substantial VRAM release: {free_delta}"
    );
    stage("continue-without-expert-cache");
    let actual = live.step(second)?;
    assert_eq!(actual.len(), expected.len());
    let cpu_next = greedy_next_token(&actual)?;
    let cached_next = greedy_next_token(&expected)?;
    let error: f64 = actual
        .iter()
        .zip(&expected)
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).powi(2))
        .sum();
    let magnitude: f64 = expected.iter().map(|&x| f64::from(x).powi(2)).sum();
    let relative_l2 = (error / magnitude.max(f64::EPSILON)).sqrt();
    // CPU and GPU expert kernels round differently. The separate checkpoint
    // test holds arithmetic constant and checks context preservation at 1e-5.
    assert!(relative_l2.is_finite());
    stage("refill");
    let refill = control.set_expert_budget(before.expert_bytes)?;
    assert!(refill > 0 && refill <= before.expert_bytes);
    let next = greedy_next_token(&actual)?;
    stage("continue-after-refill");
    greedy_next_token(&live.step(next)?)?;
    println!(
        "pid={}, device={}, before={before:?}, released={released:?}, free_delta={free_delta}, refill_bytes={refill}, cross_placement_l2={relative_l2}, cpu_next={cpu_next}, cached_next={cached_next}",
        std::process::id(), engine.info.device,
    );
    Ok(())
}

#[test]
#[ignore = "requires explicit GPU authorization and a cached production model"]
fn gpu_production_reclaim_matches_cpu_checkpoint() -> Result<(), Box<dyn Error>> {
    use rusting_strata::{backend, gguf::Gguf, gpu_model::GpuModel, model::Model};
    let started = std::time::Instant::now();
    let stage = |name| {
        eprintln!(
            "stage={name}, pid={}, elapsed={:.3}s",
            std::process::id(),
            started.elapsed().as_secs_f64()
        );
    };
    let path: PathBuf = std::env::var_os("AGENTIX_QWEN_PRODUCTION_TEST_MODEL")
        .ok_or("missing explicitly selected production GGUF")?
        .into();
    stage("load");
    let gguf = Gguf::open(&path)?;
    let model = Model::load(&gguf)?;
    let mut gpu = GpuModel::load_budgeted(
        &model,
        backend::open(Some("cuda"), 0)?,
        128,
        64,
        512 * 1024 * 1024,
        512 * 1024 * 1024,
    )?;
    gpu.adapt_every = 0;
    gpu.reset()?;
    stage("warm-cached-prefix");
    let mut logits = Vec::new();
    for token in [1, 2, 3] {
        logits = gpu.step(token)?;
    }
    logits = gpu.step(greedy_next_token(&logits)?)?;
    let next = greedy_next_token(&logits)?;
    stage("checkpoint-before-release");
    gpu.save_state()?;
    let position = gpu.pos;
    let cached = gpu.step(next)?;
    gpu.restore_state()?;
    assert_eq!(gpu.pos, position);
    let bytes = gpu.expert_cache_bytes();
    let before = gpu.device_memory()?.0;
    stage("release");
    assert_eq!(gpu.set_expert_budget(0)?, 0);
    assert_eq!(gpu.pos, position);
    let released = gpu.device_memory()?.0.saturating_sub(before);
    assert!(
        released >= 16 * 1024 * 1024,
        "no substantial VRAM release: {released}"
    );
    stage("live-cpu-continuation");
    let live = gpu.step(next)?;
    assert_eq!(gpu.pos, position + 1);
    stage("refill");
    let refill = gpu.set_expert_budget(bytes)?;
    assert!(refill > 0 && refill <= bytes);
    assert_eq!(gpu.pos, position + 1);
    stage("continue-after-refill");
    greedy_next_token(&gpu.step(greedy_next_token(&live)?)?)?;
    // Replay from the original checkpoint only after the uninterrupted live
    // release/refill path has completed. Both compared steps use CPU experts.
    stage("matched-cpu-replay");
    gpu.set_expert_budget(0)?;
    gpu.restore_state()?;
    let replay = gpu.step(next)?;
    assert_eq!(live.len(), replay.len());
    assert_eq!(greedy_next_token(&live)?, greedy_next_token(&replay)?);
    let l2 = |a: &[f32], b: &[f32]| {
        let error: f64 = a
            .iter()
            .zip(b)
            .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
            .sum();
        let magnitude: f64 = b.iter().map(|&x| f64::from(x).powi(2)).sum();
        (error / magnitude.max(f64::EPSILON)).sqrt()
    };
    let matched_error = l2(&live, &replay);
    assert!(
        matched_error < 1e-5,
        "pre-release context was not preserved: {matched_error}"
    );
    println!("device={}, position={position}, released_bytes={released}, refill_bytes={refill}, matched_cpu_l2={matched_error}, cross_placement_l2={}", gpu.gpu.device(), l2(&cached, &replay));
    Ok(())
}
