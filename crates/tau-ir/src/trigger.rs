//! Trigger bindings — compiled, capability-safe metadata describing how
//! tau is invoked. See the framing doc
//! `docs/superpowers/specs/2026-06-13-trigger-ingress-and-serve-transport-framing.md`
//! and ADR-0044.
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

// schemars 0.8 derive generates code using bare `Box`/`String`/`vec!`
// from the std prelude — import it when the feature is active.
#[cfg(feature = "schema")]
#[allow(unused_imports)]
use std::prelude::rust_2021::*;

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::ids::AgentId;

/// The kind of external event a trigger binds to.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RetryPolicy {
    /// Total attempts including the first; `1` = no retry.
    pub max_attempts: u32,
    /// Backoff parameters.
    pub backoff: Backoff,
    /// Where a run that exhausts `max_attempts` is sent — a **sink
    /// reference** (`mcp:<name>` or an already-granted capability target),
    /// never a tau-owned queue. `None` ⇒ no dead-letter sink. The envelope
    /// shape is a runtime concern not modelled in slice 1 (see ADR-0044 §D2).
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

/// Day-of-week names systemd's `OnCalendar` expects, indexed by cron dow
/// (0 and 7 both = Sunday).
const DOW_NAMES: [&str; 8] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Translate a 5-field cron expression to a systemd `OnCalendar` value, for
/// the slice-1 subset where each field is `*` or a plain non-negative
/// integer. Returns `None` for any field using ranges (`-`), lists (`,`), or
/// steps (`/`) — the caller skips the systemd timer for such triggers and
/// logs a warning (k8s still emits the cron verbatim).
pub fn cron_to_oncalendar(schedule: &str) -> Option<String> {
    let f: Vec<&str> = schedule.split_whitespace().collect();
    if f.len() != 5 {
        return None;
    }
    // Each field must be `*` or all-ASCII-digits.
    fn field(s: &str) -> Option<Option<u8>> {
        if s == "*" {
            Some(None)
        } else if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
            s.parse::<u8>().ok().map(Some)
        } else {
            None // ranges / lists / steps → unsupported
        }
    }
    let min = field(f[0])?;
    let hour = field(f[1])?;
    let dom = field(f[2])?;
    let month = field(f[3])?;
    let dow = field(f[4])?;

    let two = |v: Option<u8>| match v {
        None => "*".to_string(),
        Some(n) => format!("{n:02}"),
    };
    // OnCalendar date+time: `[DOW ]YYYY-MM-DD HH:MM:SS` with `*` wildcards.
    let date = format!("*-{}-{}", two(month), two(dom));
    let time = format!("{}:{}:00", two(hour), two(min));
    let body = format!("{date} {time}");
    match dow {
        None => Some(body),
        Some(d) if (d as usize) < DOW_NAMES.len() => {
            Some(format!("{} {}", DOW_NAMES[d as usize], body))
        }
        Some(_) => None, // out-of-range dow
    }
}

/// Emit systemd `.service` + `.timer` descriptors for each **cron** trigger.
/// `artifact_ref` is the path the unit invokes (the built `.tau` bundle).
/// Manual triggers and cron schedules outside the converter subset produce
/// no output (the caller logs the skip). Returns `(filename, content)` pairs.
pub fn emit_systemd(bindings: &[TriggerBinding], artifact_ref: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for b in bindings {
        if b.kind != TriggerKind::Cron {
            continue;
        }
        let Some(schedule) = b.schedule.as_deref() else {
            continue;
        };
        let Some(oncalendar) = cron_to_oncalendar(schedule) else {
            continue; // caller warns
        };
        // The .service has no [Install] section: it is activated by the
        // paired .timer (systemd matches same-prefix units), not enabled directly.
        let service = format!(
            "[Unit]\n\
             Description=tau trigger '{name}' (agent {agent})\n\n\
             [Service]\n\
             Type=oneshot\n\
             ExecStart=tau run --bundle {artifact} --agent {agent}\n",
            name = b.name,
            agent = b.agent.0,
            artifact = artifact_ref,
        );
        let timer = format!(
            "[Unit]\n\
             Description=tau trigger '{name}' schedule ({schedule})\n\n\
             [Timer]\n\
             OnCalendar={oncalendar}\n\
             Persistent=true\n\n\
             [Install]\n\
             WantedBy=timers.target\n",
            name = b.name,
            schedule = schedule,
            oncalendar = oncalendar,
        );
        out.push((format!("tau-{}.service", b.name), service));
        out.push((format!("tau-{}.timer", b.name), timer));
    }
    out
}

