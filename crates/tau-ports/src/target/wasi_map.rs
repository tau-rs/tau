//! Capability → WASI/WIT mapping table for the wasm target (EPIC 3.1).
//!
//! `map_capability` lowers one [`tau_domain::Capability`] to its WASI/WIT
//! realization: the WIT interface [`WitInterface`] imports the generated world
//! must declare (3.2), the `WasiConfig` fragment the host `WasiCtx` consumes
//! (3.3), and the `Disposition` that says how the capability is satisfied on
//! wasm (3.4). Pure, total, and read-only over `tau_domain`.
//!
//! See `docs/superpowers/specs/2026-07-23-epic-3-1-cap-wit-table-design.md`.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use tau_domain::{
    AgentCapability, Capability, FsCapability, HostSet, HttpMethod, NetCapability,
    ProcessCapability, SkillCapability,
};

/// WASI preview-2 version this table pins (wasip2, wasmtime-45, β.7.5).
pub const WASI_VERSION: &str = "0.2.3";

/// The WASI interfaces this table references. [`WitInterface::package_id`]
/// returns the fully-qualified WIT package id, e.g.
/// `"wasi:http/outgoing-handler@0.2.3"`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WitInterface {
    /// `wasi:http/types` — HTTP request/response value types.
    WasiHttpTypes,
    /// `wasi:http/outgoing-handler` — outbound HTTP; carries the host allow-list.
    WasiHttpOutgoingHandler,
    /// `wasi:filesystem/types` — filesystem descriptors and operations.
    WasiFilesystemTypes,
    /// `wasi:filesystem/preopens` — the set of preopened directories.
    WasiFilesystemPreopens,
}

impl WitInterface {
    /// Fully-qualified WIT package id (interface path + `@` + [`WASI_VERSION`]).
    pub fn package_id(&self) -> &'static str {
        match self {
            WitInterface::WasiHttpTypes => "wasi:http/types@0.2.3",
            WitInterface::WasiHttpOutgoingHandler => "wasi:http/outgoing-handler@0.2.3",
            WitInterface::WasiFilesystemTypes => "wasi:filesystem/types@0.2.3",
            WitInterface::WasiFilesystemPreopens => "wasi:filesystem/preopens@0.2.3",
        }
    }
}

/// How a capability is satisfied on the wasm target.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// Bounded by a WASI import + config: network, fs.read, fs.write.
    Wasi,
    /// Enforced in-guest by the tau runtime; no WASI surface
    /// (taskllist, plan, agent.spawn, skill.spawn).
    InGuest,
    /// Requires host mediation outside the WASI ABI; out of scope for wasm
    /// capability gating (hardware / generic `Custom`).
    HostMediated,
    /// Cannot be expressed on the wasm target (fs.exec, process.spawn).
    Unsupported {
        /// Human-readable reason, surfaced by 3.2/3.4 diagnostics.
        reason: &'static str,
    },
}

/// Preopen access mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreopenAccess {
    /// fs.read → read-only preopen.
    ReadOnly,
    /// fs.write → read-write preopen.
    ReadWrite,
}

/// A single preopen derived from an fs capability. Glob → directory
/// resolution is deferred to story 3.3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preopen {
    /// Glob patterns copied verbatim from the fs capability.
    pub paths: Vec<String>,
    /// Read-only (fs.read) or read-write (fs.write).
    pub access: PreopenAccess,
}

/// Runtime configuration a capability contributes to the host `WasiCtx` (3.3).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WasiConfig {
    /// No runtime config (all non-`Wasi` dispositions).
    None,
    /// Network egress filter. `hosts` reuses D4-B [`HostSet`] semantics
    /// (exact | typed `Any`); `methods == None` means all methods.
    AllowedHosts {
        /// Allowed hostnames, copied verbatim from the capability.
        hosts: HostSet,
        /// Allowed HTTP methods; `None` = all.
        methods: Option<BTreeSet<HttpMethod>>,
    },
    /// Filesystem preopens derived from the capability's glob paths.
    Preopens(Vec<Preopen>),
}

/// The WASI/WIT realization of a single capability on the wasm target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasiMapping {
    /// WIT interface imports the generated world must declare (3.2). Empty
    /// unless `disposition == Disposition::Wasi`.
    pub imports: Vec<WitInterface>,
    /// Runtime config fragment this capability contributes to `WasiCtx` (3.3).
    pub config: WasiConfig,
    /// How this capability is satisfied on the wasm target.
    pub disposition: Disposition,
}

