use agentix_qwen_sys::inspect_gguf;
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
        .ok_or("usage: inspect_gguf_header <gguf-shard>")?;
    if arguments.next().is_some() {
        return Err("usage: inspect_gguf_header <gguf-shard>".into());
    }

    let info = inspect_gguf(&path)?;
    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    writeln!(
        output,
        "# version={}\tarchitecture={}\tsplit={}/{}\tfile_bytes={}\tdata_offset={}\talignment={}",
        info.version,
        info.architecture.as_deref().unwrap_or("unknown"),
        info.split_index
            .map_or_else(|| "none".to_owned(), |index| index.to_string()),
        info.split_count,
        info.file_size,
        info.data_offset,
        info.alignment,
    )?;
    writeln!(
        output,
        "name\ttype\textent_0\textent_1\textent_2\textent_3\ttensor_offset\ttensor_bytes\tfile_offset"
    )?;
    for tensor in info.tensors {
        let file_offset = info
            .data_offset
            .checked_add(tensor.offset)
            .ok_or("tensor file offset overflow")?;
        writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            tensor.name,
            tensor.type_name,
            tensor.extents[0],
            tensor.extents[1],
            tensor.extents[2],
            tensor.extents[3],
            tensor.offset,
            tensor.byte_size,
            file_offset,
        )?;
    }
    Ok(())
}
