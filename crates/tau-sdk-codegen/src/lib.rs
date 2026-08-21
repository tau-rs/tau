//! Codegen for tau authoring SDKs, pinned against the frozen IR JSON schema
//! (which the generator loads and validates).
//!
//! The generated SDKs are *authoring front-ends*: they produce the same
//! `ProjectConfig` the TOML surface parses to, so all three surfaces lower
//! to byte-identical canonical IR via the single Rust lowering pass.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod authoring;
pub mod embed_js;
pub mod embed_rust;
pub mod emit;
pub mod emit_python;
pub mod emit_rust_lib;
pub mod emit_ts;
pub mod error;
pub mod schema;

pub use emit::{generate, generate_into};
pub use embed_rust::{render_embed_rust, EmbedRustInput};
pub use emit_rust_lib::{render_rust_lib, RustLibInput};
pub use error::CodegenError;
