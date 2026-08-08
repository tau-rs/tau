//! Capability → WASI/WIT mapping table for the wasm target (EPIC 3.1).
//!
//! `map_capability` lowers one [`tau_domain::Capability`] to its WASI/WIT
//! realization: the WIT interface [`WitInterface`] imports the generated world
//! must declare (3.2), the `WasiConfig` fragment the host `WasiCtx` consumes
//! (3.3), and the `Disposition` that says how the capability is satisfied on
//! wasm (3.4). Pure, total, and read-only over `tau_domain`.
//!
//! See `docs/superpowers/specs/2026-07-23-epic-3-1-cap-wit-table-design.md`.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use tau_domain::package::capability::lattice::glob::{expand, Pattern, Segment};
use tau_domain::package::capability::lattice::host::host_union;
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

/// Whether a resolved preopen equals its capability, or is broader because
/// WASI preopens are directory-granular. See the design doc's
/// "fs-granularity divergence".
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreopenGranularity {
    /// Preopen == cap: an all-literal path (preopened as-is), or a literal
    /// prefix + trailing `**` (the preopen equals the cap subtree).
    Exact,
    /// Preopen is broader than the cap: a whole-segment `*` glob whose
    /// preopened literal-prefix directory admits more than the `*`-matched
    /// set. The build gate (3.2/3.4) rejects this by default. A literal
    /// single file is NOT widened — it is preopened as-is and fails at
    /// apply-time if it is really a file.
    WidenedToDir,
}

/// One preopen after glob→directory resolution: a host directory to hand the
/// guest, its access mode, whether resolution widened the cap, and the
/// originating cap glob(s) for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPreopen {
    /// The absolute host directory the embedder will preopen.
    pub host_dir: String,
    /// Read-only (fs.read) or read-write (fs.write).
    pub access: PreopenAccess,
    /// Whether this preopen exactly matches the cap or widened it.
    pub granularity: PreopenGranularity,
    /// Original capability glob(s) that produced this preopen.
    pub from: Vec<String>,
}

/// The `(host_dir, granularity)` a single normalized G2 pattern resolves to.
///
/// `host_dir` is the leading `Literal` prefix joined under `/` (empty prefix
/// → `"/"`). Granularity is `WidenedToDir` iff the pattern contains any
/// whole-segment `Star`; `StarStar` and all-literal are `Exact`.
fn resolve_pattern(pat: &Pattern) -> (String, PreopenGranularity) {
    let mut dir = String::new();
    let mut widened = false;
    let mut stopped = false; // true once we pass the first wildcard segment
    for seg in &pat.0 {
        match seg {
            Segment::Literal(s) if !stopped => {
                dir.push('/');
                dir.push_str(s);
            }
            Segment::Star => {
                widened = true;
                stopped = true;
            }
            Segment::StarStar => {
                stopped = true;
            }
            // A literal after the first wildcard does not extend host_dir.
            Segment::Literal(_) => {}
        }
    }
    if dir.is_empty() {
        dir.push('/');
    }
    let granularity = if widened {
        PreopenGranularity::WidenedToDir
    } else {
        PreopenGranularity::Exact
    };
    (dir, granularity)
}

/// The `ResolvedPreopen`s one raw fs-cap path yields. Non-G2 / malformed
/// (`expand` → `None`) → empty (fail-closed drop). A brace glob yields one
/// per expanded arm; each records the raw `path` in `from`.
fn preopens_for_path(path: &str, access: PreopenAccess) -> Vec<ResolvedPreopen> {
    match expand(path) {
        None => Vec::new(),
        Some(patterns) => patterns
            .iter()
            .map(|p| {
                let (host_dir, granularity) = resolve_pattern(p);
                ResolvedPreopen {
                    host_dir,
                    access: access.clone(),
                    granularity,
                    from: vec![path.to_string()],
                }
            })
            .collect(),
    }
}

