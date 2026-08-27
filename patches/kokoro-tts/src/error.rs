use {
    crate::G2PError,
    bincode::error::DecodeError,
    ndarray::ShapeError,
    ort::Error as OrtError,
    std::{
        error::Error,
        fmt::{Debug, Display, Formatter, Result as FmtResult},
        io::Error as IoError,
        time::SystemTimeError,
    },
};

#[derive(Debug)]
pub enum KokoroError {
    Decode(DecodeError),
    G2P(G2PError),
    Io(IoError),
    ModelReleased,
    Ort(String),
    Send(String),
    Shape(ShapeError),
    SystemTime(SystemTimeError),
    VoiceNotFound(String),
    VoiceVersionInvalid(String),
}

impl Display for KokoroError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "KokoroError: ")?;
        match self {
            Self::Decode(e) => Display::fmt(e, f),
            Self::G2P(e) => Display::fmt(e, f),
            Self::Io(e) => Display::fmt(e, f),
            Self::Ort(msg) => write!(f, "Ort({})", msg),
            Self::ModelReleased => write!(f, "ModelReleased"),
            Self::Send(msg) => write!(f, "Send({})", msg),
            Self::Shape(e) => Display::fmt(e, f),
            Self::SystemTime(e) => Display::fmt(e, f),
            Self::VoiceNotFound(name) => write!(f, "VoiceNotFound({})", name),
            Self::VoiceVersionInvalid(msg) => write!(f, "VoiceVersionInvalid({})", msg),
        }
    }
}

impl Error for KokoroError {}

impl From<IoError> for KokoroError {
    fn from(value: IoError) -> Self {
        Self::Io(value)
    }
}

impl From<DecodeError> for KokoroError {
    fn from(value: DecodeError) -> Self {
        Self::Decode(value)
    }
}

impl<T> From<OrtError<T>> for KokoroError {
    fn from(value: OrtError<T>) -> Self {
        Self::Ort(value.to_string())
    }
}

impl From<G2PError> for KokoroError {
    fn from(value: G2PError) -> Self {
        Self::G2P(value)
    }
}

impl From<ShapeError> for KokoroError {
    fn from(value: ShapeError) -> Self {
        Self::Shape(value)
    }
}

impl From<SystemTimeError> for KokoroError {
    fn from(value: SystemTimeError) -> Self {
        Self::SystemTime(value)
    }
}
