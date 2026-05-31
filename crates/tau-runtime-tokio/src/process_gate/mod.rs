//! Runtime process-gate glue — `DynProcessCapabilityGate` trait-object
//! wrapper + declarative-requirements resolver + plan validation.
//!
//! Renamed from `crates/tau-runtime-tokio/src/sandbox/` at Phase β.1.4
//! per spec §3 (the `CapabilityGate` vocabulary shift). Concrete adapter
//! type names (`SandboxAdapter`, `SandboxValidationError`, etc.) keep
//! their current names — the public crate names `tau-sandbox-*` and the
//! concrete impls `NativeSandbox`/`ContainerSandbox`/etc. retain
//! "sandbox" as-is (spec §3 "What does NOT rename"); the surrounding
//! tokio-shell glue carrying those types follows the same discipline.

pub mod dyn_gate;
pub mod passthrough;
mod plan;
pub mod registry;
pub mod resolution_error;
pub mod resolver;
mod validation;

pub use dyn_gate::DynProcessCapabilityGate;
pub use plan::build_plan;
pub use resolution_error::{ResolutionError, ResolutionRejection};
pub use resolver::{
    instantiate_for_probe, resolve_adapter, resolve_adapter_forced, resolve_strict_for_validation,
    SandboxAdapter,
};
pub use validation::{validate_plan_against_adapter, SandboxValidationError};

pub mod target_match;

pub use target_match::{adapter_satisfies, kind_to_family, registration_for_triple};
