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

use std::collections::BTreeMap;

use crate::value::Value;

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
    /// Plugin-specific capability not yet typed in core.
    /// See: [escape-hatches.md#capability-custom](../../../../../docs/explanation/escape-hatches.md#capability-custom).
    Custom {
        /// Capability name (e.g. `"mcp.tool.use"`).
        name: String,
        /// Capability parameters.
        params: BTreeMap<String, Value>,
    },
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
        /// Allowed hosts (exact match or glob).
        hosts: Vec<String>,
        /// Allowed HTTP methods (uppercase by convention, e.g. `["GET", "POST"]`).
        methods: Vec<String>,
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
        }
    }
}

#[cfg(feature = "serde")]
mod capability_de {
    use super::*;
    use serde::ser::SerializeMap;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Deserialize)]
    struct RawCapability {
        kind: String,
        #[serde(default)]
        paths: Option<Vec<String>>,
        #[serde(default)]
        max_bytes: Option<u64>,
        #[serde(default)]
        hosts: Option<Vec<String>>,
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

    impl<'de> Deserialize<'de> for Capability {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let raw = RawCapability::deserialize(d)?;
            Ok(match raw.kind.as_str() {
                "fs.read" => Capability::Filesystem(FsCapability::Read {
                    paths: raw.paths.unwrap_or_default(),
                }),
                "fs.write" => Capability::Filesystem(FsCapability::Write {
                    paths: raw.paths.unwrap_or_default(),
                    max_bytes: raw.max_bytes,
                }),
                "fs.exec" => Capability::Filesystem(FsCapability::Exec {
                    paths: raw.paths.unwrap_or_default(),
                }),
                "net.http" => Capability::Network(NetCapability::Http {
                    hosts: raw.hosts.unwrap_or_default(),
                    methods: raw.methods.unwrap_or_default(),
                }),
                "process.spawn" => Capability::Process(ProcessCapability::Spawn {
                    commands: raw.commands.unwrap_or_default(),
                }),
                "agent.spawn" => Capability::Agent(AgentCapability::Spawn {
                    allowed_kinds: raw.allowed_kinds.unwrap_or_default(),
                }),
                "skill.spawn" => Capability::Skill(SkillCapability::Spawn {
                    allowed_skills: raw.allowed_skills.unwrap_or_default(),
                }),
                "task_list" => match raw.mode.as_deref() {
                    Some(m @ ("read" | "write" | "manage")) => Capability::TaskList {
                        mode: m.to_string(),
                    },
                    _ => {
                        // Unknown mode: fall back to Custom but preserve mode
                        // in params so downstream tools (capability_satisfies)
                        // can see all fields the caller supplied.
                        let mut params = raw.rest;
                        if let Some(m) = raw.mode {
                            params.insert("mode".into(), Value::String(m));
                        }
                        Capability::Custom {
                            name: raw.kind,
                            params,
                        }
                    }
                },
                "plan" => match raw.mode.as_deref() {
                    Some(m @ ("read" | "write")) => Capability::Plan {
                        mode: m.to_string(),
                    },
                    _ => {
                        let mut params = raw.rest;
                        if let Some(m) = raw.mode {
                            params.insert("mode".into(), Value::String(m));
                        }
                        Capability::Custom {
                            name: raw.kind,
                            params,
                        }
                    }
                },
                _ => {
                    // For any unknown kind, the Custom fallback must
                    // preserve every JSON field the caller supplied. Since
                    // `mode` is a named field on RawCapability (added for
                    // task_list/plan parsing), it does NOT live in `rest`
                    // for non-task_list/plan kinds — re-insert it.
                    let mut params = raw.rest;
                    if let Some(m) = raw.mode {
                        params.insert("mode".into(), Value::String(m));
                    }
                    Capability::Custom {
                        name: raw.kind,
                        params,
                    }
                }
            })
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
                    let mut m = s.serialize_map(Some(3))?;
                    m.serialize_entry("kind", "net.http")?;
                    m.serialize_entry("hosts", hosts)?;
                    m.serialize_entry("methods", methods)?;
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
            }
        }
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
            hosts: vec!["api.example.com".into()],
            methods: vec!["GET".into()],
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
    fn task_list_with_unknown_mode_falls_back_to_custom() {
        let json = r#"{"kind":"task_list","mode":"bogus"}"#;
        let cap: Capability = serde_json::from_str(json).unwrap();
        match cap {
            Capability::Custom { name, .. } => assert_eq!(name, "task_list"),
            other => panic!("expected Custom-fallback, got {other:?}"),
        }
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
        let mut params = BTreeMap::new();
        params.insert(
            "servers".into(),
            Value::Array(vec![Value::String("fs-mcp".into())]),
        );
        let cap = Capability::Custom {
            name: "mcp.tool.use".into(),
            params,
        };
        let json = serde_json::to_string(&cap).unwrap();
        assert_eq!(json, r#"{"kind":"mcp.tool.use","servers":["fs-mcp"]}"#);
        let back: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(cap, back);
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
