//! Variant-B embedding host for tau (EPIC 7.1). A product links the
//! generated `embed_native_workflow_lib` (no_std) and implements the
//! runtime ports itself; see `README.md`.
pub mod dispatcher;
pub mod llm;
pub mod ports;

pub use dispatcher::HostDispatcher;
