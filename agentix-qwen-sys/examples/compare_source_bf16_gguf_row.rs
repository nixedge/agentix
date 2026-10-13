//! Compare one directly mapped source BF16 tensor row with its GGUF row.
//!
//! Usage:
//!   compare_source_bf16_gguf_row SOURCE_ROW.bin CONFIG.json TENSOR ROW SHARD...

use agentix_qwen::config::QwenTextConfig;
use agentix_qwen_sys::QwenGgufTensorStore;
use std::{env, error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() < 5 {
        return Err(
            "usage: compare_source_bf16_gguf_row SOURCE_ROW.bin CONFIG.json TENSOR ROW SHARD..."
                .into(),
        );
    }

    let source_row_path = PathBuf::from(arguments[0].as_os_str());
    let config_path = PathBuf::from(arguments[1].as_os_str());
    let tensor_name = arguments[2]
        .to_str()
        .ok_or("tensor name is not valid UTF-8")?;
    let row_index = arguments[3]
        .to_str()
        .ok_or("row index is not valid UTF-8")?
        .parse::<u64>()?;
    let shard_paths = arguments[4..]
        .iter()
        .map(|argument| PathBuf::from(argument.as_os_str()))
        .collect::<Vec<_>>();

    let config = QwenTextConfig::parse(&fs::read(config_path)?)?;
    let store = QwenGgufTensorStore::open(&shard_paths, &config)?;
    let gguf_rows = store.open_rows(tensor_name)?;
    let gguf_values = gguf_rows.read_row(row_index)?;
    let source_bytes = fs::read(source_row_path)?;
    if source_bytes.len() % 2 != 0 || source_bytes.len() / 2 != gguf_values.len() {
        return Err("source BF16 row width does not match GGUF tensor row".into());
    }

    let source_values = source_bytes
        .chunks_exact(2)
        .map(|bytes| {
            let value = u16::from_le_bytes([bytes[0], bytes[1]]);
            f32::from_bits(u32::from(value) << 16)
        })
        .collect::<Vec<_>>();
    if source_values
        .iter()
        .chain(&gguf_values)
        .any(|value| !value.is_finite())
    {
        return Err("source or GGUF row contains a non-finite value".into());
    }

    let mut squared_error = 0.0_f64;
    let mut dot_product = 0.0_f64;
    let mut source_squared = 0.0_f64;
    let mut gguf_squared = 0.0_f64;
    let mut max_abs_error = 0.0_f64;
    for (source, gguf) in source_values.iter().zip(&gguf_values) {
        let source = f64::from(*source);
        let gguf = f64::from(*gguf);
        let error = (source - gguf).abs();
        max_abs_error = max_abs_error.max(error);
        squared_error += error * error;
        dot_product += source * gguf;
        source_squared += source * source;
        gguf_squared += gguf * gguf;
    }
    let width = source_values.len();
    if width == 0 || source_squared == 0.0 || gguf_squared == 0.0 {
        return Err("source or GGUF comparison row has zero norm".into());
    }
    let rmse = (squared_error / width as f64).sqrt();
    let relative_rmse = rmse / (source_squared / width as f64).sqrt();
    let cosine_similarity = dot_product / (source_squared * gguf_squared).sqrt();
    println!(
        "tensor={tensor_name}\trow={row_index}\twidth={width}\tmax_abs_error={max_abs_error:.9e}\trmse={rmse:.9e}\trelative_rmse={relative_rmse:.9e}\tcosine_similarity={cosine_similarity:.9}"
    );
    Ok(())
}
