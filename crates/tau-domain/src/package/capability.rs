//! Capability declarations attached to a package manifest.
//!
//! Hierarchical typed enum: top-level by namespace
//! (`Filesystem`/`Network`/`Process`/`Agent`/`Custom`), per-namespace
//! verb enums underneath. Variant-level `#[non_exhaustive]` permits
//! additive field evolution.
//!
//! Wire format per ADR-0002: manifest TOML uses flat dot-namespaced
//! `kind = "fs.read"` form. The custom `Deserialize` impl on
//! [`Capability`] maps it onto the variant tree.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

// `HostName` is used by the serde impls and by `shape_tests` (a non-serde
// test); make it available in both configurations without warning when neither.
#[cfg(any(feature = "serde", test))]
use crate::package::host::HostName;
use crate::package::host::{HostSet, HttpMethod};
use crate::value::Value;

pub mod lattice;

/// A capability declaration.
///
/// # Example
///
/// ```
/// use tau_domain::{Capability, CapabilityShape};
/// use std::collections::BTreeMap;
///
/// // `Capability::Custom` is the constructable escape-hatch variant.
/// // Typed variants (`Filesystem`, `Network`, …) are obtained by
/// // deserializing a package manifest (via tau-pkg).
/// let cap = Capability::Custom {
///     name: "my.capability".into(),
///     params: BTreeMap::new(),
/// };
/// assert!(matches!(cap.required_shape(), CapabilityShape::Custom { .. }));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum Capability {
    /// Filesystem-related capability.
    Filesystem(FsCapability),
    /// Network-related capability.
    Network(NetCapability),
    /// Process spawning / signaling capability.
    Process(ProcessCapability),
    /// Inter-agent capability.
    Agent(AgentCapability),
    /// Skill-related capability (Skills-4).
    Skill(SkillCapability),
    /// Read or mutate the shared TaskList of the current multi-agent Run.
    /// `mode` is one of `"read"`, `"write"`, `"manage"`. Not OS-sandbox-enforced;
    /// gated at the virtual-tool dispatch layer in tau-runtime.
    TaskList {
        /// Access mode.
        mode: String,
    },
    /// Read or append to the Run's free-form plan/notes scratchpad.
    /// `mode` is one of `"read"`, `"write"`. Not OS-sandbox-enforced;
    /// gated at the virtual-tool dispatch layer in tau-runtime.
    Plan {
        /// Access mode.
        mode: String,
    },
    /// Plugin-specific capability not yet typed in core. Requires an explicit
    /// `custom.` kind prefix (D7-B PR2) — deliberate escape-hatch intent.
    /// See: [escape-hatches.md#capability-custom](../../../../../docs/explanation/escape-hatches.md#capability-custom).
    Custom {
        /// Capability name (e.g. `"custom.mcp.tool.use"`).
        name: String,
        /// Capability parameters.
        params: BTreeMap<String, Value>,
    },
    /// A capability kind unknown to *this* tau's vocabulary, accepted only
    /// because the manifest declared a newer `vocab_version` (D7-B PR2,
    /// preserves Phase 2 §D forward-compat). Fail-closed in the lattice
    /// (subsumes nothing, subsumed by nothing but an exact match / Any
    /// ceiling) and surfaced by `tau check` as an info finding. Distinct from
    /// [`Capability::Custom`], which is a deliberate local escape hatch.
    /// See: [escape-hatches.md#capability-forward](../../../../../docs/explanation/escape-hatches.md#capability-forward).
    Forward {
        /// The forward (unknown-to-this-tau) capability kind.
        kind: String,
        /// Shape-checked parameter map as supplied.
        params: BTreeMap<String, Value>,
    },
}

/// Whether unknown (non-`custom.`) capability kinds are accepted during
/// deserialization. Authoring surfaces (`tau.toml`, `[allow]`) and interchange
/// readers use [`VocabMode::Strict`]; a package manifest declaring a
/// `vocab_version` newer than [`KNOWN_VOCAB`] uses [`VocabMode::Vocab`], which
/// admits unknown kinds as [`Capability::Forward`] instead of erroring
/// (D7-B PR2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocabMode {
    /// Unknown non-`custom.` kinds are a hard error (with a did-you-mean).
    Strict,
    /// The declared capability-vocabulary generation. When greater than
    /// [`KNOWN_VOCAB`], unknown non-`custom.` kinds parse as
    /// [`Capability::Forward`].
    Vocab(u32),
}

/// The capability-vocabulary generation this build of tau understands. A
/// manifest may declare a `vocab_version` newer than this to opt unknown
/// kinds into forward-compatible [`Capability::Forward`] parsing. Bump this
/// when a kind graduates from `Forward` to a typed variant.
pub const KNOWN_VOCAB: u32 = 1;

// `forward_open` is consulted only by the `serde` deserializer in
// `capability_de` below, so the whole impl tracks that gate — ungated it is a
// deny-level `dead_code` in the feature-less build.
#[cfg(feature = "serde")]
impl VocabMode {
    /// `true` if unknown non-`custom.` kinds should parse as
    /// [`Capability::Forward`] rather than erroring.
    fn forward_open(self) -> bool {
        matches!(self, VocabMode::Vocab(v) if v > KNOWN_VOCAB)
    }
}

/// Filesystem capability verbs.
///
/// Variants are `#[non_exhaustive]` — construction from outside the crate
/// requires manifest deserialization (via tau-pkg). The corresponding
/// [`CapabilityShape`] identifies the sandbox primitive each verb maps to.
///
/// # Example
///
/// ```
/// use tau_domain::CapabilityShape;
///
/// // `CapabilityShape::FilesystemRead` is what an fs.read capability
/// // requires from a sandbox adapter.
/// let shape = CapabilityShape::FilesystemRead;
/// assert!(matches!(shape, CapabilityShape::FilesystemRead));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FsCapability {
    /// Read paths matching the given glob patterns.
    #[non_exhaustive]
    Read {
        /// Glob patterns to grant read access on.
        paths: Vec<String>,
    },
    /// Write paths matching the given globs (with optional size cap).
    #[non_exhaustive]
    Write {
        /// Glob patterns to grant write access on.
        paths: Vec<String>,
        /// Optional maximum write size, in bytes.
        max_bytes: Option<u64>,
    },
    /// Execute (spawn) binaries from paths matching the given globs.
    #[non_exhaustive]
    Exec {
        /// Glob patterns of binaries permitted to execute.
        paths: Vec<String>,
    },
}

