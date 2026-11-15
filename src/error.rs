//! Error types for Synapse.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SynapseError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Home directory could not be determined")]
    HomeDirNotFound,
    #[error("Unsupported architecture: {0}")]
    UnsupportedArchitecture(String),
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Version '{0}' not found for runtime '{1}'")]
    VersionNotFound(String, String),
    #[error("Invalid version identifier: '{0}'")]
    InvalidVersion(String),
    #[error("Runtime '{0}' version '{1}' is not installed")]
    NotInstalled(String, String),
    #[error("Unsupported runtime: {0}")]
    UnsupportedRuntime(String),
    #[error("Checksum mismatch for {file}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("Checksum entry not found for file '{0}' in {1}")]
    ChecksumNotFound(String, String),
    #[error("Extraction error: {0}")]
    Extraction(String),
    #[error("No version pin file found for {0} in current or parent directories")]
    PinNotFound(String),
}

pub type Result<T> = std::result::Result<T, SynapseError>;
