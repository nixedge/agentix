//! Independent CPU reference and storage primitives; not yet a full inference backend.
#![forbid(unsafe_code)]

pub mod budget;
pub mod ngram;
pub mod reference;
pub mod rows;

#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    Overflow,
    Capacity(&'static str),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "invalid Qwen input: {reason}"),
            Self::Overflow => f.write_str("Qwen size arithmetic overflow"),
            Self::Capacity(tier) => write!(f, "insufficient {tier} capacity"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
