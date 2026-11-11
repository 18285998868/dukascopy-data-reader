//! Public error type for configuration, filesystem and archive parsing
//! failures.

use thiserror::Error;

/// Errors produced while loading Dukascopy archive data.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReaderError {
    /// Filesystem access failed while scanning or reading an archive.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// User configuration is invalid.
    #[error("config: {0}")]
    Config(String),

    /// Archive contents are malformed or a batch-level load failed.
    #[error("parse: {0}")]
    Parse(String),
}

/// Convenience alias for `std::result::Result<T, ReaderError>`.
pub type Result<T> = std::result::Result<T, ReaderError>;
