//! The load-bearing invariant for slice 1: adding `triggers` to `IrModule`
//! must NOT change the canonical bytes (and thus the content hash) of any
//! trigger-less module. A trigger-bearing module must hash differently and
//! must NOT bump `ir_format` (Option B): it stays v1.0.0.

use tau_ir::trigger::{Backoff, BackoffStrategy, RetryPolicy, TriggerBinding, TriggerKind};
use tau_ir::{compute_hash, to_canonical_bytes, AgentId, IrFormatVersion, IrModule, Workflow};
use tau_ports::target::registry;

fn target() -> tau_ports::target::TargetTriple {
    registry::list_available().next().unwrap().triple
}

fn trigger_less() -> IrModule {
    IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: "0.0.0".into(),
        target: target(),
        workflow: Workflow::default(),
        triggers: Vec::new(),
    }
}

#[test]
fn trigger_less_module_emits_no_triggers_key() {
    let bytes = to_canonical_bytes(&trigger_less());
    let json = core::str::from_utf8(&bytes).unwrap();
    assert!(
        !json.contains("triggers"),
        "trigger-less module must not emit a `triggers` key: {json}"
    );
}

#[test]
fn trigger_less_hash_is_round_trip_stable() {
    let m = trigger_less();
    let h1 = compute_hash(&m);
    let bytes = to_canonical_bytes(&m);
    let m2 = tau_ir::from_canonical_bytes(&bytes).unwrap();
    let h2 = compute_hash(&m2);
    assert_eq!(h1, h2, "round-trip changed the hash");
}

#[test]
fn trigger_bearing_module_changes_hash_but_keeps_ir_format() {
    let mut m = trigger_less();
    let baseline = compute_hash(&m);
    // Option B: ir_format is NOT bumped — it stays v1.0.0. The appended
    // `triggers` array is what differentiates the hash.
    m.triggers = vec![TriggerBinding {
        name: "nightly".into(),
        kind: TriggerKind::Cron,
        agent: AgentId("summarizer".into()),
        schedule: Some("0 3 * * *".into()),
        timezone: Some("UTC".into()),
        retry: Some(RetryPolicy {
            max_attempts: 3,
            backoff: Backoff {
                strategy: BackoffStrategy::Exponential,
                base: "30s".into(),
                max: "10m".into(),
            },
            dead_letter: Some("dlq-sink".into()),
        }),
    }];
    let with_trigger = compute_hash(&m);
    assert_ne!(baseline, with_trigger, "triggers must change the hash");
    assert_eq!(
        m.ir_format.0,
        IrFormatVersion::CURRENT,
        "Option B: ir_format must NOT bump for a trigger-bearing module"
    );
    let bytes = to_canonical_bytes(&m);
    let back = tau_ir::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(m, back);
}