/// The whole host WASI configuration folded from a capability set. Consumed
/// by the wasm host embedder (3.2-paired work) to build a
/// `wasmtime_wasi::WasiCtx`. All non-`Disposition::Wasi` caps contribute
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasiConfiguration {
    /// Egress allow-list: `host_union` of every `net.http` `hosts`. Absent
    /// net.http caps yield `HostSet::Exact({})` (deny-all egress).
    pub allowed_hosts: HostSet,
    /// Allowed HTTP methods across all net.http caps. `None` = all methods
    /// (absorbing); else the union of the per-cap sets. No net.http cap →
    /// `None`.
    pub methods: Option<BTreeSet<HttpMethod>>,
    /// Deduplicated, glob-resolved preopens, sorted by `host_dir`. Empty
    /// absent fs caps.
    pub preopens: Vec<ResolvedPreopen>,
}

impl WasiConfiguration {
    /// The fail-closed WASI configuration: deny-all egress (`HostSet::Exact(∅)`),
    /// no method grants, no preopens. Identical to `resolve_wasi_config([])`.
    /// The wasm host's no-caps entry (`run_component`) uses this so a component
    /// with no declared capabilities receives no WASI authority.
    pub fn deny_all() -> Self {
        Self {
            allowed_hosts: HostSet::Exact(BTreeSet::new()),
            methods: None,
            preopens: Vec::new(),
        }
    }
}

/// Fold a capability set into one host WASI configuration.
///
/// Total and pure: every set yields a [`WasiConfiguration`]. Calls
/// [`map_capability`] per cap and folds the `Disposition::Wasi` fragments —
/// host union, `None`-absorbing method union, and preopen dedup + glob→dir
/// resolution. Fail-closed: no net.http cap → deny-all egress; no fs cap →
/// no preopens; a non-G2 fs path is dropped.
///
/// # Example
///
/// ```
/// use tau_ports::target::wasi_map::{resolve_wasi_config, PreopenAccess, PreopenGranularity};
/// use tau_domain::fixtures::{cap_fs_read, cap_net_http};
///
/// let caps = [cap_net_http(&["api.example.com"], &["POST"]), cap_fs_read(&["/data/**"])];
/// let cfg = resolve_wasi_config(&caps);
/// assert_eq!(cfg.preopens.len(), 1);
/// assert_eq!(cfg.preopens[0].host_dir, "/data");
/// assert_eq!(cfg.preopens[0].access, PreopenAccess::ReadOnly);
/// assert_eq!(cfg.preopens[0].granularity, PreopenGranularity::Exact);
/// ```
pub fn resolve_wasi_config<'a>(
    caps: impl IntoIterator<Item = &'a Capability>,
) -> WasiConfiguration {
    let mut host_sets: Vec<HostSet> = Vec::new();
    let mut any_net = false;
    let mut methods_all = false;
    let mut method_union: BTreeSet<HttpMethod> = BTreeSet::new();
    let mut raw_preopens: Vec<ResolvedPreopen> = Vec::new();

    for cap in caps {
        match map_capability(cap).config {
            WasiConfig::AllowedHosts { hosts, methods } => {
                any_net = true;
                host_sets.push(hosts);
                match methods {
                    None => methods_all = true,
                    Some(s) => method_union.extend(s),
                }
            }
            WasiConfig::Preopens(preopens) => {
                for p in preopens {
                    for path in &p.paths {
                        raw_preopens.extend(preopens_for_path(path, p.access.clone()));
                    }
                }
            }
            WasiConfig::None => {}
        }
    }

    // host_union over an empty iterator returns Exact(∅) = deny-all (host.rs:21).
    let allowed_hosts = host_union(host_sets.iter());
    let methods = if !any_net || methods_all {
        None
    } else {
        Some(method_union)
    };

    WasiConfiguration {
        allowed_hosts,
        methods,
        preopens: dedup_preopens(raw_preopens),
    }
}

