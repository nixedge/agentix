//! Compare the pinned CPU row projection with CUDA on the actual GGUF output matrix.
//!
//! Usage: cargo run -p agentix-qwen-sys --features cuda --example \
//!   compare_cuda_gguf_matvec -- CONFIG.json SHARD1.gguf SHARD2.gguf [DEVICE]

use agentix_qwen::{config::QwenTextConfig, reference::linear_from_rows};
use agentix_qwen_sys::{
    cuda_devices, cuda_quantized_gguf_matvec, QwenGgufTensorStore, QwenStorageType,
};
use std::{env, error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let config_path = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("missing CONFIG.json argument")?;
    let shard_paths = [
        arguments
            .next()
            .map(PathBuf::from)
            .ok_or("missing SHARD1.gguf argument")?,
        arguments
            .next()
            .map(PathBuf::from)
            .ok_or("missing SHARD2.gguf argument")?,
    ];
    let device_index = arguments
        .next()
        .map(|value| value.to_string_lossy().parse::<usize>())
        .transpose()?;
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }
    let device_index = device_index.unwrap_or(0);

    let config_bytes = std::fs::read(config_path)?;
    let config = QwenTextConfig::parse(&config_bytes)?;
    let store = QwenGgufTensorStore::open(&shard_paths, &config)?;
    let output_entry = store
        .tensor_index()
        .entries
        .iter()
        .find(|entry| entry.name == "output.weight")
        .ok_or("output.weight is missing from the validated tensor index")?;
    if output_entry.storage != QwenStorageType::Q6K {
        return Err("output.weight is not the pinned Q6_K comparison tensor".into());
    }
    let mut cpu_matrix = store.open_matrix_rows("output.weight")?;
    let cuda_matrix = store.open_matrix_rows("output.weight")?;
    if cpu_matrix.input_features() != config.hidden_size
        || usize::try_from(cpu_matrix.output_features()).ok() != Some(config.vocab_size)
    {
        return Err("output.weight geometry does not match Qwen config".into());
    }
    let devices = cuda_devices();
    let device = devices
        .get(device_index)
        .ok_or("requested GGML CUDA device is unavailable")?;

    let input = (0..config.hidden_size)
        .map(|index| ((index * 17 % 101) as f32 - 50.0) / 50.0)
        .collect::<Vec<_>>();
    let cpu_logits = linear_from_rows(&input, &mut cpu_matrix, None)?;
    let cuda_logits = cuda_quantized_gguf_matvec(device_index, &cuda_matrix, &input)?;
    if cpu_logits.len() != cuda_logits.len()
        || cpu_logits.iter().any(|value| !value.is_finite())
        || cuda_logits.iter().any(|value| !value.is_finite())
    {
        return Err("CPU or CUDA projection returned malformed logits".into());
    }

    let mut max_absolute_error = 0.0_f32;
    let mut max_relative_error = 0.0_f32;
    for (&cpu, &cuda) in cpu_logits.iter().zip(&cuda_logits) {
        let absolute = (cpu - cuda).abs();
        let relative = absolute / cpu.abs().max(1.0e-8);
        max_absolute_error = max_absolute_error.max(absolute);
        max_relative_error = max_relative_error.max(relative);
    }
    let cpu_top1 = argmax(&cpu_logits);
    let cuda_top1 = argmax(&cuda_logits);

    println!("device={}", device.name);
    println!("matrix=output.weight");
    println!("storage={}", output_entry.storage.type_name());
    println!("rows={}", cuda_matrix.output_features());
    println!("columns={}", cuda_matrix.input_features());
    println!("max_absolute_error={max_absolute_error:.9e}");
    println!("max_relative_error={max_relative_error:.9e}");
    println!("cpu_top1={cpu_top1}");
    println!("cuda_top1={cuda_top1}");
    println!("top1_match={}", cpu_top1 == cuda_top1);
    println!("numeric_tolerance=not_set");
    Ok(())
}

fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for index in 1..values.len() {
        if values[index] > values[best] {
            best = index;
        }
    }
    best
}