/// Network capability verbs.
///
/// Variants are `#[non_exhaustive]` — construction from outside the crate
/// requires manifest deserialization (via tau-pkg). The corresponding
/// [`CapabilityShape`] identifies the sandbox primitive this verb maps to.
///
/// # Example
///
/// ```
/// use tau_domain::CapabilityShape;
///
/// // `CapabilityShape::NetworkHttp` is what a net.http capability
/// // requires from a sandbox adapter.
/// let shape = CapabilityShape::NetworkHttp;
/// assert!(matches!(shape, CapabilityShape::NetworkHttp));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NetCapability {
    /// HTTP requests to the allow-listed hosts and methods.
    #[non_exhaustive]
    Http {
        /// Allowed hosts: exact lowercase hostnames or the typed `Any`
        /// (authored `hosts = "any"`). Absent / empty at parse time is a
        /// hard error (D7-B: `net.http` requires hosts). Suffix wildcards
        /// are not yet supported.
        hosts: HostSet,
        /// Allowed HTTP methods. `None` = all methods; `Some(set)` = only those.
        methods: Option<BTreeSet<HttpMethod>>,
    },
}

/// Process capability verbs.
///
/// Variants are `#[non_exhaustive]` — construction from outside the crate
/// requires manifest deserialization (via tau-pkg). The corresponding
/// [`CapabilityShape`] identifies the sandbox primitive this verb maps to.
///
/// # Example
///
/// ```
/// use tau_domain::CapabilityShape;
///
/// // `CapabilityShape::ProcessExec` is what a process.spawn capability
/// // requires from a sandbox adapter.
/// let shape = CapabilityShape::ProcessExec;
/// assert!(matches!(shape, CapabilityShape::ProcessExec));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ProcessCapability {
    /// Spawn subprocesses for the allow-listed command names.
    #[non_exhaustive]
    Spawn {
        /// Allowed command names.
        commands: Vec<String>,
    },
}

/// Agent capability verbs.
///
/// Variants are `#[non_exhaustive]` — construction from outside the crate
/// requires manifest deserialization (via tau-pkg). The corresponding
/// [`CapabilityShape`] identifies the enforcement primitive this verb maps to.
///
/// # Example
///
/// ```
/// use tau_domain::CapabilityShape;
///
/// // `CapabilityShape::AgentSpawn` is what an agent.spawn capability
/// // requires from the runtime (not OS-sandbox-enforced at v0.1).
/// let shape = CapabilityShape::AgentSpawn;
/// assert!(matches!(shape, CapabilityShape::AgentSpawn));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AgentCapability {
    /// Spawn sub-agents whose package kind matches the allow-list.
    #[non_exhaustive]
    Spawn {
        /// Permitted package kinds (e.g. `["worker"]`).
        allowed_kinds: Vec<String>,
    },
}

/// Skill capability verbs.
///
/// Added by Skills-4 (ROADMAP §16). Skills are an installable
/// package kind that ships a reusable agent behavior (SKILL.md
/// system_prompt + declared capabilities). The `Spawn` variant
/// authorizes a parent agent to invoke installed skills as child
/// agents via the `skill.<name>.spawn` virtual tool.
///
/// Variants are `#[non_exhaustive]` — construction from outside the crate
/// requires manifest deserialization (via tau-pkg). The corresponding
/// [`CapabilityShape`] identifies the enforcement primitive this verb maps to.
///
/// # Example
///
/// ```
/// use tau_domain::CapabilityShape;
///
/// // `CapabilityShape::SkillSpawn` is what a skill.spawn capability
/// // requires from the runtime (gated at virtual-tool dispatch layer).
/// let shape = CapabilityShape::SkillSpawn;
/// assert!(matches!(shape, CapabilityShape::SkillSpawn));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SkillCapability {
    /// Spawn an installed skill as a child agent. `allowed_skills` is
    /// the list of skill names (matching `LockedPackage.name` for
    /// `kind = "skill"` entries in the lockfile) the parent agent
    /// may invoke via `skill.<name>.spawn`.
    #[non_exhaustive]
    Spawn {
        /// Permitted skill names.
        allowed_skills: Vec<String>,
    },
}

/// Typed vocabulary describing the *shape* of enforcement a [`Capability`]
/// requires from a sandbox adapter. Each variant maps to a distinct
/// kernel-level enforcement primitive (filesystem read/write, exec gating,
/// network egress filtering, etc).
///
/// Adapters declare a `CapabilityShapeSet` they support; the runtime
/// cross-checks plan-required vs adapter-supported before spawning a
/// plugin process.
///
/// Variant-level evolution is handled by `#[non_exhaustive]`. Adding a new
/// shape is **additive** — existing adapters that don't support it report
/// `SandboxError::ShapeUnsupported`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CapabilityShape {
    /// Plugin needs read access to a filtered set of paths.
    FilesystemRead,
    /// Plugin needs write access to a filtered set of paths.
    FilesystemWrite,
    /// Plugin needs to exec a binary (covers both `fs.exec` and `process.spawn`
    /// — same kernel surface).
    ProcessExec,
    /// Plugin needs HTTP egress to a filtered host list.
    NetworkHttp,
    /// Plugin needs to spawn a sub-agent. (Future: not enforced by OS sandbox
    /// today; reserved for forward-compat.)
    AgentSpawn,
    /// Plugin / agent needs to spawn an installed skill via the
    /// `skill.<name>.spawn` virtual tool. (Added by Skills-4.)
    SkillSpawn,
    /// Plugin uses a `Capability::Custom` whose enforcement is plugin-defined.
    /// Adapters MAY refuse to sandbox `Custom` shapes.
    /// See: [escape-hatches.md#capability-custom](../../../../../docs/explanation/escape-hatches.md#capability-custom).
    Custom {
        /// Custom capability name (`Capability::Custom { name }`).
        name: String,
    },
}

/// A set of [`CapabilityShape`]s, used by adapters to declare what they support
/// and by the runtime to declare what a plan requires. Subset / membership
/// queries are O(n) where n is the set size; we expect at most ~6 entries.
///
/// # Example
///
/// ```
/// use tau_domain::{CapabilityShape, CapabilityShapeSet};
///
/// let mut adapter = CapabilityShapeSet::new();
/// adapter.insert(CapabilityShape::FilesystemRead);
/// adapter.insert(CapabilityShape::NetworkHttp);
///
/// let mut plan = CapabilityShapeSet::new();
/// plan.insert(CapabilityShape::FilesystemRead);
///
/// assert!(plan.is_subset_of(&adapter));
/// assert!(adapter.contains(&CapabilityShape::NetworkHttp));
/// assert_eq!(adapter.len(), 2);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CapabilityShapeSet {
    inner: Vec<CapabilityShape>,
}

