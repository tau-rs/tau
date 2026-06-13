//! Trigger bindings — compiled, capability-safe metadata describing how
//! tau is invoked. See the framing doc
//! `docs/superpowers/specs/2026-06-13-trigger-ingress-and-serve-transport-framing.md`
//! and ADR-0042.
//!
//! A trigger has two halves: the **substrate** (the scheduler/socket/queue,
//! owned by the host) and the **binding** (declared once, compiled, portable —
//! owned by tau). This module is the binding. It carries no inbound capability
//! and adds no executable node; it is pure metadata that rides in the canonical
//! IR (and thus participates in the content hash).
//!
//! Slice 1 ships `Cron` + `Manual`. `Webhook`/`Queue` are slice 2 (they
//! additionally require a host-adapter contract, which `tau check` will
//! enforce). The enums are `#[non_exhaustive]` so adding those kinds later is
//! a minor change.

use alloc::string::String;
use serde::{Deserialize, Serialize};

use crate::ids::AgentId;

/// The kind of external event a trigger binds to.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerKind {
    /// Fires on a cron schedule. Substrate = systemd/k8s/Lambda scheduler.
    Cron,
    /// The default: tau is invoked by an external driver (a parent process,
    /// CI step, etc.). No scheduler descriptor is emitted.
    Manual,
}

/// Backoff strategy for trigger-level re-invocation (host-honoured).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackoffStrategy {
    /// Constant delay between attempts.
    Fixed,
    /// Exponentially increasing delay, capped at `Backoff::max`.
    Exponential,
}

// Note: the trigger data STRUCTS (Backoff/RetryPolicy/TriggerBinding) are
// deliberately exhaustive — they are constructed by struct-literal in the
// lowering pass and in integration tests (an external crate), matching this
// crate's convention for IR data (IrModule/Workflow/Agent/Tool are all plain
// structs). The forward-compat axis here is the ENUMS (new kinds), which are
// `#[non_exhaustive]`.
/// Backoff parameters. Durations are stored as the author's verbatim
/// duration strings (e.g. `"30s"`, `"10m"`) — they are host-honoured
/// metadata, not values the (no_std) IR interpreter ever computes with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backoff {
    /// `fixed` or `exponential`.
    pub strategy: BackoffStrategy,
    /// Base delay, duration string (e.g. `"30s"`).
    pub base: String,
    /// Cap on the computed delay, duration string (e.g. `"10m"`).
    pub max: String,
}

/// Trigger-level re-invocation policy. This is **not** a per-node interpreter
/// retry: the host (or host adapter) re-invokes the artifact; tau's
/// interpreter stays deterministic and stateless across invocations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Total attempts including the first; `1` = no retry.
    pub max_attempts: u32,
    /// Backoff parameters.
    pub backoff: Backoff,
    /// Where a run that exhausts `max_attempts` is sent — a **sink
    /// reference** (`mcp:<name>` or an already-granted capability target),
    /// never a tau-owned queue. `None` ⇒ no dead-letter sink. The envelope
    /// shape is a runtime concern not modelled in slice 1 (see ADR-0042 §D2).
    pub dead_letter: Option<String>,
}

/// One named trigger binding. Canonically ordered by `name` within
/// `IrModule.triggers`. A trigger is metadata about how tau is invoked,
/// never an executable node.
///
/// Optional fields serialize verbatim (`None` → `null`, no skipping) to
/// match the IR's canonical-encoding discipline (see `canonical.rs`). Only
/// the module-level `triggers` `Vec` skips-when-empty, to preserve
/// trigger-less hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerBinding {
    /// Trigger name (the `[trigger.<name>]` table key).
    pub name: String,
    /// The kind of event this binds to.
    pub kind: TriggerKind,
    /// Entrypoint agent id (validated at lowering against the workflow).
    pub agent: AgentId,
    /// 5-field cron expression (cron kind only; `None` otherwise).
    pub schedule: Option<String>,
    /// IANA timezone name; defaults to `"UTC"` at config-validation time
    /// (cron kind only).
    pub timezone: Option<String>,
    /// Re-invocation policy (`None` = invoke once, no retry).
    pub retry: Option<RetryPolicy>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::AgentId;
    use alloc::string::ToString;

    fn cron_binding() -> TriggerBinding {
        TriggerBinding {
            name: "nightly".to_string(),
            kind: TriggerKind::Cron,
            agent: AgentId("summarizer".to_string()),
            schedule: Some("0 3 * * *".to_string()),
            timezone: Some("UTC".to_string()),
            retry: Some(RetryPolicy {
                max_attempts: 3,
                backoff: Backoff {
                    strategy: BackoffStrategy::Exponential,
                    base: "30s".to_string(),
                    max: "10m".to_string(),
                },
                dead_letter: Some("dlq-sink".to_string()),
            }),
        }
    }

    #[test]
    fn trigger_binding_round_trips_through_json() {
        let b = cron_binding();
        let bytes = serde_json::to_vec(&b).expect("serialize");
        let back: TriggerBinding = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(b, back);
    }

    #[test]
    fn kind_serializes_lowercase() {
        let bytes = serde_json::to_vec(&TriggerKind::Cron).unwrap();
        assert_eq!(bytes, b"\"cron\"");
        let bytes = serde_json::to_vec(&TriggerKind::Manual).unwrap();
        assert_eq!(bytes, b"\"manual\"");
    }

    #[test]
    fn manual_binding_has_no_schedule() {
        let b = TriggerBinding {
            name: "manual".to_string(),
            kind: TriggerKind::Manual,
            agent: AgentId("summarizer".to_string()),
            schedule: None,
            timezone: None,
            retry: None,
        };
        let bytes = serde_json::to_vec(&b).unwrap();
        // Lock the canonical contract: Option fields serialize as `null`,
        // never skipped (the module-level `triggers` Vec is the ONLY field
        // that skips-when-empty — see Task 2). A stray skip_serializing_if
        // here would silently change a trigger-bearing module's hash.
        let json = core::str::from_utf8(&bytes).unwrap();
        assert!(
            json.contains("\"schedule\":null"),
            "schedule must serialize as null: {json}"
        );
        assert!(
            json.contains("\"timezone\":null"),
            "timezone must serialize as null: {json}"
        );
        assert!(
            json.contains("\"retry\":null"),
            "retry must serialize as null: {json}"
        );
        let back: TriggerBinding = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(b, back);
    }
}
