//! Error type for the TS extractor. Phase 4 fleshes out all 10 variants.

use std::path::PathBuf;
use thiserror::Error;

/// All errors that can arise during TS extraction.
#[derive(Debug, Error)]
pub enum TsExtractError {
    /// swc parse error — invalid TS syntax.
    #[error("{file}:{line}:{col}: parse error: {message}")]
    ParseError {
        /// Source file path.
        file: PathBuf,
        /// Line (1-indexed).
        line: u32,
        /// Column (1-indexed).
        col: u32,
        /// Error message from swc.
        message: String,
    },
}