/// Lower one tau [`Capability`] to its WASI/WIT realization on the wasm target.
///
/// Total and pure: every capability yields a [`WasiMapping`]. Capabilities
/// that bind to a WASI import return `Disposition::Wasi` with non-empty
/// `imports`; all others carry empty `imports` and `WasiConfig::None`.
///
/// # Example
///
/// ```
/// use tau_ports::target::wasi_map::{map_capability, Disposition};
/// use tau_domain::fixtures::cap_fs_read;
///
/// let cap = cap_fs_read(&["/d"]);
/// assert!(matches!(map_capability(&cap).disposition, Disposition::Wasi));
/// ```
pub fn map_capability(cap: &Capability) -> WasiMapping {
    match cap {
        Capability::Network(NetCapability::Http { hosts, methods, .. }) => WasiMapping {
            imports: vec![
                WitInterface::WasiHttpTypes,
                WitInterface::WasiHttpOutgoingHandler,
            ],
            config: WasiConfig::AllowedHosts {
                hosts: hosts.clone(),
                methods: methods.clone(),
            },
            disposition: Disposition::Wasi,
        },
        Capability::Filesystem(FsCapability::Read { paths, .. }) => {
            fs_preopen(paths.clone(), PreopenAccess::ReadOnly)
        }
        Capability::Filesystem(FsCapability::Write { paths, .. }) => {
            fs_preopen(paths.clone(), PreopenAccess::ReadWrite)
        }
        Capability::Filesystem(FsCapability::Exec { .. }) => WasiMapping {
            imports: Vec::new(),
            config: WasiConfig::None,
            disposition: Disposition::Unsupported {
                reason: "wasm target has no exec surface",
            },
        },
        Capability::Process(ProcessCapability::Spawn { .. }) => WasiMapping {
            imports: Vec::new(),
            config: WasiConfig::None,
            disposition: Disposition::Unsupported {
                reason: "wasm target cannot spawn OS processes",
            },
        },
        Capability::Agent(AgentCapability::Spawn { .. })
        | Capability::Skill(SkillCapability::Spawn { .. })
        | Capability::TaskList { .. }
        | Capability::Plan { .. } => WasiMapping {
            imports: Vec::new(),
            config: WasiConfig::None,
            disposition: Disposition::InGuest,
        },
        Capability::Custom { .. } => WasiMapping {
            imports: Vec::new(),
            config: WasiConfig::None,
            disposition: Disposition::HostMediated,
        },
        // Fail-closed: an unknown future capability (or future FsCapability /
        // NetCapability / … variant, all `#[non_exhaustive]`) is NOT granted a
        // WASI import. It maps to HostMediated so it can never silently reach
        // the guest's WASI ABI.
        _ => WasiMapping {
            imports: Vec::new(),
            config: WasiConfig::None,
            disposition: Disposition::HostMediated,
        },
    }
}

