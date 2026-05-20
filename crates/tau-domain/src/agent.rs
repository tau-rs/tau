//! Agent definition + lifecycle status types.
//!
//! tau-domain holds the *vocabulary* (identity, definition, status enum).
//! State-machine *transitions* live in tau-runtime — see G2 / spec §3.4.

use crate::id::{AgentId, PackageName};
use crate::package::PackageId;
use crate::value::Value;
use std::collections::BTreeMap;

/// Agent lifecycle status. Carries diagnostic data only on `Failed`;
/// transition rules live in tau-runtime.
///
/// State graph (informational; not enforced here):
/// `Declared → Installed → Ready → Running ↔ Stopped`,
/// with `Failed` reachable from any non-terminal state.
///
/// # Example
///
/// ```
/// use tau_domain::{AgentStatus, FailureKind};
///
/// // The `Failed` variant is variant-level `#[non_exhaustive]`, so
/// // external callers use [`AgentStatus::failed`] instead of
/// // struct-literal construction.
/// let s = AgentStatus::failed(
///     FailureKind::BackendError,
///     Some("connection refused: api.openai.com".into()),
/// );
/// match s {
///     AgentStatus::Failed { kind: FailureKind::BackendError, .. } => {
///         // retry with backoff
///     }
///     _ => panic!(),
/// }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AgentStatus {
    /// Manifest seen, package not yet installed.
    Declared,
    /// Package installed on disk, ready to instantiate.
    Installed,
    /// Instance created, idle.
    Ready,
    /// Actively processing a message.
    Running,
    /// Intentionally halted.
    Stopped,
    /// The agent failed. `kind` enables typed retry/restart logic;
    /// `detail` carries human-readable specifics.
    #[non_exhaustive]
    Failed {
        /// Typed failure category for restart logic.
        kind: FailureKind,
        /// Human-readable detail (e.g. `"panic at src/foo.rs:42"`).
        detail: Option<String>,
    },
}

impl AgentStatus {
    /// Construct an [`AgentStatus::Failed`] with the given `kind` and
    /// optional `detail`.
    ///
    /// `Failed` is variant-level `#[non_exhaustive]` (E0639), which
    /// blocks struct-literal construction from outside `tau-domain`.
    /// Callers (notably tau-runtime when reporting agent-level
    /// failures from `Runtime::run`) use this constructor.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_domain::{AgentStatus, FailureKind};
    ///
    /// let s = AgentStatus::failed(FailureKind::BackendError, Some("HTTP 503".into()));
    /// match s {
    ///     AgentStatus::Failed { kind: FailureKind::BackendError, .. } => {}
    ///     _ => panic!(),
    /// }
    /// ```
    pub fn failed(kind: FailureKind, detail: Option<String>) -> Self {
        AgentStatus::Failed { kind, detail }
    }
}

/// Categorical failure kinds. New typed kinds are added additively;
/// `InternalError` is the catch-all escape hatch.
///
/// See: [escape-hatches.md#failurekind-internalerror](../docs/explanation/escape-hatches.md#failurekind-internalerror).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FailureKind {
    /// Agent process crashed unexpectedly (panic, signal, abort).
    Crashed,
    /// Configured LLM backend returned an error or was unreachable.
    BackendError,
    /// A capability check denied an operation the agent attempted.
    PolicyDenied,
    /// Agent exceeded a resource limit (memory, message rate, timeout).
    OutOfResources,
    /// Catch-all for failures that don't match the named kinds.
    /// See: [escape-hatches.md#failurekind-internalerror](../docs/explanation/escape-hatches.md#failurekind-internalerror).
    InternalError,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_can_carry_detail() {
        let s = AgentStatus::Failed {
            kind: FailureKind::Crashed,
            detail: Some("SIGSEGV".into()),
        };
        match s {
            AgentStatus::Failed {
                kind: FailureKind::Crashed,
                detail: Some(d),
            } => {
                assert_eq!(d, "SIGSEGV");
            }
            _ => panic!(),
        }
    }
}

/// Static description of an agent. Holds what the runtime needs to
/// instantiate one; richer config lives in skills / plugin packages
/// per G2.
///
/// # Example
///
/// ```
/// use tau_domain::{AgentDefinition, AgentId, PackageId, PackageName, Version};
/// use std::str::FromStr;
///
/// // `PackageId` is `#[non_exhaustive]` — use `::new` instead of
/// // struct-literal construction across crate boundaries.
/// let pkg = PackageId::new(
///     PackageName::from_str("research-pkg").unwrap(),
///     Version::parse("0.1.0").unwrap(),
/// );
/// let def = AgentDefinition::new(
///     AgentId::from_str("researcher").unwrap(),
///     "Researcher".into(),
///     pkg,
///     PackageName::from_str("claude-anthropic").unwrap(),
/// );
/// assert_eq!(def.id.as_str(), "researcher");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AgentDefinition {
    /// Canonical identifier for this definition.
    pub id: AgentId,
    /// Human-readable display name (free-form, unvalidated).
    pub display_name: String,
    /// Which package this agent ships from.
    pub package: PackageId,
    /// Reference to an installed LLM-backend plugin package.
    /// Required at v0.1 (see ADR-0002 escape clause).
    pub llm_backend: PackageName,
    /// Optional system prompt.
    pub system_prompt: Option<String>,
    /// Free-form per-agent config (validated by plugins, not by tau-domain).
    pub config: BTreeMap<String, Value>,
}

impl AgentDefinition {
    /// Construct an `AgentDefinition` with empty `system_prompt` and
    /// `config`. Use the `with_*` builders to fill them in.
    pub fn new(
        id: AgentId,
        display_name: String,
        package: PackageId,
        llm_backend: PackageName,
    ) -> Self {
        Self {
            id,
            display_name,
            package,
            llm_backend,
            system_prompt: None,
            config: BTreeMap::new(),
        }
    }

    /// Set `system_prompt`.
    pub fn with_system_prompt(mut self, prompt: String) -> Self {
        self.system_prompt = Some(prompt);
        self
    }

    /// Set `config`.
    pub fn with_config(mut self, config: BTreeMap<String, Value>) -> Self {
        self.config = config;
        self
    }
}

#[cfg(test)]
mod definition_tests {
    use super::*;
    use crate::version::Version;
    use std::str::FromStr;

    #[test]
    fn builder_chain_sets_fields() {
        let def = AgentDefinition::new(
            AgentId::from_str("a").unwrap(),
            "A".into(),
            PackageId {
                name: PackageName::from_str("p").unwrap(),
                version: Version::parse("0.0.1").unwrap(),
            },
            PackageName::from_str("b").unwrap(),
        )
        .with_system_prompt("hi".into());

        assert_eq!(def.system_prompt.as_deref(), Some("hi"));
    }
}
