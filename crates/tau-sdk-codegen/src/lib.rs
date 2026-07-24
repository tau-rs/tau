//! Codegen for tau authoring SDKs from the frozen IR JSON schema.
//!
//! The generated SDKs are *authoring front-ends*: they produce the same
//! `ProjectConfig` the TOML surface parses to, so all three surfaces lower
//! to byte-identical canonical IR via the single Rust lowering pass.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod error;

pub use error::CodegenError;
