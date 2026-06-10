//! swc parser setup + module-level AST acquisition.

use std::path::Path;

use swc_common::{sync::Lrc, FileName, SourceMap, Spanned};
use swc_ecma_ast::Module;
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};

use crate::error::TsExtractError;

/// Parse a TS source string into an `swc_ecma_ast::Module`.
///
/// Returns the parsed module AND the `SourceMap` used (caller keeps it
/// alive for span-to-position resolution during error reporting).
pub fn parse_module(
    source: &str,
    source_path: &Path,
) -> Result<(Module, Lrc<SourceMap>), TsExtractError> {
    let cm: Lrc<SourceMap> = Default::default();

    let fm = cm.new_source_file(
        Lrc::new(FileName::Real(source_path.to_path_buf())),
        source.to_string(),
    );

    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax::default()),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);

    let module = parser.parse_module().map_err(|e| {
        let span = e.span();
        let loc = cm.lookup_char_pos(span.lo);
        TsExtractError::ParseError {
            file: source_path.to_path_buf(),
            line: loc.line as u32,
            col: (loc.col.0 + 1) as u32,
            message: format!("{:?}", e.kind()),
        }
    })?;

    Ok((module, cm))
}
