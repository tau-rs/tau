//! Runtime sandbox glue — declarative-requirements resolver + plan validation.

pub mod passthrough;
mod plan;
pub mod registry;
pub mod resolution_error;
pub mod resolver;
mod validation;

pub use plan::build_plan;
pub use resolution_error::{ResolutionError, ResolutionRejection};
pub use resolver::{
    instantiate_for_probe, resolve_adapter, resolve_adapter_forced, resolve_strict_for_validation,
    SandboxAdapter,
};
pub use validation::{validate_plan_against_adapter, SandboxValidationError};

pub mod target_match;

pub use target_match::{adapter_satisfies, kind_to_family, registration_for_triple};