impl CapabilityShapeSet {
    /// Create an empty set.
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    /// Insert a shape (no-op if already present).
    pub fn insert(&mut self, shape: CapabilityShape) {
        if !self.inner.contains(&shape) {
            self.inner.push(shape);
        }
    }

    /// Check whether the set contains a shape.
    pub fn contains(&self, shape: &CapabilityShape) -> bool {
        self.inner.contains(shape)
    }

    /// `true` if every shape in `self` is also in `other`.
    pub fn is_subset_of(&self, other: &CapabilityShapeSet) -> bool {
        self.inner.iter().all(|s| other.inner.contains(s))
    }

    /// Iterate over the shapes.
    pub fn iter(&self) -> impl Iterator<Item = &CapabilityShape> {
        self.inner.iter()
    }

    /// Number of shapes in the set.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// `true` if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Capability {
    /// The [`CapabilityShape`] this capability requires from a sandbox
    /// adapter. Used by `tau-runtime`'s validation layer to cross-check
    /// plan-required shapes against adapter-supported shapes.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_domain::{Capability, CapabilityShape};
    /// use std::collections::BTreeMap;
    ///
    /// let cap = Capability::Custom {
    ///     name: "my.tool".into(),
    ///     params: BTreeMap::new(),
    /// };
    /// assert_eq!(
    ///     cap.required_shape(),
    ///     CapabilityShape::Custom { name: "my.tool".into() },
    /// );
    /// ```
    pub fn required_shape(&self) -> CapabilityShape {
        match self {
            Capability::Filesystem(FsCapability::Read { .. }) => CapabilityShape::FilesystemRead,
            Capability::Filesystem(FsCapability::Write { .. }) => CapabilityShape::FilesystemWrite,
            Capability::Filesystem(FsCapability::Exec { .. }) => CapabilityShape::ProcessExec,
            Capability::Network(NetCapability::Http { .. }) => CapabilityShape::NetworkHttp,
            Capability::Process(ProcessCapability::Spawn { .. }) => CapabilityShape::ProcessExec,
            Capability::Agent(AgentCapability::Spawn { .. }) => CapabilityShape::AgentSpawn,
            Capability::Skill(SkillCapability::Spawn { .. }) => CapabilityShape::SkillSpawn,
            Capability::TaskList { .. } => CapabilityShape::Custom {
                name: "task_list".to_string(),
            },
            Capability::Plan { .. } => CapabilityShape::Custom {
                name: "plan".to_string(),
            },
            Capability::Custom { name, .. } => CapabilityShape::Custom { name: name.clone() },
            // Forward caps have no known enforcement shape; adapters treat the
            // Custom shape as fail-closed (may refuse to sandbox it).
            Capability::Forward { kind, .. } => CapabilityShape::Custom { name: kind.clone() },
        }
    }
}

#[cfg(feature = "serde")]
mod capability_de {
    use super::*;
    use serde::ser::SerializeMap;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// `hosts` is authored as the exact string `"any"` OR a list of host
    /// strings. Untagged so it works in both TOML (manifests) and JSON (the
    /// `[allow]` bridge).
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawHosts {
        Str(String),
        List(Vec<String>),
    }

    fn parse_hosts_field(raw: Option<RawHosts>) -> Result<HostSet, String> {
        match raw {
            // D7-B: `net.http` requires an explicit host ceiling; an absent
            // `hosts` is a hard error, not a silent empty allow-list.
            None => Err("capability kind \"net.http\": requires `hosts` \
                 (a non-empty list, or \"any\" for unrestricted egress)"
                .into()),
            Some(RawHosts::Str(s)) if s == "any" => Ok(HostSet::Any),
            Some(RawHosts::Str(s)) => Err(alloc::format!(
                "net.http hosts: bare string {s:?} is not valid; write hosts = \"any\" or a list of hosts"
            )),
            Some(RawHosts::List(list)) if list.is_empty() => Err(
                "net.http hosts list must be non-empty (use \"any\" for unrestricted egress)".into(),
            ),
            Some(RawHosts::List(list)) => {
                let mut set = alloc::collections::BTreeSet::new();
                for h in list {
                    set.insert(
                        HostName::parse(&h)
                            .map_err(|e| alloc::format!("net.http host {h:?}: {e}"))?,
                    );
                }
                Ok(HostSet::Exact(set))
            }
        }
    }

    fn parse_methods_field(
        raw: Option<Vec<String>>,
    ) -> Result<Option<alloc::collections::BTreeSet<HttpMethod>>, String> {
        match raw {
            None => Ok(None),
            Some(list) => {
                let mut set = alloc::collections::BTreeSet::new();
                for m in list {
                    set.insert(HttpMethod::parse(&m).map_err(|e| e.to_string())?);
                }
                Ok(Some(set))
            }
        }
    }

    #[derive(Deserialize)]
    pub(crate) struct RawCapability {
        kind: String,
        #[serde(default)]
        paths: Option<Vec<String>>,
        #[serde(default)]
        max_bytes: Option<u64>,
        #[serde(default)]
        hosts: Option<RawHosts>,
        #[serde(default)]
        methods: Option<Vec<String>>,
        #[serde(default)]
        commands: Option<Vec<String>>,
        #[serde(default)]
        allowed_kinds: Option<Vec<String>>,
        #[serde(default)]
        allowed_skills: Option<Vec<String>>,
        #[serde(default)]
        mode: Option<String>,
        #[serde(flatten)]
        rest: BTreeMap<String, Value>,
    }

