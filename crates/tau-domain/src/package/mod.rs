//! Package metadata types (sources, manifests, capabilities).

pub mod capability;
pub mod manifest;
pub mod plugin;
pub mod sandbox;
pub mod skill;
pub mod skill_format;
pub mod source;

pub use capability::{
    AgentCapability, Capability, CapabilityShape, CapabilityShapeSet, FsCapability, NetCapability,
    NetHosts, ProcessCapability, SkillCapability,
};
pub use manifest::{kinds, PackageDep, PackageId, PackageKind, PackageManifest, UncheckedManifest};
pub use plugin::{PluginKind, PluginManifest, PortKind};
pub use sandbox::{PluginRequiredTier, PluginSandboxRequirements};
#[cfg(feature = "std")]
pub use skill_format::detect_format;
#[cfg(feature = "serde")]
pub use skill_format::synthesize_manifest_from_skill_md;
pub use skill_format::{SkillFormat, SynthesizeError};
pub use source::{GitLocation, PackageSource};
