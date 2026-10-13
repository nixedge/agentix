use agentix_qwen_sys::QuantizedRows;
use std::{
    env,
    error::Error,
    io::{self, Write},
    path::PathBuf,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let path = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("usage: decode_gguf_row <gguf-shard> <tensor-name> <row-width> <row-index>")?;
    let tensor_name = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or("tensor name must be valid UTF-8")?;
    let row_width = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or("missing row width")?
        .parse::<usize>()?;
    let row_index = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or("missing row index")?
        .parse::<u64>()?;
    if arguments.next().is_some() {
        return Err(
            "usage: decode_gguf_row <gguf-shard> <tensor-name> <row-width> <row-index>".into(),
        );
    }

    let rows = QuantizedRows::open(&path, &tensor_name, row_width)?;
    let decoded = rows.read_row(row_index)?;
    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    writeln!(
        output,
        "# row={row_index}\twidth={}\tgroups={}\trows_per_group={}",
        decoded.len(),
        rows.group_count(),
        rows.rows_per_group(),
    )?;
    writeln!(output, "column\tf32")?;
    for (column, value) in decoded.iter().enumerate() {
        writeln!(output, "{column}\t{value:.9e}")?;
    }
    Ok(())
}