    impl RawCapability {
        /// The named fields the caller actually supplied (excludes `kind` and
        /// the `rest` flatten-map). Drives field-shape strictness (D7-B PR1).
        fn present_named(&self) -> Vec<&'static str> {
            let mut v = Vec::new();
            if self.paths.is_some() {
                v.push("paths");
            }
            if self.max_bytes.is_some() {
                v.push("max_bytes");
            }
            if self.hosts.is_some() {
                v.push("hosts");
            }
            if self.methods.is_some() {
                v.push("methods");
            }
            if self.commands.is_some() {
                v.push("commands");
            }
            if self.allowed_kinds.is_some() {
                v.push("allowed_kinds");
            }
            if self.allowed_skills.is_some() {
                v.push("allowed_skills");
            }
            if self.mode.is_some() {
                v.push("mode");
            }
            v
        }
    }

    impl RawCapability {
        /// Reconstruct the full parameter map for a `custom.`/`Forward`
        /// capability: every field the caller supplied (typed reserved fields
        /// plus the `rest` flatten-map), losslessly, so nothing is silently
        /// dropped (D7-B).
        fn into_params(self) -> BTreeMap<String, Value> {
            let mut params = self.rest;
            let str_array =
                |items: Vec<String>| Value::Array(items.into_iter().map(Value::String).collect());
            if let Some(paths) = self.paths {
                params.insert("paths".to_string(), str_array(paths));
            }
            if let Some(mb) = self.max_bytes {
                params.insert("max_bytes".to_string(), Value::Integer(mb as i64));
            }
            if let Some(hosts) = self.hosts {
                let v = match hosts {
                    RawHosts::Str(s) => Value::String(s),
                    RawHosts::List(h) => str_array(h),
                };
                params.insert("hosts".to_string(), v);
            }
            for (k, list) in [
                ("methods", self.methods),
                ("commands", self.commands),
                ("allowed_kinds", self.allowed_kinds),
                ("allowed_skills", self.allowed_skills),
            ] {
                if let Some(items) = list {
                    params.insert(k.to_string(), str_array(items));
                }
            }
            if let Some(mode) = self.mode {
                params.insert("mode".to_string(), Value::String(mode));
            }
            params
        }
    }

    /// Reject any field the caller supplied that is not valid for `kind`.
    /// `allowed` is the exact set of named fields the kind accepts; anything
    /// else (a named field OR an unknown flatten key) is an error that names
    /// the offending field and the kind's expected shape (D7-B PR1: no more
    /// silent `unwrap_or_default` conflation).
    fn reject_extra_fields(
        raw: &RawCapability,
        allowed: &[&str],
        shape: &str,
    ) -> Result<(), String> {
        for field in raw.present_named() {
            if !allowed.contains(&field) {
                return Err(alloc::format!(
                    "capability kind {:?}: unexpected field {:?} ({} accepts: {})",
                    raw.kind,
                    field,
                    raw.kind,
                    shape
                ));
            }
        }
        if let Some((key, _)) = raw.rest.iter().next() {
            return Err(alloc::format!(
                "capability kind {:?}: unexpected field {:?} ({} accepts: {})",
                raw.kind,
                key,
                raw.kind,
                shape
            ));
        }
        Ok(())
    }

    /// The fixed capability kinds this tau vocabulary knows (excludes the
    /// `custom.` escape-hatch namespace). Drives the did-you-mean suggestion.
    const KNOWN_KINDS: &[&str] = &[
        "fs.read",
        "fs.write",
        "fs.exec",
        "net.http",
        "process.spawn",
        "agent.spawn",
        "skill.spawn",
        "task_list",
        "plan",
    ];

    /// Levenshtein edit distance (no_std, `alloc`-only).
    fn levenshtein(a: &str, b: &str) -> usize {
        let b_chars: Vec<char> = b.chars().collect();
        let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
        let mut cur = alloc::vec![0usize; b_chars.len() + 1];
        for (i, ca) in a.chars().enumerate() {
            cur[0] = i + 1;
            for (j, &cb) in b_chars.iter().enumerate() {
                let cost = if ca == cb { 0 } else { 1 };
                cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
            }
            core::mem::swap(&mut prev, &mut cur);
        }
        prev[b_chars.len()]
    }

    /// Error message for an unrecognized (non-`custom.`) capability kind, with
    /// a did-you-mean suggestion and a pointer to the escape hatch.
    fn unknown_kind_error(kind: &str) -> String {
        let suggestion = KNOWN_KINDS
            .iter()
            .map(|k| (*k, levenshtein(kind, k)))
            .filter(|(_, d)| *d <= 3)
            .min_by_key(|(_, d)| *d)
            .map(|(k, _)| k);
        match suggestion {
            Some(k) => alloc::format!(
                "unknown capability kind {kind:?}; did you mean {k:?}? \
                 (for a plugin-defined capability use a `custom.` prefix — \
                 see docs/explanation/escape-hatches.md)"
            ),
            None => alloc::format!(
                "unknown capability kind {kind:?} \
                 (for a plugin-defined capability use a `custom.` prefix — \
                 see docs/explanation/escape-hatches.md)"
            ),
        }
    }

    /// Resolve a raw capability into a typed [`Capability`] under `mode`.
    /// Shared by the [`Deserialize`] impl (always [`VocabMode::Strict`]) and
    /// the package-manifest two-pass (vocab-aware). Returns a plain-`String`
    /// error so both call sites can adapt it (D7-B PR2).
    pub(crate) fn build_capability(
        raw: RawCapability,
        mode: VocabMode,
    ) -> Result<Capability, String> {
        Ok(match raw.kind.as_str() {
            "fs.read" => {
                reject_extra_fields(&raw, &["paths"], "paths")?;
                Capability::Filesystem(FsCapability::Read {
                    paths: raw
                        .paths
                        .ok_or("capability kind \"fs.read\": requires `paths`")?,
                })
            }
            "fs.write" => {
                reject_extra_fields(&raw, &["paths", "max_bytes"], "paths, max_bytes")?;
                Capability::Filesystem(FsCapability::Write {
                    paths: raw
                        .paths
                        .ok_or("capability kind \"fs.write\": requires `paths`")?,
                    max_bytes: raw.max_bytes,
                })
            }
            "fs.exec" => {
                reject_extra_fields(&raw, &["paths"], "paths")?;
                Capability::Filesystem(FsCapability::Exec {
                    paths: raw
                        .paths
                        .ok_or("capability kind \"fs.exec\": requires `paths`")?,
                })
            }
            "net.http" => {
                reject_extra_fields(&raw, &["hosts", "methods"], "hosts, methods")?;
                let hosts = parse_hosts_field(raw.hosts)?;
                let methods = parse_methods_field(raw.methods)?;
                Capability::Network(NetCapability::Http { hosts, methods })
            }
            "process.spawn" => {
                reject_extra_fields(&raw, &["commands"], "commands")?;
                Capability::Process(ProcessCapability::Spawn {
                    commands: raw
                        .commands
                        .ok_or("capability kind \"process.spawn\": requires `commands`")?,
                })
            }
            "agent.spawn" => {
                reject_extra_fields(&raw, &["allowed_kinds"], "allowed_kinds")?;
                Capability::Agent(AgentCapability::Spawn {
                    allowed_kinds: raw
                        .allowed_kinds
                        .ok_or("capability kind \"agent.spawn\": requires `allowed_kinds`")?,
                })
            }
            "skill.spawn" => {
                reject_extra_fields(&raw, &["allowed_skills"], "allowed_skills")?;
                Capability::Skill(SkillCapability::Spawn {
                    allowed_skills: raw
                        .allowed_skills
                        .ok_or("capability kind \"skill.spawn\": requires `allowed_skills`")?,
                })
            }
            "task_list" => {
                reject_extra_fields(&raw, &["mode"], "mode")?;
                match raw.mode.as_deref() {
                    Some(m @ ("read" | "write" | "manage")) => Capability::TaskList {
                        mode: m.to_string(),
                    },
                    Some(other) => {
                        return Err(alloc::format!(
                            "capability kind \"task_list\": mode {other:?} unsupported \
                             (accepts \"read\", \"write\", \"manage\")"
                        ))
                    }
                    None => return Err("capability kind \"task_list\": requires `mode`".into()),
                }
            }
            "plan" => {
                reject_extra_fields(&raw, &["mode"], "mode")?;
                match raw.mode.as_deref() {
                    Some(m @ ("read" | "write")) => Capability::Plan {
                        mode: m.to_string(),
                    },
                    Some(other) => {
                        return Err(alloc::format!(
                            "capability kind \"plan\": mode {other:?} unsupported \
                             (accepts \"read\", \"write\")"
                        ))
                    }
                    None => return Err("capability kind \"plan\": requires `mode`".into()),
                }
            }
            other if other.starts_with("custom.") => {
                // Explicit escape-hatch intent (D7-B PR2). Arbitrary params.
                let name = raw.kind.clone();
                Capability::Custom {
                    name,
                    params: raw.into_params(),
                }
            }
            other => {
                // Unknown, non-`custom.` kind. Accepted as fail-closed
                // `Forward` only when the manifest declared a newer vocab;
                // otherwise a hard error with a did-you-mean (D7-B PR2).
                if mode.forward_open() {
                    let kind = raw.kind.clone();
                    Capability::Forward {
                        kind,
                        params: raw.into_params(),
                    }
                } else {
                    return Err(unknown_kind_error(other));
                }
            }
        })
    }

    impl<'de> Deserialize<'de> for Capability {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            use serde::de::Error as _;
            let raw = RawCapability::deserialize(d)?;
            // Authoring/interchange deserialization is always strict; the
            // vocab-aware path runs in the package-manifest two-pass.
            build_capability(raw, VocabMode::Strict).map_err(D::Error::custom)
        }
    }

    impl Serialize for Capability {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            match self {
                Capability::Filesystem(FsCapability::Read { paths }) => {
                    let mut m = s.serialize_map(Some(2))?;
                    m.serialize_entry("kind", "fs.read")?;
                    m.serialize_entry("paths", paths)?;
                    m.end()
                }
                Capability::Filesystem(FsCapability::Write { paths, max_bytes }) => {
                    let len = if max_bytes.is_some() { 3 } else { 2 };
                    let mut m = s.serialize_map(Some(len))?;
                    m.serialize_entry("kind", "fs.write")?;
                    m.serialize_entry("paths", paths)?;
                    if let Some(b) = max_bytes {
                        m.serialize_entry("max_bytes", b)?;
                    }
                    m.end()
                }
                Capability::Filesystem(FsCapability::Exec { paths }) => {
                    let mut m = s.serialize_map(Some(2))?;
                    m.serialize_entry("kind", "fs.exec")?;
                    m.serialize_entry("paths", paths)?;
                    m.end()
                }
                Capability::Network(NetCapability::Http { hosts, methods }) => {
                    let len = 2 + usize::from(methods.is_some());
                    let mut m = s.serialize_map(Some(len))?;
                    m.serialize_entry("kind", "net.http")?;
                    match hosts {
                        HostSet::Any => m.serialize_entry("hosts", "any")?,
                        HostSet::Exact(set) => {
                            let list: Vec<&str> = set.iter().map(|h| h.as_str()).collect();
                            m.serialize_entry("hosts", &list)?;
                        }
                    }
                    if let Some(set) = methods {
                        let list: Vec<&str> = set.iter().map(|v| v.as_str()).collect();
                        m.serialize_entry("methods", &list)?;
                    }
                    m.end()
                }
                Capability::Process(ProcessCapability::Spawn { commands }) => {
                    let mut m = s.serialize_map(Some(2))?;
                    m.serialize_entry("kind", "process.spawn")?;
                    m.serialize_entry("commands", commands)?;
                    m.end()
                }
                Capability::Agent(AgentCapability::Spawn { allowed_kinds }) => {
                    let mut m = s.serialize_map(Some(2))?;
                    m.serialize_entry("kind", "agent.spawn")?;
                    m.serialize_entry("allowed_kinds", allowed_kinds)?;
                    m.end()
                }
                Capability::Skill(SkillCapability::Spawn { allowed_skills }) => {
                    let mut m = s.serialize_map(Some(2))?;
                    m.serialize_entry("kind", "skill.spawn")?;
                    m.serialize_entry("allowed_skills", allowed_skills)?;
                    m.end()
                }
                Capability::TaskList { mode } => {
                    let mut m = s.serialize_map(Some(2))?;
                    m.serialize_entry("kind", "task_list")?;
                    m.serialize_entry("mode", mode)?;
                    m.end()
                }
                Capability::Plan { mode } => {
                    let mut m = s.serialize_map(Some(2))?;
                    m.serialize_entry("kind", "plan")?;
                    m.serialize_entry("mode", mode)?;
                    m.end()
                }
                Capability::Custom { name, params } => {
                    let mut m = s.serialize_map(Some(1 + params.len()))?;
                    m.serialize_entry("kind", name)?;
                    for (k, v) in params {
                        m.serialize_entry(k, v)?;
                    }
                    m.end()
                }
                Capability::Forward { kind, params } => {
                    let mut m = s.serialize_map(Some(1 + params.len()))?;
                    m.serialize_entry("kind", kind)?;
                    for (k, v) in params {
                        m.serialize_entry(k, v)?;
                    }
                    m.end()
                }
            }
        }
    }
}

