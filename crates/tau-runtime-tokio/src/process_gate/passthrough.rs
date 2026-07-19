//! Back-compat re-export of the passthrough gate.
//!
//! `PassthroughSandbox` moved to `tau-ports` as [`tau_ports::PassthroughGate`]
//! (ADR-0060) so transport crates can use the default no-op gate without
//! depending on this host runtime. The old name is kept as an alias for
//! existing callers (the resolver's `SandboxAdapter::Passthrough` variant, the
//! registry, and the CLI wiring).

pub use tau_ports::PassthroughGate as PassthroughSandbox;
