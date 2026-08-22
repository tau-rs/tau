//! Directory-based tool/agent definitions (`[dirs]`, ADR-0067).
//!
//! `file` parses individual definition files (`agents/**/*.{md,toml}`,
//! `tools/**/*.toml`) into the same unchecked shapes the inline
//! `[agents.*]` / `[tools.*]` tables produce. The recursive filesystem
//! scanner that walks a `[dirs]` root and calls into `file` lands in a
//! later task.

pub(crate) mod file;
