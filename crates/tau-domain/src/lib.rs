#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Core domain types for tau. Pure data — no I/O, no plugin contracts.
//! See the constitution (G5) for why messages are the universal interaction primitive.
//!
//! `#![no_std]` + `alloc`: this is the runtime vocabulary the `no_std` wasm
//! guest links through `tau-runtime-core`. The std-only corners (git
//! `PackageSource` URL parsing, SKILL.md YAML) live behind the host-only
//! `package-source` / `skill-md` features.

#[macro_use]
extern crate alloc;

#[cfg(any(test, feature = "std"))]
extern crate std;

pub mod agent;
pub mod error;
pub mod id;
pub mod message;
pub mod package;
pub mod value;
pub mod version;

#[cfg(any(test, feature = "test-fixtures"))]
pub mod fixtures;

#[cfg(feature = "skill-md")]
pub use crate::package::skill::parse_skill_md;
pub use crate::package::skill::{
    SkillContent, SkillContentError, SkillFrontmatter, SkillManifest, SKILL_DIR_VAR,
};
pub use agent::{AgentDefinition, AgentStatus, FailureKind};
pub use error::{
    AgentIdError, PackageKindError, PackageManifestError, PackageNameError, PackageSourceError,
    PluginKindError, PortKindError,
};
pub use id::{AgentId, AgentInstanceId, MessageId, PackageName};
pub use message::{Address, Message, MessagePayload};
#[cfg(feature = "std")]
pub use package::detect_format;
#[cfg(feature = "serde")]
pub use package::synthesize_manifest_from_skill_md;
pub use package::{
    kinds, AgentCapability, Capability, CapabilityShape, CapabilityShapeSet, FsCapability,
    GitLocation, HostName, HostNameError, HostSet, HttpMethod, NetCapability, PackageDep,
    PackageId, PackageKind, PackageManifest, PackageSource, PluginKind, PluginManifest,
    PluginRequiredTier, PluginSandboxRequirements, PortKind, ProcessCapability, SkillCapability,
    SkillFormat, SynthesizeError, UncheckedManifest,
};
pub use value::Value;
pub use version::{Version, VersionReq};

// External-crate re-exports for convenience: anything that takes a
// `tau_domain::Url` should accept a `url::Url` from anywhere in the tree.
#[cfg(feature = "package-source")]
pub use url::Url;
pub use uuid::Uuid;
