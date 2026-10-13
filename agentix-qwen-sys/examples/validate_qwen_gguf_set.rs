use agentix_qwen::{config::QwenTextConfig, schema::qwen4exp_tensor_geometries};
use agentix_qwen_sys::{inspect_gguf_set, validate_qwen4exp_gguf};
use std::{
    collections::{BTreeMap, BTreeSet},
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
        .ok_or("usage: validate_qwen_gguf_set <qwen-config.json> <gguf-shard>...")?;
    let shard_paths = arguments.map(PathBuf::from).collect::<Vec<_>>();
    if shard_paths.is_empty() {
        return Err("at least one GGUF shard is required".into());
    }

    let config = QwenTextConfig::parse(&fs::read(config_path)?)?;
    let shard_refs = shard_paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let set = inspect_gguf_set(&shard_refs)?;
    let mut combined = set.shards[0].clone();
    combined.tensors = set
        .shards
        .iter()
        .flat_map(|shard| shard.tensors.iter().cloned())
        .collect();
    combined.file_size = set
        .shards
        .iter()
        .map(|shard| shard.file_size)
        .max()
        .ok_or("GGUF set has no shards")?;

    let report = validate_qwen4exp_gguf(&combined, &config)?;
    if !report.is_valid() {
        return Err(format!(
            "Qwen schema mismatch: missing={}, unexpected={}, duplicate={}, shape_mismatch={}",
            report.missing.len(),
            report.unexpected.len(),
            report.duplicate_names.len(),
            report.shape_mismatches.len(),
        )
        .into());
    }

    let expected = qwen4exp_tensor_geometries(&config)?;
    let allowed = [
        "f32", "f16", "bf16", "q4_0", "iq4_nl", "q8_0", "iq2_xs", "iq3_s", "iq2_s", "iq3_xxs",
        "iq4_xs", "q2_0", "q4_k", "q5_k", "q6_k", "q4_K", "q5_K", "q6_K",
    ];
    let mut type_counts = BTreeMap::new();
    let mut unsupported = BTreeSet::new();
    for tensor in &combined.tensors {
        if !allowed.contains(&tensor.type_name.as_str()) {
            unsupported.insert(tensor.type_name.as_str());
        }
        *type_counts
            .entry(tensor.type_name.as_str())
            .or_insert(0_usize) += 1;
    }

    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    writeln!(
        output,
        "architecture=qwen4exp\tshards={}\ttensors={}\tschema_tensors={}\tunsupported_types={}\tvalid={}",
        set.shards.len(),
        combined.tensors.len(),
        expected.len(),
        unsupported.iter().copied().collect::<Vec<_>>().join(","),
        unsupported.is_empty(),
    )?;
    for (storage, count) in type_counts {
        writeln!(output, "{storage}\t{count}")?;
    }
    if !unsupported.is_empty() {
        return Err("GGUF set contains storage types with no Agentix row reader".into());
    }
    Ok(())
}