/// Build a filesystem `Wasi` mapping (shared by fs.read / fs.write).
fn fs_preopen(paths: Vec<String>, access: PreopenAccess) -> WasiMapping {
    WasiMapping {
        imports: vec![
            WitInterface::WasiFilesystemTypes,
            WitInterface::WasiFilesystemPreopens,
        ],
        config: WasiConfig::Preopens(vec![Preopen { paths, access }]),
        disposition: Disposition::Wasi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `Capability`'s `Filesystem`/`Network` variants (and their inner
    // `FsCapability`/`NetCapability` payload variants) are `#[non_exhaustive]`
    // at the variant level, so struct-literal construction from outside
    // `tau-domain` is blocked. `tau_domain::fixtures::cap_{fs_read,fs_write,
    // net_http}` (feature = "test-fixtures", already a tau-ports dev-dep) are
    // the crate's documented external-test construction path — see
    // crates/tau-domain/src/fixtures.rs and
    // crates/tau-runtime-tokio/tests/run_capability_denied.rs for precedent.
    use alloc::collections::BTreeSet;
    use alloc::string::ToString;
    use alloc::vec;
    use tau_domain::fixtures::{cap_fs_read, cap_fs_write, cap_net_http};
    use tau_domain::{HostName, HostSet, HttpMethod};

    fn exact(hosts: &[&str]) -> HostSet {
        HostSet::Exact(hosts.iter().map(|h| HostName::parse(h).unwrap()).collect())
    }

    #[test]
    fn package_id_is_fully_qualified_and_version_pinned() {
        let all = [
            WitInterface::WasiHttpTypes,
            WitInterface::WasiHttpOutgoingHandler,
            WitInterface::WasiFilesystemTypes,
            WitInterface::WasiFilesystemPreopens,
        ];
        for iface in all {
            let id = iface.package_id();
            assert!(id.starts_with("wasi:"), "not fully qualified: {id}");
            assert!(
                id.ends_with(&alloc::format!("@{WASI_VERSION}")),
                "version drift: {id} != @{WASI_VERSION}"
            );
        }
        assert_eq!(
            WitInterface::WasiHttpOutgoingHandler.package_id(),
            "wasi:http/outgoing-handler@0.2.3"
        );
    }

    #[test]
    fn net_http_maps_to_wasi_http_with_hosts_and_methods_verbatim() {
        let mut methods = BTreeSet::new();
        methods.insert(HttpMethod::Post);
        let cap = cap_net_http(&["api.anthropic.com"], &["POST"]);

        let m = map_capability(&cap);

        assert!(matches!(m.disposition, Disposition::Wasi));
        assert_eq!(
            m.imports,
            vec![
                WitInterface::WasiHttpTypes,
                WitInterface::WasiHttpOutgoingHandler
            ]
        );
        match m.config {
            WasiConfig::AllowedHosts {
                hosts,
                methods: got,
            } => {
                assert_eq!(hosts, exact(&["api.anthropic.com"]));
                assert_eq!(got, Some(methods));
            }
            other => panic!("expected AllowedHosts, got {other:?}"),
        }
    }

    #[test]
    fn net_http_any_and_all_methods_pass_through_unchanged() {
        let cap = cap_net_http(&["any"], &[]);
        match map_capability(&cap).config {
            WasiConfig::AllowedHosts { hosts, methods } => {
                assert!(hosts.is_any());
                assert_eq!(methods, None);
            }
            other => panic!("expected AllowedHosts, got {other:?}"),
        }
    }

    #[test]
    fn fs_read_maps_to_readonly_preopen_with_paths_verbatim() {
        let cap = cap_fs_read(&["/data/**"]);
        let m = map_capability(&cap);
        assert!(matches!(m.disposition, Disposition::Wasi));
        assert_eq!(
            m.imports,
            vec![
                WitInterface::WasiFilesystemTypes,
                WitInterface::WasiFilesystemPreopens
            ]
        );
        match m.config {
            WasiConfig::Preopens(p) => {
                assert_eq!(p.len(), 1);
                assert_eq!(p[0].paths, vec!["/data/**".to_string()]);
                assert!(matches!(p[0].access, PreopenAccess::ReadOnly));
            }
            other => panic!("expected Preopens, got {other:?}"),
        }
    }

    #[test]
    fn fs_write_maps_to_readwrite_preopen() {
        let cap = cap_fs_write(&["/out"], None);
        match map_capability(&cap).config {
            WasiConfig::Preopens(p) => {
                assert_eq!(p[0].paths, vec!["/out".to_string()]);
                assert!(matches!(p[0].access, PreopenAccess::ReadWrite));
            }
            other => panic!("expected Preopens, got {other:?}"),
        }
    }

    use tau_domain::fixtures::{cap_agent_spawn, cap_custom, cap_fs_exec, cap_process_spawn};

    #[test]
    fn fs_exec_is_unsupported() {
        let cap = cap_fs_exec(&["/bin/x"]);
        let m = map_capability(&cap);
        assert!(m.imports.is_empty());
        assert!(matches!(m.config, WasiConfig::None));
        match m.disposition {
            Disposition::Unsupported { reason } => assert!(!reason.is_empty()),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn process_spawn_is_unsupported() {
        let cap = cap_process_spawn(&["ls"]);
        assert!(matches!(
            map_capability(&cap).disposition,
            Disposition::Unsupported { .. }
        ));
    }

    // NOTE: SkillCapability::Spawn shares the exact same match arm as
    // agent.spawn (see the OR'd `Capability::Agent(AgentCapability::Spawn { .. })
    // | Capability::Skill(SkillCapability::Spawn { .. }) | ...` arm in
    // `map_capability`), but it is not externally constructible for a
    // standalone test: the variant is `#[non_exhaustive]` at the variant
    // level and `tau_domain::fixtures` has no `cap_skill_spawn` constructor.
    // It is covered structurally by the shared arm below, not by an
    // independent assertion.
    #[test]
    fn agent_spawn_is_in_guest() {
        let agent = cap_agent_spawn(&["worker"]);
        let m = map_capability(&agent);
        assert!(m.imports.is_empty());
        assert!(matches!(m.config, WasiConfig::None));
        assert!(matches!(m.disposition, Disposition::InGuest));
    }

    #[test]
    fn tasklist_and_plan_are_in_guest() {
        let tasks = Capability::TaskList {
            mode: "read".into(),
        };
        let plan = Capability::Plan {
            mode: "write".into(),
        };
        for cap in [tasks, plan] {
            assert!(matches!(
                map_capability(&cap).disposition,
                Disposition::InGuest
            ));
        }
    }

    #[test]
    fn custom_is_host_mediated() {
        let cap = cap_custom("hw.fan");
        let m = map_capability(&cap);
        assert!(m.imports.is_empty());
        assert!(matches!(m.disposition, Disposition::HostMediated));
    }
}
