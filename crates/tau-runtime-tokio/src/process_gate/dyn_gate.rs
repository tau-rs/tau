//! Back-compat re-export of the process-gate trait object.
//!
//! The object-safe process gate moved to `tau-ports` as
//! [`tau_ports::DynProcessGate`] (ADR-0062) so transport crates (e.g.
//! `tau-mcp-tokio`) can depend on the port without pulling in this whole host
//! runtime. `DynProcessCapabilityGate` remains as an alias for the callers that
//! still reference the tokio-shell path.

pub use tau_ports::DynProcessGate as DynProcessCapabilityGate;
