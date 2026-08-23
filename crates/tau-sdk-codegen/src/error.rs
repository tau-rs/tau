//! Error type at the crate boundary.

use thiserror::Error;

/// Errors surfaced by SDK codegen.
#[derive(Debug, Error)]
pub enum CodegenError {
    /// Reading the schema or writing an output file failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The IR schema JSON was malformed or missing an expected structure.
    #[error("schema error: {0}")]
    Schema(String),
}
