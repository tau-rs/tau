//! ADR-0008's obligation on this crate, the Rule half: boot attaches one or
//! two seeded rules beside the natives, and every roll call a rule appears
//! in carries what [`Rule::evaluate`] said about the event the kernel would
//! have built — so the evaluator sits under the soak and the second-platform
//! refold, not only under its unit tests and the fuzz target.
//!
//! The rulings themselves are not re-derived here: the log holds digests of
//! payloads the run never wrote down, so an outside fold cannot rebuild the
//! event. What is checked is what the log alone can say — a rule is
//! attached at the point it is written for, a rule never fails, and across
//! the Tier 1 seeds the rules reach every verdict they can produce.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use tau_kernel::abi::HookId;
use tau_kernel::hook::{HookPoint, HookSource, Rule, Ruling};
use tau_kernel::log::{Entry, Log};
use tau_sim::run;

/// The Tier 1 seeds, at a size that keeps each run well under a second.
const SEEDS: [u64; 3] = [
    0x7a75_0000_0000_0001,
    0x7a75_0000_0000_0002,
    0x7a75_0000_0000_0003,
];
const EVENTS: u64 = 2_000;

/// Every rule the log attached: its id, the point, and the source text.
fn attached_rules(log: &Log) -> BTreeMap<HookId, (HookPoint, String)> {
    log.entries()
        .iter()
        .filter_map(|e| match e {
            Entry::Attached {
                hook,
                point,
                program: HookSource::Rule(text),
                ..
            } => Some((*hook, (point.clone(), text.clone()))),
            _ => None,
        })
        .collect()
}

/// Every ruling a rule gave, across the log.
fn rule_rulings(log: &Log) -> Vec<(HookId, Ruling)> {
    let rules = attached_rules(log);
    log.entries()
        .iter()
        .filter_map(|e| match e {
            Entry::Verdicts { roll, .. } => Some(roll),
            _ => None,
        })
        .flatten()
        .filter(|(hook, _)| rules.contains_key(hook))
        .cloned()
        .collect()
}

#[test]
fn every_seed_attaches_a_rule_at_the_point_it_is_written_for() {
    for seed in SEEDS {
        let report = run(seed, EVENTS).unwrap();
        let rules = attached_rules(&report.log);
        assert!(
            (1..=2).contains(&rules.len()),
            "seed {seed:#x}: {} rules attached, expected one or two",
            rules.len()
        );
        for (hook, (point, text)) in &rules {
            let rule = Rule::parse(text)
                .unwrap_or_else(|e| panic!("seed {seed:#x}: {hook} holds unparsable text: {e}"));
            assert_eq!(
                rule.source(),
                text,
                "seed {seed:#x}: {hook} was recorded in non-canonical text"
            );
            assert_eq!(
                rule.point(),
                point,
                "seed {seed:#x}: {hook} is written for one point and attached at another"
            );
        }
    }
}

#[test]
fn a_rule_is_consulted_and_never_fails() {
    for seed in SEEDS {
        let report = run(seed, EVENTS).unwrap();
        let rulings = rule_rulings(&report.log);
        assert!(
            !rulings.is_empty(),
            "seed {seed:#x}: no roll call names a rule"
        );
        for (hook, ruling) in rulings {
            assert!(
                !matches!(ruling, Ruling::Failed { .. }),
                "seed {seed:#x}: {hook} is a rule and rules cannot fail, yet: {ruling:?}"
            );
        }
    }
}

#[test]
fn across_the_seeds_the_rules_allow_deny_and_emit() {
    let mut allowed = false;
    let mut denied = false;
    let mut emitted = false;
    for seed in SEEDS {
        let report = run(seed, EVENTS).unwrap();
        for (_, ruling) in rule_rulings(&report.log) {
            match ruling {
                Ruling::Allow => allowed = true,
                Ruling::Deny(_) => denied = true,
                Ruling::Emit { .. } => emitted = true,
                Ruling::Failed { .. } => {}
            }
        }
    }
    assert!(allowed, "no rule ever allowed");
    assert!(
        denied,
        "no rule ever denied: the pool's predicates never held"
    );
    assert!(
        emitted,
        "no rule ever emitted: the pool's predicates never held"
    );
}

#[test]
fn rules_are_installed_in_seeded_order_among_the_natives() {
    // The order is a draw, so across the seeds a rule lands both before and
    // after a native at its own point; a fixed "natives first" would not.
    let mut before_a_native = false;
    let mut after_a_native = false;
    for seed in SEEDS {
        let report = run(seed, EVENTS).unwrap();
        let rules = attached_rules(&report.log);
        for e in report.log.entries() {
            let Entry::Attached {
                hook,
                point,
                program: HookSource::Native(_),
                ..
            } = e
            else {
                continue;
            };
            for (rule, (rule_point, _)) in &rules {
                if rule_point != point {
                    continue;
                }
                if rule < hook {
                    before_a_native = true;
                } else {
                    after_a_native = true;
                }
            }
        }
    }
    assert!(
        before_a_native,
        "a rule never preceded a native at its point"
    );
    assert!(
        after_a_native,
        "a rule never followed a native at its point"
    );
}