/// Merge preopens by host directory: RW absorbs RO, `WidenedToDir` absorbs
/// `Exact`, `from` concatenated. `BTreeMap` yields a deterministic
/// host_dir-sorted result. Nested directories stay separate (distinct keys).
fn dedup_preopens(raw: Vec<ResolvedPreopen>) -> Vec<ResolvedPreopen> {
    use alloc::collections::btree_map::Entry;
    let mut by_dir: BTreeMap<String, ResolvedPreopen> = BTreeMap::new();
    for p in raw {
        match by_dir.entry(p.host_dir.clone()) {
            Entry::Occupied(mut o) => {
                let e = o.get_mut();
                if p.access == PreopenAccess::ReadWrite {
                    e.access = PreopenAccess::ReadWrite;
                }
                if p.granularity == PreopenGranularity::WidenedToDir {
                    e.granularity = PreopenGranularity::WidenedToDir;
                }
                e.from.extend(p.from);
            }
            Entry::Vacant(v) => {
                v.insert(p);
            }
        }
    }
    by_dir.into_values().collect()
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
    use tau_domain::package::capability::lattice::glob::expand;
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

    #[test]
    fn resolve_pattern_glob_to_dir_table() {
        // (input glob, expected host_dir, expected granularity)
        let cases = [
            ("/data/**", "/data", PreopenGranularity::Exact),
            ("/data/sub/**", "/data/sub", PreopenGranularity::Exact),
            ("/data/*", "/data", PreopenGranularity::WidenedToDir),
            ("/var/log/*", "/var/log", PreopenGranularity::WidenedToDir),
            ("/data/*/logs/**", "/data", PreopenGranularity::WidenedToDir),
            ("/srv", "/srv", PreopenGranularity::Exact), // all-literal dir
            ("/etc/app.conf", "/etc/app.conf", PreopenGranularity::Exact), // all-literal; preopen self, NOT /etc
            ("/x.txt", "/x.txt", PreopenGranularity::Exact), // all-literal; NOT root "/"
            ("/**", "/", PreopenGranularity::Exact),         // whole FS, exact
            ("/*", "/", PreopenGranularity::WidenedToDir),   // whole-segment star at root
        ];
        for (glob, dir, gran) in cases {
            let pats = expand(glob).expect("G2 valid");
            assert_eq!(pats.len(), 1, "{glob} brace-expands to one pattern");
            let (host_dir, granularity) = resolve_pattern(&pats[0]);
            assert_eq!(host_dir, dir, "host_dir for {glob}");
            assert_eq!(granularity, gran, "granularity for {glob}");
        }
    }

    #[test]
    fn preopens_for_path_non_g2_is_dropped_fail_closed() {
        // Intra-segment wildcard is not G2; expand -> None -> no preopen.
        assert!(expand("/data/*.txt").is_none(), "intra-segment * is non-G2");
        assert_eq!(
            preopens_for_path("/data/*.txt", PreopenAccess::ReadOnly),
            Vec::new()
        );
    }

    #[test]
    fn preopens_for_path_brace_expands_to_multiple() {
        // A brace glob yields one ResolvedPreopen per arm, carrying the raw path in `from`.
        let got = preopens_for_path("/data/{a,b}/**", PreopenAccess::ReadOnly);
        let dirs: Vec<&str> = got.iter().map(|p| p.host_dir.as_str()).collect();
        assert_eq!(dirs, vec!["/data/a", "/data/b"]);
        assert!(got.iter().all(|p| p.access == PreopenAccess::ReadOnly));
        assert!(got
            .iter()
            .all(|p| p.granularity == PreopenGranularity::Exact));
        assert!(got
            .iter()
            .all(|p| p.from == vec!["/data/{a,b}/**".to_string()]));
    }

    #[test]
    fn host_fold_unions_exact_and_any_absorbs() {
        let a =
            resolve_wasi_config(&[cap_net_http(&["a.com"], &[]), cap_net_http(&["b.com"], &[])]);
        assert_eq!(a.allowed_hosts, exact(&["a.com", "b.com"]));
        let b = resolve_wasi_config(&[cap_net_http(&["a.com"], &[]), cap_net_http(&["any"], &[])]);
        assert_eq!(b.allowed_hosts, HostSet::Any);
    }

    #[test]
    fn method_fold_unions_and_none_absorbs() {
        let post = HttpMethod::parse("POST").unwrap();
        let get = HttpMethod::parse("GET").unwrap();
        // Some ∪ Some = union
        let a = resolve_wasi_config(&[
            cap_net_http(&["a.com"], &["POST"]),
            cap_net_http(&["a.com"], &["GET"]),
        ]);
        assert_eq!(
            a.methods,
            Some([get, post].into_iter().collect::<BTreeSet<_>>())
        );
        // Some ∪ None = None (all methods absorbs)
        let b = resolve_wasi_config(&[
            cap_net_http(&["a.com"], &["POST"]),
            cap_net_http(&["a.com"], &[]),
        ]);
        assert_eq!(b.methods, None);
    }

    #[test]
    fn no_net_cap_is_deny_all_egress() {
        let cfg = resolve_wasi_config(&[cap_fs_read(&["/data/**"])]);
        assert_eq!(cfg.allowed_hosts, HostSet::Exact(BTreeSet::new())); // deny-all, NOT Any
        assert_eq!(cfg.methods, None);
    }

    #[test]
    fn empty_cap_set_grants_nothing() {
        let cfg = resolve_wasi_config(core::iter::empty());
        assert_eq!(cfg.allowed_hosts, HostSet::Exact(BTreeSet::new()));
        assert_eq!(cfg.methods, None);
        assert!(cfg.preopens.is_empty());
    }

    #[test]
    fn preopen_dedup_rw_wins_and_widen_sticks() {
        // Same dir seen RO (from **, exact) and RW (from **, exact) -> single RW.
        let same = resolve_wasi_config(&[
            cap_fs_read(&["/data/**"]),
            cap_fs_write(&["/data/**"], None),
        ]);
        assert_eq!(same.preopens.len(), 1);
        assert_eq!(same.preopens[0].host_dir, "/data");
        assert_eq!(same.preopens[0].access, PreopenAccess::ReadWrite);
        assert_eq!(same.preopens[0].granularity, PreopenGranularity::Exact);

        // Same dir, one contributor widened (/data/*) -> granularity WidenedToDir.
        let widen = resolve_wasi_config(&[cap_fs_read(&["/data/*"]), cap_fs_read(&["/data/**"])]);
        assert_eq!(widen.preopens.len(), 1);
        assert_eq!(widen.preopens[0].host_dir, "/data");
        assert_eq!(widen.preopens[0].access, PreopenAccess::ReadOnly);
        assert_eq!(
            widen.preopens[0].granularity,
            PreopenGranularity::WidenedToDir
        );
    }

    #[test]
    fn preopen_nested_dirs_kept_separate_and_sorted() {
        let cfg = resolve_wasi_config(&[
            cap_fs_write(&["/data/other"], None),
            cap_fs_read(&["/data/**"]),
        ]);
        let dirs: Vec<&str> = cfg.preopens.iter().map(|p| p.host_dir.as_str()).collect();
        assert_eq!(dirs, vec!["/data", "/data/other"]); // BTreeMap order (sorted, deterministic)
        assert_eq!(cfg.preopens[0].access, PreopenAccess::ReadOnly); // /data       RO
        assert_eq!(cfg.preopens[1].access, PreopenAccess::ReadWrite); // /data/other RW
    }

    #[test]
    fn non_wasi_dispositions_contribute_nothing() {
        // agent.spawn is InGuest; process.spawn / fs.exec are Unsupported; none add config.
        let cfg = resolve_wasi_config(&[cap_process_spawn(&["ls"]), cap_fs_exec(&["/bin/**"])]);
        assert_eq!(cfg.allowed_hosts, HostSet::Exact(BTreeSet::new()));
        assert!(cfg.preopens.is_empty());
    }

    #[test]
    fn deny_all_equals_empty_fold() {
        // The no-caps host config must be byte-identical to folding zero caps:
        // deny-all egress, no method grants, no preopens.
        let folded = resolve_wasi_config(core::iter::empty::<&Capability>());
        assert_eq!(WasiConfiguration::deny_all(), folded);
        assert_eq!(
            WasiConfiguration::deny_all().allowed_hosts,
            HostSet::Exact(Default::default())
        );
        assert!(WasiConfiguration::deny_all().methods.is_none());
        assert!(WasiConfiguration::deny_all().preopens.is_empty());
    }
}
