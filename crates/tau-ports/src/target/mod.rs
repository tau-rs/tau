//! Tau deployment target identifier (target triple) module.
//!
//! See spec `docs/superpowers/specs/2026-05-19-target-triple-registry-design.md`.

pub mod adapter_family;
pub mod parse;
pub mod platform;
pub mod profile;
pub mod registry;
pub mod triple;
pub mod wasi_map;
pub mod wit_world;

pub use adapter_family::AdapterFamily;
pub use parse::ParseError;
pub use platform::Platform;
pub use profile::{TargetCapabilityProfile, TripleStatus};
pub use registry::{list_all, list_available, lookup, TargetTripleEntry, REGISTRY};
pub use triple::TargetTriple;
pub use wasi_map::{
    map_capability, Disposition, Preopen, PreopenAccess, WasiConfig, WasiMapping, WitInterface,
    WASI_VERSION,
};
pub use wit_world::{generate_world, WitWorldError};