/// Emit a k8s `CronJob` manifest for each **cron** trigger. k8s consumes
/// 5-field cron verbatim, so every cron trigger emits exactly one manifest.
/// Manual triggers produce no output.
pub fn emit_k8s(bindings: &[TriggerBinding], artifact_ref: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for b in bindings {
        if b.kind != TriggerKind::Cron {
            continue;
        }
        let Some(schedule) = b.schedule.as_deref() else {
            continue;
        };
        // `\x20` is a literal space: the `\`-newline string continuation
        // strips leading source indentation, so YAML nesting spaces are
        // written explicitly here.
        let manifest = format!(
            "apiVersion: batch/v1\n\
             kind: CronJob\n\
             metadata:\n\
             \x20\x20name: tau-{name}\n\
             spec:\n\
             \x20\x20schedule: \"{schedule}\"\n\
             \x20\x20jobTemplate:\n\
             \x20\x20\x20\x20spec:\n\
             \x20\x20\x20\x20\x20\x20template:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20spec:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20restartPolicy: Never\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20containers:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- name: tau\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20image: tau:latest\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20args: [\"run\", \"--bundle\", \"{artifact}\", \"--agent\", \"{agent}\"]\n",
            name = b.name,
            schedule = schedule,
            artifact = artifact_ref,
            agent = b.agent.0,
        );
        out.push((format!("tau-{}.cronjob.yaml", b.name), manifest));
    }
    out
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

    #[test]
    fn k8s_emits_cronjob_with_verbatim_schedule() {
        let bindings = alloc::vec![cron_binding()];
        let out = emit_k8s(&bindings, "/srv/app.tau");
        assert_eq!(out.len(), 1);
        let (fname, content) = &out[0];
        assert!(fname.ends_with("nightly.cronjob.yaml"), "got {fname}");
        assert!(content.contains("kind: CronJob"), "got {content}");
        assert!(content.contains("schedule: \"0 3 * * *\""), "got {content}");
        assert!(content.contains("summarizer"), "got {content}");
        assert!(
            content.contains("/srv/app.tau"),
            "artifact ref must appear in args: {content}"
        );
    }

    #[test]
    fn systemd_emits_timer_and_service_for_simple_cron() {
        let bindings = alloc::vec![cron_binding()];
        let out = emit_systemd(&bindings, "/srv/app.tau");
        assert_eq!(out.len(), 2);
        let names: alloc::vec::Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.iter().any(|n| n.ends_with("nightly.service")),
            "got {names:?}"
        );
        assert!(
            names.iter().any(|n| n.ends_with("nightly.timer")),
            "got {names:?}"
        );
        let timer = &out.iter().find(|(n, _)| n.ends_with(".timer")).unwrap().1;
        assert!(timer.contains("OnCalendar=*-*-* 03:00:00"), "got {timer}");
        let service = &out.iter().find(|(n, _)| n.ends_with(".service")).unwrap().1;
        assert!(
            service.contains("ExecStart=tau run --bundle /srv/app.tau --agent summarizer"),
            "got {service}"
        );
    }

    #[test]
    fn manual_trigger_emits_nothing() {
        let bindings = alloc::vec![TriggerBinding {
            name: "m".into(),
            kind: TriggerKind::Manual,
            agent: AgentId("a".into()),
            schedule: None,
            timezone: None,
            retry: None,
        }];
        assert!(emit_systemd(&bindings, "/srv/app.tau").is_empty());
        assert!(emit_k8s(&bindings, "/srv/app.tau").is_empty());
    }

    #[test]
    fn systemd_skips_unconvertible_cron() {
        let bindings = alloc::vec![TriggerBinding {
            name: "fast".into(),
            kind: TriggerKind::Cron,
            agent: AgentId("a".into()),
            schedule: Some("*/5 * * * *".into()),
            timezone: Some("UTC".into()),
            retry: None,
        }];
        // systemd skips it (returns empty); k8s still emits it verbatim.
        assert!(emit_systemd(&bindings, "/srv/app.tau").is_empty());
        assert_eq!(emit_k8s(&bindings, "/srv/app.tau").len(), 1);
    }

    #[test]
    fn cron_to_oncalendar_handles_dom_month_dow() {
        // "30 4 1 6 *" → minute 30, hour 04, dom 01, month 06, any dow.
        assert_eq!(
            cron_to_oncalendar("30 4 1 6 *").as_deref(),
            Some("*-06-01 04:30:00")
        );
        // dow Monday (1), all-* date → "Mon *-*-* HH:MM:SS"
        assert_eq!(
            cron_to_oncalendar("0 9 * * 1").as_deref(),
            Some("Mon *-*-* 09:00:00")
        );
        // unsupported step form → None
        assert_eq!(cron_to_oncalendar("*/5 * * * *"), None);
        // simple daily → "*-*-* 03:00:00"
        assert_eq!(
            cron_to_oncalendar("0 3 * * *").as_deref(),
            Some("*-*-* 03:00:00")
        );
        assert_eq!(
            cron_to_oncalendar("0 0 * * 0").as_deref(),
            Some("Sun *-*-* 00:00:00")
        );
        assert_eq!(
            cron_to_oncalendar("0 0 * * 7").as_deref(),
            Some("Sun *-*-* 00:00:00")
        );
    }

    #[test]
    fn emits_only_cron_from_mixed_bindings() {
        let bindings = alloc::vec![
            cron_binding(),
            TriggerBinding {
                name: "m".into(),
                kind: TriggerKind::Manual,
                agent: AgentId("a".into()),
                schedule: None,
                timezone: None,
                retry: None,
            },
            TriggerBinding {
                name: "evening".into(),
                kind: TriggerKind::Cron,
                agent: AgentId("b".into()),
                schedule: Some("0 18 * * *".into()),
                timezone: Some("UTC".into()),
                retry: None,
            },
        ];
        // 2 cron triggers → k8s 2 files, systemd 4 files; the manual is ignored.
        assert_eq!(emit_k8s(&bindings, "/srv/app.tau").len(), 2);
        assert_eq!(emit_systemd(&bindings, "/srv/app.tau").len(), 4);
    }
}
