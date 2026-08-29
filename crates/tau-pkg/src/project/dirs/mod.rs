//! Directory-based tool/agent definitions (`[dirs]`, ADR-0069).
//!
//! `file` parses individual definition files (`agents/**/*.{md,toml}`,
//! `tools/**/*.toml`) into the same unchecked shapes the inline
//! `[agents.*]` / `[tools.*]` tables produce. `scan` walks a `[dirs]` root
//! recursively, enforces strict hygiene (symlink rejection, charset,
//! extension, root containment/overlap), and derives each definition's
//! engine name from its path.

pub(crate) mod file;
mod scan;

pub use scan::{definition_files, scan_dirs, ScannedDefs};
