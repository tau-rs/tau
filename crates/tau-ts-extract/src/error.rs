//! All error types for the TS extractor.

use std::path::PathBuf;
use thiserror::Error;

/// Source-file position (1-indexed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    /// Source file path.
    pub file: PathBuf,
    /// Line number (1-indexed).
    pub line: u32,
    /// Column number (1-indexed).
    pub col: u32,
}

impl std::fmt::Display for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file.display(), self.line, self.col)
    }
}

/// All errors that can arise during TS extraction.
#[derive(Debug, Error)]
pub enum TsExtractError {
    /// The source file is not valid UTF-8.
    #[error("{file}: not UTF-8")]
    NotUtf8 {
        /// Source file path.
        file: PathBuf,
    },

    /// swc parse error.
    #[error("{pos}: parse error: {message}")]
    ParseError {
        /// Source position.
        pos: Position,
        /// Error message.
        message: String,
    },

    /// Called a function that isn't a tau factory.
    #[error("{pos}: unknown factory `{name}` (expected agent/tool/mcp/contextManager)")]
    UnknownFactory {
        /// Source position of the call.
        pos: Position,
        /// Name that was called.
        name: String,
    },

    /// An expression that's not in the allowed literal whitelist.
    #[error("{pos}: unsupported expression `{kind}`: {hint}")]
    UnsupportedExpression {
        /// Source position.
        pos: Position,
        /// AST node kind.
        kind: String,
        /// Remediation hint.
        hint: String,
    },

    /// Identifier reference doesn't resolve to a top-level constant.
    #[error("{pos}: unresolved identifier `{name}` (not declared as top-level const)")]
    UnresolvedIdentifier {
        /// Source position.
        pos: Position,
        /// Name that was referenced.
        name: String,
    },

    /// Factory whose implementation requires a future sub-project.
    #[error("{pos}: `{factory}` is deferred to {until}")]
    Deferred {
        /// Source position.
        pos: Position,
        /// Factory name.
        factory: String,
        /// Future milestone (e.g. "β.4").
        until: String,
    },

    /// `import` from anywhere other than the `tau` module.
    #[error("{pos}: imports from `{import_source}` are not supported in β.8 v1 (multi-file deferred to v1.1)")]
    ImportNotSupported {
        /// Source position.
        pos: Position,
        /// Import source path.
        import_source: String,
    },

    /// `A → B → A`-style identifier reference cycle.
    #[error("{pos}: cyclic reference: {cycle}")]
    CyclicReference {
        /// Source position.
        pos: Position,
        /// Cycle description.
        cycle: String,
    },

    /// Inline tool body — `tool({ run: async () => ... })`.
    #[error("{pos}: inline tool bodies require δ.2 (use `native: \"FnName\"` reference to a Rust-compiled-in tool)")]
    InlineToolBody {
        /// Source position.
        pos: Position,
    },

    /// Wrapped `std::io::Error` for file reads.
    #[error("{file}: I/O error: {source}")]
    Io {
        /// Source file path.
        file: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

impl TsExtractError {
    /// Build a Position from an swc Span + a SourceMap.
    pub fn position_from_span(
        sm: &swc_common::SourceMap,
        span: swc_common::Span,
        file: std::path::PathBuf,
    ) -> Position {
        let loc = sm.lookup_char_pos(span.lo);
        Position {
            file,
            line: loc.line as u32,
            col: (loc.col.0 + 1) as u32,
        }
    }
}
