//! Errors from protocol encoding, framing, and decoding.

use std::fmt;

/// A protocol failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A read needed more bytes than the buffer held.
    UnexpectedEof { needed: usize, available: usize },
    /// A VarInt used more than five bytes.
    VarIntTooLong,
    /// A VarLong used more than ten bytes.
    VarLongTooLong,
    /// A string's encoded length or character count exceeded the packet limit.
    StringTooLong { length: usize, max: usize },
    /// A string was not valid UTF-8.
    InvalidUtf8,
    /// A length prefix was negative.
    NegativeLength(i32),
    /// A frame or packet exceeded the size ceiling.
    PacketTooLarge { size: usize, max: usize },
    /// A packet body had bytes left after decoding.
    TrailingBytes(usize),
    /// A packet id is not modelled for the named state.
    UnknownPacket { state: &'static str, id: i32 },
    /// A zlib stream was malformed or did not match its declared size.
    Compression { message: String },
    /// An I/O operation failed.
    Io { message: String },
    /// The peer sent a value the protocol does not allow.
    Protocol { message: String },
    /// The status JSON was malformed.
    Json { message: String },
}

impl Error {
    /// A protocol violation with a human-readable explanation.
    #[must_use]
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol {
            message: message.into(),
        }
    }

    /// A compression failure with a human-readable explanation.
    #[must_use]
    pub fn compression(message: impl Into<String>) -> Self {
        Self::Compression {
            message: message.into(),
        }
    }

    /// An I/O failure with a human-readable explanation.
    #[must_use]
    pub fn io(message: impl Into<String>) -> Self {
        Self::Io {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof { needed, available } => {
                write!(
                    f,
                    "unexpected end of input: needed {needed} bytes, had {available}"
                )
            }
            Self::VarIntTooLong => f.write_str("VarInt is longer than five bytes"),
            Self::VarLongTooLong => f.write_str("VarLong is longer than ten bytes"),
            Self::StringTooLong { length, max } => {
                write!(f, "string is too long: {length} exceeds {max}")
            }
            Self::InvalidUtf8 => f.write_str("string is not valid UTF-8"),
            Self::NegativeLength(length) => write!(f, "negative length prefix {length}"),
            Self::PacketTooLarge { size, max } => {
                write!(f, "packet is too large: {size} exceeds {max}")
            }
            Self::TrailingBytes(count) => {
                write!(f, "packet had {count} trailing bytes")
            }
            Self::UnknownPacket { state, id } => {
                write!(f, "unknown {state} packet id 0x{id:02X}")
            }
            Self::Compression { message } => write!(f, "compression error: {message}"),
            Self::Io { message } => write!(f, "I/O error: {message}"),
            Self::Protocol { message } => write!(f, "protocol error: {message}"),
            Self::Json { message } => write!(f, "status JSON error: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::io(error.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json {
            message: error.to_string(),
        }
    }
}

/// A protocol result.
pub type Result<T> = std::result::Result<T, Error>;
