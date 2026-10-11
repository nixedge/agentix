//! Pure upstream patch primitives. Callers own file access, bounds and approvals.
// SAFETY: preserve upstream fixture assertions; production keeps workspace panic lints.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod file_update;
#[rustfmt::skip]
mod parser;
#[rustfmt::skip]
mod seek_sequence;
#[rustfmt::skip]
mod streaming_parser;
#[rustfmt::skip]
mod text_file;

pub use parser::{Hunk, ParseError, UpdateFileChunk, parse_patch};
pub use streaming_parser::StreamingPatchParser;

#[derive(Debug, PartialEq)]
pub struct ApplyPatchArgs {
    pub patch: String,
    pub hunks: Vec<Hunk>,
    pub workdir: Option<String>,
    pub environment_id: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ApplyPatchError {
    #[error("{0}")]
    ComputeReplacements(String),
}

pub fn update_text(
    path: &str,
    original: &str,
    chunks: &[UpdateFileChunk],
) -> Result<String, ApplyPatchError> {
    let mut source = text_file::SourceFile::parse(original);
    let replacements = file_update::compute_replacements(&source.line_texts(), path, chunks)?;
    source.apply_replacements(&replacements);
    Ok(source.into_contents())
}
