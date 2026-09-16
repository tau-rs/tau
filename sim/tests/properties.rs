//! Tier 2 item 1: the four properties, over arbitrary trees (HANDOFF §8,
//! ADR-0002). `kernel/tests/abi_invariants.rs` has each at its smallest.
//!
//! The generator is the sim's: a seed picks a random legal workload of
//! spawns, exits, cancels, sends, replies, resolutions, claims and ticks, and
//! proptest picks the seed and how much of it to run. The run is generated
//! unchecked, then folded again here from `State::initial()` one entry at a
//! time, with the property checked after every step. Because the first `k`
//! accepted entries of a seed's run are the same whatever the total, shrinking
//! the event count finds the shortest prefix of that run that breaks the
//! property; the seed itself is not shrunk, since a smaller seed is just a
//! different run. A failure prints the entries up to the offending one as
//! the log would hold them.
//!
//! Case count: `PROPTEST_CASES`, defaulting to a number that keeps every test
//! under the 5s quick ceiling. `tier2.yml` cranks it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use proptest::test_runner::Config;
use tau_kernel::abi::{AgentId, Budget};
use tau_kernel::log::{Entry, Log};
use tau_kernel::reducer::{Agent, State, Status};
use tau_sim::{conserved, run_unchecked, Options, Report};

/// The most accepted entries a case drives before the draining epilogue.
/// Enough for a few dozen spawns at depth four; the soak covers scale.
const MAX_EVENTS: u64 = 512;

/// Cases per property when `PROPTEST_CASES` is unset.
const DEFAULT_CASES: u32 = 64;

fn config() -> Config {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_CASES);
    Config {
        cases,
        ..Config::default()
    }
}

/// A seed and a length: one arbitrary tree.
fn arbitrary_run() -> impl Strategy<Value = (u64, u64)> {
    (any::<u64>().no_shrink(), 1..=MAX_EVENTS)
}

fn is_live(a: &Agent) -> bool {
    matches!(a.status, Status::Live | Status::Cancelling)
}

/// One step of the fold: the entry just applied, the state on either side of
/// it, and everything a property may need to relate it to.
struct Step<'a> {
    /// The whole log.
    log: &'a [Entry],
    /// The entry just applied.
    entry: &'a Entry,
    /// The state before it.
    before: &'a State,
    /// The state after it.
    after: &'a State,
    /// The root's grant.
    grant: &'a Budget,
    /// Whether this is the log's last entry.
    last: bool,
}

