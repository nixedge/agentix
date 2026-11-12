//! Network-free RustingStrata execution and Agentix Qwen prompt handling.
#![forbid(unsafe_code)]

pub mod budget;
pub mod engine;
pub mod tokenizer;

#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    Overflow,
    Capacity(&'static str),
    Io(std::io::Error),
    Backend(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid Qwen input: {message}"),
            Self::Overflow => f.write_str("Qwen size arithmetic overflow"),
            Self::Capacity(tier) => write!(f, "insufficient {tier} capacity"),
            Self::Io(error) => error.fmt(f),
            Self::Backend(message) => write!(f, "RustingStrata: {message}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
pub type Result<T> = std::result::Result<T, Error>;
