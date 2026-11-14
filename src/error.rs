//! Public error type for configuration, filesystem and archive failures.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Errors produced while loading Dukascopy archive data.
///
/// Filesystem variants carry the path that failed so callers can report or
/// retry the exact file instead of guessing from a bare OS error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReaderError {
    /// User configuration is invalid.
    #[error("config: {0}")]
    Config(String),

    /// A filesystem operation failed.
    #[error("io error at {path}: {source}")]
    Io {
        /// Path that the operation was working on.
        path: PathBuf,
        /// Underlying operating-system error.
        #[source]
        source: std::io::Error,
    },

    /// A `.bi5` file could not be decompressed or decoded.
    #[error("corrupt dukascopy file {path}: {detail}")]
    CorruptFile {
        /// Path of the offending archive file.
        path: PathBuf,
        /// What is wrong with the file.
        detail: String,
    },

    /// An internal invariant was violated (for example unequal column
    /// lengths assembled by the caller).
    #[error("integrity check failed: {0}")]
    Integrity(String),
}

impl ReaderError {
    /// Wrap an OS error together with the path it applies to.
    pub(crate) fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    /// Mark a `.bi5` file as corrupt.
    pub(crate) fn corrupt(path: impl AsRef<Path>, detail: impl Into<String>) -> Self {
        Self::CorruptFile {
            path: path.as_ref().to_path_buf(),
            detail: detail.into(),
        }
    }
}

/// Convenience alias for `std::result::Result<T, ReaderError>`.
pub type Result<T> = std::result::Result<T, ReaderError>;
