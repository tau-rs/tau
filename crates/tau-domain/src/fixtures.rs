//! Test fixtures for `tau-domain` types.
//!
//! Gated behind the `test-fixtures` feature (off by default). Downstream
//! crates depend via:
//!
//! ```toml
//! [dev-dependencies]
//! tau-domain = { workspace = true, features = ["test-fixtures"] }
//! ```
//!
//! All helpers are deterministic where possible; UUID-based ones
//! (`any_message`, `any_agent_definition`-derived IDs) generate fresh
//! v7 UUIDs each call.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use core::str::FromStr;

use crate::agent::AgentDefinition;
use crate::id::{AgentId, PackageName};
#[cfg(feature = "std")]
use crate::id::{AgentInstanceId, MessageId};
#[cfg(feature = "std")]
use crate::message::{Address, Message, MessagePayload};
use crate::package::capability::{
    AgentCapability, Capability, FsCapability, NetCapability, ProcessCapability,
};
use crate::package::manifest::{PackageId, PackageKind, UncheckedManifest};
use crate::package::{PackageManifest, PackageSource};
use crate::value::Value;
use crate::version::Version;

/// A deterministic, valid `PackageName`.
pub fn any_package_name() -> PackageName {
    PackageName::from_str("test-pkg").expect("valid")
}

/// A deterministic, valid `AgentId`.
pub fn any_agent_id() -> AgentId {
    AgentId::from_str("test-agent").expect("valid")
}

/// A deterministic, valid `PackageSource` (https URL, no rev).
pub fn any_package_source() -> PackageSource {
    PackageSource::from_str("https://example.com/test.git").expect("valid")
}

/// A minimal valid `UncheckedManifest`.
pub fn any_unchecked_manifest() -> UncheckedManifest {
    UncheckedManifest {
        name: any_package_name(),
        version: Version::parse("0.1.0").expect("valid"),
        description: "test package".into(),
        authors: vec![],
        license: None,
        source: any_package_source(),
        kind: PackageKind::Custom {
            kind: "tool".into(),
        },
        dependencies: vec![],
        capabilities: vec![],
        plugin: None,
        sandbox: crate::package::sandbox::PluginSandboxRequirements::default(),
        skill: None,
    }
}

/// A minimal validated `PackageManifest`.
pub fn any_package_manifest() -> PackageManifest {
    any_unchecked_manifest()
        .validate()
        .expect("fixture should validate")
}

/// A minimal `AgentDefinition`.
pub fn any_agent_definition() -> AgentDefinition {
    AgentDefinition::new(
        any_agent_id(),
        "Test Agent".into(),
        PackageId {
            name: any_package_name(),
            version: Version::parse("0.1.0").expect("valid"),
        },
        any_package_name(),
    )
}

/// A minimal `Message` with a Text payload (fresh UUIDs).
///
/// Host-only (`std`): mints ids via [`MessageId::new`]/[`AgentInstanceId::new`],
/// which read the ambient clock + RNG.
#[cfg(feature = "std")]
pub fn any_message() -> Message {
    Message {
        id: MessageId::new(),
        sender: Address::User,
        recipient: Address::Agent(AgentInstanceId::new()),
        parent_id: None,
        created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0)
            .expect("0 is a valid timestamp"),
        headers: BTreeMap::new(),
        payload: MessagePayload::Text {
            content: "hello".into(),
        },
    }
}

// ---------------------------------------------------------------------------
// Capability constructors
// ---------------------------------------------------------------------------
//
// These helpers bypass the `#[non_exhaustive]` barrier that prevents external
// crates from constructing variant structs with literal syntax. External test
// crates (tau-sandbox-native, tau-runtime, tau-plugin-compat, ...) depend on
// `tau-domain` with `features = ["test-fixtures"]` and call these functions
// instead of the `serde_json::from_value(json!({...}))` round-trip workaround.

/// Build a `Capability::Filesystem(FsCapability::Read)` granting read access
/// to the given path glob patterns.
pub fn cap_fs_read(paths: &[&str]) -> Capability {
    Capability::Filesystem(FsCapability::Read {
        paths: paths.iter().map(|s| s.to_string()).collect(),
    })
}

/// Build a `Capability::Filesystem(FsCapability::Write)` granting write access
/// to the given path glob patterns, with an optional byte limit.
pub fn cap_fs_write(paths: &[&str], max_bytes: Option<u64>) -> Capability {
    Capability::Filesystem(FsCapability::Write {
        paths: paths.iter().map(|s| s.to_string()).collect(),
        max_bytes,
    })
}

/// Build a `Capability::Filesystem(FsCapability::Exec)` granting exec access
/// to binaries matching the given path glob patterns.
pub fn cap_fs_exec(paths: &[&str]) -> Capability {
    Capability::Filesystem(FsCapability::Exec {
        paths: paths.iter().map(|s| s.to_string()).collect(),
    })
}

/// Build a `Capability::Network(NetCapability::Http)` granting HTTP access to
/// the given hosts with the given HTTP methods. An empty `hosts` slice yields
/// the any-host escape hatch ([`NetHosts::Any`]); a non-empty slice yields a
/// [`NetHosts::List`].
pub fn cap_net_http(hosts: &[&str], methods: &[&str]) -> Capability {
    let hosts = if hosts.is_empty() {
        crate::NetHosts::Any
    } else {
        crate::NetHosts::List(hosts.iter().map(|s| s.to_string()).collect())
    };
    Capability::Network(NetCapability::Http {
        hosts,
        methods: methods.iter().map(|s| s.to_string()).collect(),
    })
}

/// Build a `Capability::Process(ProcessCapability::Spawn)` granting permission
/// to spawn subprocesses matching the given command names.
pub fn cap_process_spawn(commands: &[&str]) -> Capability {
    Capability::Process(ProcessCapability::Spawn {
        commands: commands.iter().map(|s| s.to_string()).collect(),
    })
}

/// Build a `Capability::Agent(AgentCapability::Spawn)` granting permission to
/// spawn sub-agents whose package kind is in the allow-list.
pub fn cap_agent_spawn(allowed_kinds: &[&str]) -> Capability {
    Capability::Agent(AgentCapability::Spawn {
        allowed_kinds: allowed_kinds.iter().map(|s| s.to_string()).collect(),
    })
}

/// Build a `Capability::Custom` with the given name and no parameters.
pub fn cap_custom(name: &str) -> Capability {
    Capability::Custom {
        name: name.into(),
        params: BTreeMap::new(),
    }
}

/// Build a `Capability::Custom` with the given name and a parameter map.
pub fn cap_custom_with_params(name: &str, params: BTreeMap<String, Value>) -> Capability {
    Capability::Custom {
        name: name.into(),
        params,
    }
}