/// Vocab-aware capability resolution shared with the package-manifest
/// two-pass (D7-B PR2). `RawCapability` deserializes the flat wire shape;
/// `build_capability` applies field-shape strictness, the `custom.` escape
/// hatch, and (under a newer `vocab_version`) `Forward` acceptance.
#[cfg(feature = "serde")]
pub(crate) use capability_de::{build_capability, RawCapability};

#[cfg(feature = "schema")]
impl schemars::JsonSchema for Capability {
    fn schema_name() -> alloc::borrow::Cow<'static, str> {
        "Capability".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "title": "capability",
            "oneOf": [
                // fs.read
                {
                    "type": "object",
                    "required": ["kind", "paths"],
                    "additionalProperties": false,
                    "properties": {
                        "kind":  { "const": "fs.read" },
                        "paths": { "type": "array", "items": { "type": "string" } }
                    }
                },
                // fs.write  (max_bytes is optional — in properties but not required)
                {
                    "type": "object",
                    "required": ["kind", "paths"],
                    "additionalProperties": false,
                    "properties": {
                        "kind":      { "const": "fs.write" },
                        "paths":     { "type": "array", "items": { "type": "string" } },
                        "max_bytes": { "type": "integer", "minimum": 0 }
                    }
                },
                // fs.exec
                {
                    "type": "object",
                    "required": ["kind", "paths"],
                    "additionalProperties": false,
                    "properties": {
                        "kind":  { "const": "fs.exec" },
                        "paths": { "type": "array", "items": { "type": "string" } }
                    }
                },
                // net.http  (hosts: "any" | non-empty list; methods optional)
                {
                    "type": "object",
                    "required": ["kind", "hosts"],
                    "additionalProperties": false,
                    "properties": {
                        "kind":  { "const": "net.http" },
                        "hosts": {
                            "oneOf": [
                                { "const": "any" },
                                { "type": "array", "items": { "type": "string" }, "minItems": 1 }
                            ]
                        },
                        "methods": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": ["GET","HEAD","POST","PUT","DELETE","CONNECT","OPTIONS","TRACE","PATCH"]
                            }
                        }
                    }
                },
                // process.spawn
                {
                    "type": "object",
                    "required": ["kind", "commands"],
                    "additionalProperties": false,
                    "properties": {
                        "kind":     { "const": "process.spawn" },
                        "commands": { "type": "array", "items": { "type": "string" } }
                    }
                },
                // agent.spawn
                {
                    "type": "object",
                    "required": ["kind", "allowed_kinds"],
                    "additionalProperties": false,
                    "properties": {
                        "kind":          { "const": "agent.spawn" },
                        "allowed_kinds": { "type": "array", "items": { "type": "string" } }
                    }
                },
                // skill.spawn
                {
                    "type": "object",
                    "required": ["kind", "allowed_skills"],
                    "additionalProperties": false,
                    "properties": {
                        "kind":           { "const": "skill.spawn" },
                        "allowed_skills": { "type": "array", "items": { "type": "string" } }
                    }
                },
                // task_list  (mode is one of "read"/"write"/"manage")
                {
                    "type": "object",
                    "required": ["kind", "mode"],
                    "additionalProperties": false,
                    "properties": {
                        "kind": { "const": "task_list" },
                        "mode": { "type": "string", "enum": ["read", "write", "manage"] }
                    }
                },
                // plan  (mode is one of "read"/"write")
                {
                    "type": "object",
                    "required": ["kind", "mode"],
                    "additionalProperties": false,
                    "properties": {
                        "kind": { "const": "plan" },
                        "mode": { "type": "string", "enum": ["read", "write"] }
                    }
                },
                // Custom — arbitrary kind string that is NOT one of the 9 fixed kinds,
                // plus arbitrary additional params (no additionalProperties:false).
                {
                    "type": "object",
                    "required": ["kind"],
                    "properties": {
                        "kind": {
                            "type": "string",
                            "not": {
                                "enum": [
                                    "fs.read",
                                    "fs.write",
                                    "fs.exec",
                                    "net.http",
                                    "process.spawn",
                                    "agent.spawn",
                                    "skill.spawn",
                                    "task_list",
                                    "plan"
                                ]
                            }
                        }
                    }
                }
            ]
        })
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    #[test]
    fn fs_read_required_shape() {
        let cap = Capability::Filesystem(FsCapability::Read {
            paths: vec!["/tmp/**".into()],
        });
        assert_eq!(cap.required_shape(), CapabilityShape::FilesystemRead);
    }

    #[test]
    fn fs_write_required_shape() {
        let cap = Capability::Filesystem(FsCapability::Write {
            paths: vec!["/tmp/x".into()],
            max_bytes: None,
        });
        assert_eq!(cap.required_shape(), CapabilityShape::FilesystemWrite);
    }

    #[test]
    fn fs_exec_required_shape() {
        let cap = Capability::Filesystem(FsCapability::Exec {
            paths: vec!["/usr/bin/git".into()],
        });
        assert_eq!(cap.required_shape(), CapabilityShape::ProcessExec);
    }

    #[test]
    fn net_http_required_shape() {
        let cap = Capability::Network(NetCapability::Http {
            hosts: HostSet::Exact(
                [HostName::parse("api.example.com").unwrap()]
                    .into_iter()
                    .collect(),
            ),
            methods: Some([HttpMethod::Get].into_iter().collect()),
        });
        assert_eq!(cap.required_shape(), CapabilityShape::NetworkHttp);
    }

    #[test]
    fn process_spawn_required_shape() {
        let cap = Capability::Process(ProcessCapability::Spawn {
            commands: vec!["git".into()],
        });
        assert_eq!(cap.required_shape(), CapabilityShape::ProcessExec);
    }

    #[test]
    fn agent_spawn_required_shape() {
        let cap = Capability::Agent(AgentCapability::Spawn {
            allowed_kinds: vec!["worker".into()],
        });
        assert_eq!(cap.required_shape(), CapabilityShape::AgentSpawn);
    }

    #[test]
    fn custom_required_shape_is_custom() {
        let cap = Capability::Custom {
            name: "mcp.tool.use".into(),
            params: Default::default(),
        };
        match cap.required_shape() {
            CapabilityShape::Custom { name } => assert_eq!(name, "mcp.tool.use"),
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn shape_set_contains_and_is_subset() {
        let mut a = CapabilityShapeSet::new();
        a.insert(CapabilityShape::FilesystemRead);
        a.insert(CapabilityShape::NetworkHttp);
        let mut b = CapabilityShapeSet::new();
        b.insert(CapabilityShape::FilesystemRead);
        b.insert(CapabilityShape::FilesystemWrite);
        b.insert(CapabilityShape::NetworkHttp);
        assert!(a.is_subset_of(&b));
        assert!(!b.is_subset_of(&a));
        assert!(a.contains(&CapabilityShape::FilesystemRead));
        assert!(!a.contains(&CapabilityShape::FilesystemWrite));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_read_constructs() {
        let c = Capability::Filesystem(FsCapability::Read {
            paths: vec!["/tmp/**".into()],
        });
        match &c {
            Capability::Filesystem(FsCapability::Read { paths }) => {
                assert_eq!(*paths, vec!["/tmp/**".to_string()]);
            }
            _ => panic!("expected Filesystem(Read), got {:?}", c),
        }
    }

    #[test]
    fn custom_constructs() {
        let mut params = BTreeMap::new();
        params.insert(
            "servers".into(),
            Value::Array(vec![Value::String("fs-mcp".into())]),
        );
        let _c = Capability::Custom {
            name: "mcp.tool.use".into(),
            params,
        };
    }

    #[cfg(feature = "serde")]
    #[test]
    fn fs_read_round_trips_through_json() {
        let cap = Capability::Filesystem(FsCapability::Read {
            paths: vec!["/tmp/**".into()],
        });
        let json = serde_json::to_string(&cap).unwrap();
        assert_eq!(json, r#"{"kind":"fs.read","paths":["/tmp/**"]}"#);
        let back: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(cap, back);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn task_list_capability_round_trips() {
        for mode in ["read", "write", "manage"] {
            let cap = Capability::TaskList {
                mode: mode.to_string(),
            };
            let json = serde_json::to_string(&cap).unwrap();
            assert_eq!(json, format!(r#"{{"kind":"task_list","mode":"{mode}"}}"#));
            let back: Capability = serde_json::from_str(&json).unwrap();
            assert_eq!(cap, back);
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn plan_capability_round_trips() {
        for mode in ["read", "write"] {
            let cap = Capability::Plan {
                mode: mode.to_string(),
            };
            let json = serde_json::to_string(&cap).unwrap();
            assert_eq!(json, format!(r#"{{"kind":"plan","mode":"{mode}"}}"#));
            let back: Capability = serde_json::from_str(&json).unwrap();
            assert_eq!(cap, back);
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn task_list_with_unknown_mode_is_rejected() {
        // D7-B PR1: unknown mode is a hard error, not a silent Custom fallback.
        let json = r#"{"kind":"task_list","mode":"bogus"}"#;
        let err = serde_json::from_str::<Capability>(json).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("task_list"), "got: {msg}");
        assert!(msg.contains("bogus"), "got: {msg}");
    }

    // ----- D7-B PR1: field-shape strictness -----

    #[cfg(feature = "serde")]
    #[test]
    fn fs_read_requires_paths() {
        let err = serde_json::from_str::<Capability>(r#"{"kind":"fs.read"}"#).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("fs.read") && msg.contains("paths"),
            "got: {msg}"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn net_http_requires_hosts() {
        // The canonical bare-net.http case that used to parse as empty hosts.
        let err = serde_json::from_str::<Capability>(r#"{"kind":"net.http"}"#).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("net.http") && msg.contains("hosts"),
            "got: {msg}"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn net_http_any_host_round_trips() {
        let json = r#"{"kind":"net.http","hosts":"any","methods":[]}"#;
        let cap: Capability = serde_json::from_str(json).unwrap();
        assert!(matches!(
            &cap,
            Capability::Network(NetCapability::Http { hosts, .. }) if hosts.is_any()
        ));
        assert_eq!(serde_json::to_string(&cap).unwrap(), json);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn net_http_host_list_round_trips() {
        let json = r#"{"kind":"net.http","hosts":["api.x.com"],"methods":["GET"]}"#;
        let cap: Capability = serde_json::from_str(json).unwrap();
        assert!(matches!(
            &cap,
            Capability::Network(NetCapability::Http { hosts, .. })
                if hosts.exact_hosts() == vec!["api.x.com".to_string()]
        ));
        assert_eq!(serde_json::to_string(&cap).unwrap(), json);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn net_http_empty_host_list_rejected() {
        let err =
            serde_json::from_str::<Capability>(r#"{"kind":"net.http","hosts":[]}"#).unwrap_err();
        assert!(err.to_string().contains("non-empty"), "got: {err}");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn paths_on_net_http_rejected() {
        let json = r#"{"kind":"net.http","hosts":"any","paths":["/x"]}"#;
        let err = serde_json::from_str::<Capability>(json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("net.http") && msg.contains("paths"),
            "got: {msg}"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn hosts_on_fs_read_rejected() {
        let json = r#"{"kind":"fs.read","paths":["/x"],"hosts":["a.com"]}"#;
        let err = serde_json::from_str::<Capability>(json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("fs.read") && msg.contains("hosts"),
            "got: {msg}"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn unknown_field_on_known_kind_rejected() {
        let json = r#"{"kind":"fs.read","paths":["/x"],"bogus":1}"#;
        let err = serde_json::from_str::<Capability>(json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("fs.read") && msg.contains("bogus"),
            "got: {msg}"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn task_list_required_shape_is_custom_named_task_list() {
        let cap = Capability::TaskList {
            mode: "read".into(),
        };
        match cap.required_shape() {
            CapabilityShape::Custom { name } => assert_eq!(name, "task_list"),
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn custom_round_trips_through_json() {
        // D7-B PR2: Custom requires an explicit `custom.` kind prefix.
        let mut params = BTreeMap::new();
        params.insert(
            "servers".into(),
            Value::Array(vec![Value::String("fs-mcp".into())]),
        );
        let cap = Capability::Custom {
            name: "custom.mcp.tool.use".into(),
            params,
        };
        let json = serde_json::to_string(&cap).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"custom.mcp.tool.use","servers":["fs-mcp"]}"#
        );
        let back: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(cap, back);
    }

    // ----- D7-B PR2: kind strictness + explicit custom. + vocab/Forward -----

    #[cfg(feature = "serde")]
    #[test]
    fn unprefixed_unknown_kind_errors_with_did_you_mean() {
        let err = serde_json::from_str::<Capability>(r#"{"kind":"fs.raed","paths":["/x"]}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown capability kind"), "got: {err}");
        assert!(
            err.contains("fs.read"),
            "should suggest fs.read; got: {err}"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn unprefixed_far_kind_errors_without_suggestion() {
        let err = serde_json::from_str::<Capability>(r#"{"kind":"zzzzzzzz"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown capability kind"), "got: {err}");
        assert!(
            err.contains("custom."),
            "should point to escape hatch; got: {err}"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn custom_prefix_parses_as_custom() {
        let cap: Capability =
            serde_json::from_str(r#"{"kind":"custom.gpu","devices":["nv"]}"#).unwrap();
        match cap {
            Capability::Custom { name, params } => {
                assert_eq!(name, "custom.gpu");
                assert!(params.contains_key("devices"));
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn unknown_kind_forwards_only_under_newer_vocab() {
        let raw: RawCapability =
            serde_json::from_str(r#"{"kind":"gpu.compute","devices":["nv"]}"#).unwrap();
        // Strict / current vocab → error.
        assert!(build_capability(
            serde_json::from_str(r#"{"kind":"gpu.compute"}"#).unwrap(),
            VocabMode::Vocab(KNOWN_VOCAB)
        )
        .is_err());
        // Newer vocab → Forward, params preserved.
        match build_capability(raw, VocabMode::Vocab(KNOWN_VOCAB + 1)).unwrap() {
            Capability::Forward { kind, params } => {
                assert_eq!(kind, "gpu.compute");
                assert!(params.contains_key("devices"));
            }
            other => panic!("expected Forward, got {other:?}"),
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn skill_spawn_capability_round_trips_through_json() {
        let cap = Capability::Skill(SkillCapability::Spawn {
            allowed_skills: vec!["critic".into(), "fact-checker".into()],
        });
        let json = serde_json::to_string(&cap).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"skill.spawn","allowed_skills":["critic","fact-checker"]}"#
        );
        let back: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(cap, back);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn skill_spawn_capability_empty_allowed_skills_round_trips() {
        let cap = Capability::Skill(SkillCapability::Spawn {
            allowed_skills: vec![],
        });
        let json = serde_json::to_string(&cap).unwrap();
        let back: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(cap, back);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn skill_spawn_required_shape_is_skill_spawn() {
        let cap = Capability::Skill(SkillCapability::Spawn {
            allowed_skills: vec!["x".into()],
        });
        assert_eq!(cap.required_shape(), CapabilityShape::SkillSpawn);
    }
}

#[cfg(all(test, feature = "serde"))]
mod net_http_serde_tests {
    use super::*;

    #[test]
    fn hosts_any_round_trips() {
        let c: Capability = serde_json::from_str(r#"{"kind":"net.http","hosts":"any"}"#).unwrap();
        assert!(
            matches!(&c, Capability::Network(NetCapability::Http { hosts, .. }) if hosts.is_any())
        );
        assert_eq!(
            serde_json::to_value(&c).unwrap()["hosts"],
            serde_json::json!("any")
        );
    }

    #[test]
    fn hosts_star_rejected_at_parse() {
        let e =
            serde_json::from_str::<Capability>(r#"{"kind":"net.http","hosts":["*"]}"#).unwrap_err();
        assert!(
            e.to_string().contains("any") || e.to_string().to_lowercase().contains("wildcard"),
            "got: {e}"
        );
    }

    #[test]
    fn methods_absent_is_none_empty_is_some_empty() {
        let all: Capability =
            serde_json::from_str(r#"{"kind":"net.http","hosts":["a.com"]}"#).unwrap();
        let none: Capability =
            serde_json::from_str(r#"{"kind":"net.http","hosts":["a.com"],"methods":[]}"#).unwrap();
        let m = |c: &Capability| match c {
            Capability::Network(NetCapability::Http { methods, .. }) => methods.clone(),
            _ => unreachable!(),
        };
        assert_eq!(m(&all), None);
        assert_eq!(m(&none), Some(alloc::collections::BTreeSet::new()));
    }

    #[test]
    fn unknown_method_rejected() {
        assert!(serde_json::from_str::<Capability>(
            r#"{"kind":"net.http","hosts":["a.com"],"methods":["GTE"]}"#
        )
        .is_err());
    }

    #[test]
    fn exact_hosts_serialize_byte_stable_sorted() {
        // hash-stability: already-lowercase input serializes identically regardless of input order.
        let a: Capability =
            serde_json::from_str(r#"{"kind":"net.http","hosts":["b.com","a.com"]}"#).unwrap();
        assert_eq!(
            serde_json::to_value(&a).unwrap()["hosts"],
            serde_json::json!(["a.com", "b.com"])
        );
    }
}

#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;

    /// All 9 fixed kinds that must appear as `const` in the `oneOf`.
    const FIXED_KINDS: &[&str] = &[
        "fs.read",
        "fs.write",
        "fs.exec",
        "net.http",
        "process.spawn",
        "agent.spawn",
        "skill.spawn",
        "task_list",
        "plan",
    ];

    #[test]
    fn capability_schema_has_ten_oneof_branches() {
        let v = serde_json::to_value(&schemars::schema_for!(Capability)).unwrap();
        let one_of = v["oneOf"].as_array().expect("oneOf must be present");
        assert_eq!(
            one_of.len(),
            10,
            "expected 10 oneOf branches (9 fixed + Custom), got {}",
            one_of.len()
        );
    }

    #[test]
    fn capability_schema_all_fixed_kinds_present_as_const() {
        let v = serde_json::to_value(&schemars::schema_for!(Capability)).unwrap();
        let one_of = v["oneOf"].as_array().expect("oneOf must be present");
        for kind in FIXED_KINDS {
            let found = one_of
                .iter()
                .any(|b| b["properties"]["kind"]["const"].as_str() == Some(kind));
            assert!(found, "kind '{}' not found as a const in oneOf", kind);
        }
    }

    #[test]
    fn capability_schema_custom_branch_has_not_enum_exclusion() {
        let v = serde_json::to_value(&schemars::schema_for!(Capability)).unwrap();
        let one_of = v["oneOf"].as_array().expect("oneOf must be present");
        // The Custom branch has no `const` on `kind`; instead it has a `not`/`enum` exclusion.
        let custom_branch = one_of
            .iter()
            .find(|b| b["properties"]["kind"]["const"].is_null())
            .expect("Custom branch (no const on kind) not found");
        let not_enum = &custom_branch["properties"]["kind"]["not"]["enum"];
        let exclusions = not_enum
            .as_array()
            .expect("not/enum exclusion list missing");
        // All 9 fixed kinds must be excluded
        for kind in FIXED_KINDS {
            assert!(
                exclusions.iter().any(|e| e.as_str() == Some(kind)),
                "fixed kind '{}' missing from Custom branch not/enum exclusion",
                kind
            );
        }
    }

    #[test]
    fn capability_schema_is_oneof_tagged_by_kind() {
        let v = serde_json::to_value(&schemars::schema_for!(Capability)).unwrap();
        let variants = v["oneOf"].as_array().expect("oneOf present");
        // Every fixed branch pins a const "kind" — verified via the const assertion above.
        // At least one branch has kind==fs.read
        assert!(variants
            .iter()
            .any(|b| b["properties"]["kind"]["const"] == "fs.read"));
    }
}
