#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Observability primitives for tau: structured logging, tracing, and
//! the "observe" verb of the four-verb core (G1).

#[cfg(any(feature = "test-fixtures", test))]
pub mod capture;
pub mod filter;
pub mod install;
pub mod vocabulary;
