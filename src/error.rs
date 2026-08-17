use std::fmt;

use thiserror::Error;

/// The message base format a low level parse error originated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jam,
    PCBoard,
    Qwk,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Format::Jam => write!(f, "JAM"),
            Format::PCBoard => write!(f, "PCBoard"),
            Format::Qwk => write!(f, "QWK"),
        }
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Jam(#[from] crate::jam::JamError),

    #[error(transparent)]
    PCBoard(#[from] crate::pcboard::PCBoardError),

    #[error(transparent)]
    Qwk(#[from] crate::qwk::QwkError),

    /// A record did not match the format specification.
    #[error("invalid {format} data at offset {offset}: {reason}")]
    InvalidData {
        format: Format,
        offset: u64,
        reason: String,
    },

    #[error("invalid utf-8: {0}")]
    Utf8(#[from] bstr::Utf8Error),

    #[error("system time is before the unix epoch: {0}")]
    SystemTime(#[from] std::time::SystemTimeError),
}

impl Error {
    pub(crate) fn invalid(format: Format, offset: u64, reason: impl Into<String>) -> Self {
        Self::InvalidData {
            format,
            offset,
            reason: reason.into(),
        }
    }

    pub(crate) fn jam(offset: u64, reason: impl Into<String>) -> Self {
        Self::invalid(Format::Jam, offset, reason)
    }

    pub(crate) fn pcboard(offset: u64, reason: impl Into<String>) -> Self {
        Self::invalid(Format::PCBoard, offset, reason)
    }

    pub(crate) fn qwk(offset: u64, reason: impl Into<String>) -> Self {
        Self::invalid(Format::Qwk, offset, reason)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
