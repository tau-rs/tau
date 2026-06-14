//! Concrete credential providers for the tokio host (β.5).
//!
//! The port + chain combinator live in `tau-ports`; these are the
//! std I/O adapters that need the filesystem and process environment.

mod env;
mod file;

pub use env::EnvProvider;
pub use file::FileProvider;
