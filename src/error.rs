//! Error type shared by every stage of the decomposer.

use std::fmt;
use std::io;

/// Everything that can go wrong while decoding, decomposing, exporting or
/// recomposing audio.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// An underlying I/O failure.
    Io(io::Error),
    /// A container or chunk layout that does not match its specification.
    Format(String),
    /// A well-formed input that this build cannot process yet.
    Unsupported(String),
    /// A textual document (Links Notation, XML, SFZ) that failed to parse.
    Parse(String),
    /// Two operands that must agree on rate, channel count or length do not.
    Mismatch(String),
    /// A configuration value outside its accepted range.
    InvalidArgument(String),
    /// A link store refused an operation.
    Storage(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Format(message) => write!(f, "invalid format: {message}"),
            Self::Unsupported(message) => write!(f, "unsupported: {message}"),
            Self::Parse(message) => write!(f, "parse error: {message}"),
            Self::Mismatch(message) => write!(f, "mismatch: {message}"),
            Self::InvalidArgument(message) => write!(f, "invalid argument: {message}"),
            Self::Storage(message) => write!(f, "storage error: {message}"),
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

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Builds an [`Error::Format`] from a formatted message.
#[macro_export]
macro_rules! format_error {
    ($($arg:tt)*) => { $crate::error::Error::Format(format!($($arg)*)) };
}

/// Builds an [`Error::Unsupported`] from a formatted message.
#[macro_export]
macro_rules! unsupported_error {
    ($($arg:tt)*) => { $crate::error::Error::Unsupported(format!($($arg)*)) };
}

/// Builds an [`Error::Parse`] from a formatted message.
#[macro_export]
macro_rules! parse_error {
    ($($arg:tt)*) => { $crate::error::Error::Parse(format!($($arg)*)) };
}

/// Builds an [`Error::InvalidArgument`] from a formatted message.
#[macro_export]
macro_rules! invalid_argument_error {
    ($($arg:tt)*) => { $crate::error::Error::InvalidArgument(format!($($arg)*)) };
}

/// Builds an [`Error::Storage`] from a formatted message.
#[macro_export]
macro_rules! storage_error {
    ($($arg:tt)*) => { $crate::error::Error::Storage(format!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_covers_every_variant() {
        let cases = [
            Error::Io(io::Error::new(io::ErrorKind::NotFound, "missing")),
            Error::Format("bad riff".into()),
            Error::Unsupported("adpcm".into()),
            Error::Parse("bad lino".into()),
            Error::Mismatch("rate".into()),
            Error::InvalidArgument("window".into()),
            Error::Storage("link 7 does not exist".into()),
        ];

        for case in &cases {
            assert!(!case.to_string().is_empty());
        }
    }

    #[test]
    fn io_errors_expose_their_source() {
        let error = Error::from(io::Error::other("boom"));

        assert!(std::error::Error::source(&error).is_some());
        assert!(std::error::Error::source(&Error::Format("x".into())).is_none());
    }
}
