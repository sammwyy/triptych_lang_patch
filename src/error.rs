// Shared error type for the patcher.

use std::fmt;

#[derive(Debug)]
pub enum PatchError {
    // Low-level binary format problem (bad signature, CRC, layout, ...).
    Format(String),
    // The patch JSON does not match the script.arc it is applied to.
    Mismatch(String),
    // A string cannot be encoded as CP932.
    Encoding(String),
    // Filesystem / IO problem.
    Io(String),
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatchError::Format(m) => write!(f, "format error: {m}"),
            PatchError::Mismatch(m) => write!(f, "patch mismatch: {m}"),
            PatchError::Encoding(m) => write!(f, "encoding error: {m}"),
            PatchError::Io(m) => write!(f, "io error: {m}"),
        }
    }
}

impl std::error::Error for PatchError {}

impl From<std::io::Error> for PatchError {
    fn from(e: std::io::Error) -> Self {
        PatchError::Io(e.to_string())
    }
}