/// A property: `Ok`, or why this step broke it.
type Property = fn(&Step<'_>) -> Result<(), String>;

/// The entries up to and including `at`, as the log file would hold them.
fn transcript(log: &[Entry], at: usize) -> String {
    let mut prefix = Log::in_memory();
    for entry in log.iter().take(at.saturating_add(1)) {
        prefix.append(entry.clone()).unwrap();
    }
    let mut bytes = Vec::new();
    prefix.write_to(&mut bytes).unwrap();
    String::from_utf8(bytes).unwrap()
}

/// Generates the run, then folds it again with `property` checked after
/// every entry.
fn holds(seed: u64, events: u64, property: Property) -> Result<(), TestCaseError> {
    let options = Options {
        events,
        check_every: None,
        max_live: None,
        snapshot_every: None,
    };
    let Report { log, grant, .. } = run_unchecked(seed, &options)
        .map_err(|e| TestCaseError::fail(format!("seed {seed:#x}, {events} events: {e}")))?;
    let entries = log.entries();
    let mut state = State::initial();
    for (at, entry) in entries.iter().enumerate() {
        let before = state.clone();
        if let Err(refusal) = state.apply(entry) {
            return Err(TestCaseError::fail(format!(
                "seed {seed:#x}, {events} events: the generator's own log was refused at \
                 entry {at}: {refusal}\n{}",
                transcript(entries, at)
            )));
        }
        let step = Step {
            log: entries,
            entry,
            before: &before,
            after: &state,
            grant: &grant,
            last: at.saturating_add(1) == entries.len(),
        };
        if let Err(why) = property(&step) {
            return Err(TestCaseError::fail(format!(
                "seed {seed:#x}, {events} events: {why} (entry {at})\n{}",
                transcript(entries, at)
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- properties

/// Property 1: over the whole tree, budgets plus reservations plus spent equal
/// the root's grant plus overdraft, along every granted dimension but `depth`.
fn budgets_sum_to_the_root_grant(step: &Step<'_>) -> Result<(), String> {
    // Nothing to conserve until the root exists.
    if step.after.agents().next().is_none() {
        return Ok(());
    }
    conserved(step.after, step.grant).map_err(|e| e.to_string())
}

/// Property 2: authority only narrows going down the tree. Checked at the
/// spawn against the parent's namespace at that moment, and afterwards over
/// every parent-child edge, since a namespace is fixed at birth.
fn namespaces_narrow_down_the_tree(step: &Step<'_>) -> Result<(), String> {
    if let Entry::Spawned {
        parent: Some(parent),
        agent,
        ns,
        ..
    } = step.entry
    {
        let held = &step
            .before
            .agent(*parent)
            .ok_or(format!(
                "{agent} was spawned under {parent}, who does not exist"
            ))?
            .ns;
        if !ns.is_subset_of(held) {
            return Err(format!(
                "{agent} was born holding capabilities its parent {parent} does not"
            ));
        }
    }
    for (id, a) in step.after.agents() {
        let Some(parent) = a.parent else { continue };
        let held = &step
            .after
            .agent(parent)
            .ok_or(format!("{id}'s parent {parent} has no record"))?
            .ns;
        if !a.ns.is_subset_of(held) {
            return Err(format!(
                "{id} holds capabilities its parent {parent} does not"
            ));
        }
    }
    Ok(())
}

/// Property 3: a cancel freezes the whole subtree at once, its deadline is
/// enforced by the clock, and a finished agent holds nothing — no
/// reservation, no open request, no mailbox — whichever way it finished.
fn cancel_leaves_no_live_descendants_and_no_leaked_reservations(
    step: &Step<'_>,
) -> Result<(), String> {
    if let Entry::Cancelled { agent, .. } = step.entry {
        for id in step.after.subtree(*agent) {
            let a = step
                .after
                .agent(id)
                .ok_or(format!("{id} is under {agent} but has no record"))?;
            if a.status == Status::Live {
                return Err(format!(
                    "{id} is still live after its ancestor {agent} was cancelled"
                ));
            }
        }
    }
    let now = step.after.now();
    for (id, a) in step.after.agents() {
        match a.status {
            Status::Live => {
                if a.deadline.is_some() {
                    return Err(format!("{id} is live but carries a deadline"));
                }
            }
            Status::Cancelling => {
                let Some(deadline) = a.deadline else {
                    return Err(format!("{id} is cancelling without a deadline"));
                };
                if deadline <= now {
                    return Err(format!(
                        "{id} is past its deadline ({deadline} <= {now}) and not aborted"
                    ));
                }
            }
            Status::Exited | Status::Aborted => {
                if !a.reserved.is_empty() {
                    return Err(format!("{id} finished holding a reservation"));
                }
                if step.after.open_corrs(id).next().is_some() {
                    return Err(format!("{id} finished with a request still open"));
                }
                if !a.mailbox.is_empty() {
                    return Err(format!("{id} finished with a mailbox"));
                }
                if a.deadline.is_some() {
                    return Err(format!("{id} finished carrying a deadline"));
                }
            }
        }
    }
    if step.last {
        // The epilogue cancels the root with no grace: nothing survives it.
        if !step.after.is_drained() {
            let live: Vec<AgentId> = step
                .after
                .agents()
                .filter(|(_, a)| is_live(a))
                .map(|(id, _)| id)
                .collect();
            return Err(format!("the root was cancelled and {live:?} outlived it"));
        }
    }
    Ok(())
}

/// Property 4: a `recv` resolves to a message that exists — an earlier entry
/// of this log, delivered to this agent and still in its mailbox — and to
/// nothing else; the resolution takes exactly that message out.
fn every_recv_resolution_references_an_existing_entry(step: &Step<'_>) -> Result<(), String> {
    let Entry::Resolved {
        seq,
        agent,
        matched,
        ..
    } = step.entry
    else {
        return Ok(());
    };
    if matched >= seq {
        return Err(format!(
            "{agent} resolved {matched}, which is not before {seq}"
        ));
    }
    let position = usize::try_from(matched.get()).map_err(|e| e.to_string())?;
    match step.log.get(position) {
        Some(origin) if origin.seq() == *matched => {}
        Some(origin) => {
            return Err(format!(
                "entry {position} is at {}, not {matched}: the log is not dense",
                origin.seq()
            ))
        }
        None => {
            return Err(format!(
                "{agent} resolved {matched}, which is not in the log"
            ))
        }
    }
    let held = |state: &State| {
        state
            .agent(*agent)
            .is_some_and(|a| a.mailbox.iter().any(|m| m.seq == *matched))
    };
    if !held(step.before) {
        return Err(format!(
            "{agent} resolved {matched}, which was not in its mailbox"
        ));
    }
    if held(step.after) {
        return Err(format!("{agent} resolved {matched} and still holds it"));
    }
    Ok(())
}

// --------------------------------------------------------------------- tests

proptest! {
    #![proptest_config(config())]

    #[test]
    fn budgets_sum_to_the_root_grant_over_any_tree((seed, events) in arbitrary_run()) {
        holds(seed, events, budgets_sum_to_the_root_grant)?;
    }

    #[test]
    fn namespaces_narrow_down_any_tree((seed, events) in arbitrary_run()) {
        holds(seed, events, namespaces_narrow_down_the_tree)?;
    }

    #[test]
    fn cancel_leaves_no_live_descendants_and_no_leaked_reservations_in_any_tree(
        (seed, events) in arbitrary_run(),
    ) {
        holds(seed, events, cancel_leaves_no_live_descendants_and_no_leaked_reservations)?;
    }

    #[test]
    fn every_recv_resolution_references_an_existing_entry_in_any_tree(
        (seed, events) in arbitrary_run(),
    ) {
        holds(seed, events, every_recv_resolution_references_an_existing_entry)?;
    }
}

/// The properties are only worth something if a broken log fails them: a
/// resolution of a message that was never delivered must be caught, and the
/// failure must carry the transcript.
#[test]
fn a_forged_resolution_is_caught_and_the_failure_prints_the_log() {
    let report = run_unchecked(
        1,
        &Options {
            events: 64,
            check_every: None,
            max_live: None,
            snapshot_every: None,
        },
    )
    .unwrap();
    let entries = report.log.entries();
    let mut state = State::initial();
    let mut forged = None;
    for entry in entries {
        let before = state.clone();
        state.apply(entry).unwrap();
        if let Entry::Resolved { seq, agent, .. } = entry {
            // Point the resolution at the log's first entry, which is a
            // driver registration and was never in anyone's mailbox.
            let fake = Entry::Resolved {
                seq: *seq,
                agent: *agent,
                matched: entries.first().unwrap().seq(),
            };
            let step = Step {
                log: entries,
                entry: &fake,
                before: &before,
                after: &state,
                grant: &report.grant,
                last: false,
            };
            forged = Some(every_recv_resolution_references_an_existing_entry(&step));
            break;
        }
    }
    let verdict = forged.expect("seed 1 resolves at least one message in 64 events");
    assert!(verdict.is_err(), "the forged resolution passed");

    let transcript = transcript(entries, 3);
    assert_eq!(transcript.lines().count(), 5, "header plus four entries");
    assert!(transcript.contains("\"entry\":\"driver_registered\""));
}
