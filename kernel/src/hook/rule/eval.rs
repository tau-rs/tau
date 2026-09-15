//! The evaluator: one pass over the predicate, no allocation until the
//! verdict is built, total.

use super::{Atom, Cmp, Fact, Predicate, Rule, RuleVerdict, Target, Value};
use crate::abi::{MsgKind, Name};
use crate::hook::{HookEvent, HookPoint, Verdict};

/// What a fact is worth at one event.
#[derive(Clone, Copy)]
enum Worth<'a> {
    Int(u64),
    Driver(&'a Name),
    MsgKind(MsgKind),
}

pub(super) fn evaluate(rule: &Rule, event: &HookEvent) -> Verdict {
    if !at_point(&rule.point, event) {
        return Verdict::Allow;
    }
    let holds = rule
        .predicate
        .as_ref()
        .is_none_or(|predicate| predicate.holds(event));
    if !holds {
        return Verdict::Allow;
    }
    match &rule.verdict {
        RuleVerdict::Allow => Verdict::Allow,
        RuleVerdict::Deny(reason) => Verdict::Deny(reason.clone()),
        RuleVerdict::Emit { to, note } => {
            let to = match to {
                Target::Subject => Some(event.subject()),
                Target::Parent => parent(event),
            };
            to.map_or(Verdict::Allow, |to| Verdict::Emit {
                to,
                payload: note.as_bytes().to_vec(),
            })
        }
    }
}

/// Whether the event is one the rule's point fires. `attach` refuses a
/// rule at any other point; this keeps `evaluate` total regardless.
fn at_point(point: &HookPoint, event: &HookEvent) -> bool {
    match (point, event) {
        (HookPoint::PreSend, HookEvent::PreSend { .. })
        | (HookPoint::PreDeliver, HookEvent::PreDeliver { .. })
        | (HookPoint::OnSpawn, HookEvent::OnSpawn { .. })
        | (HookPoint::OnExit, HookEvent::OnExit { .. }) => true,
        (
            HookPoint::OnBudget { dim, below },
            HookEvent::OnBudget {
                dim: ev_dim,
                below: ev_below,
                ..
            },
        ) => dim == ev_dim && below == ev_below,
        _ => false,
    }
}

fn parent(event: &HookEvent) -> Option<crate::abi::AgentId> {
    match event {
        HookEvent::PreSend { parent, .. }
        | HookEvent::PreDeliver { parent, .. }
        | HookEvent::OnSpawn { parent, .. }
        | HookEvent::OnExit { parent, .. }
        | HookEvent::OnBudget { parent, .. } => *parent,
    }
}

fn payload(event: &HookEvent) -> Option<&[u8]> {
    match event {
        HookEvent::PreSend { payload, .. } | HookEvent::PreDeliver { payload, .. } => {
            Some(payload.as_slice())
        }
        _ => None,
    }
}

impl Predicate {
    /// Recursion is bounded by the nesting limit the parser enforced.
    fn holds(&self, event: &HookEvent) -> bool {
        match self {
            Self::Or(l, r) => l.holds(event) || r.holds(event),
            Self::And(l, r) => l.holds(event) && r.holds(event),
            Self::Not(p) => !p.holds(event),
            Self::Atom(atom) => atom.holds(event),
        }
    }
}

impl Atom {
    fn holds(&self, event: &HookEvent) -> bool {
        match self {
            Self::Cmp { fact, cmp, value } => fact
                .worth(event)
                .is_some_and(|worth| compare(worth, *cmp, value)),
            Self::In { fact, values } => fact
                .worth(event)
                .is_some_and(|worth| values.iter().any(|v| compare(worth, Cmp::Eq, v))),
            Self::Contains(needle) => payload(event).is_some_and(|hay| contains(hay, needle)),
            Self::StartsWith(prefix) => {
                payload(event).is_some_and(|hay| hay.starts_with(prefix.as_bytes()))
            }
        }
    }
}

/// One linear scan. An empty needle is in every payload.
fn contains(hay: &[u8], needle: &str) -> bool {
    let needle = needle.as_bytes();
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

/// A comparison across types is false; the parser does not produce one.
fn compare(worth: Worth<'_>, cmp: Cmp, value: &Value) -> bool {
    match (worth, value) {
        (Worth::Int(l), Value::Int(r)) => cmp.holds(&l, r),
        (Worth::Driver(l), Value::Driver(r)) => cmp.holds(l, r),
        (Worth::MsgKind(l), Value::MsgKind(r)) => cmp.holds(&l, r),
        _ => false,
    }
}

impl Fact {
    /// The fact at this event, or `None` where the event lacks it: a
    /// dimension the subject holds no grant on, or a fact the point does
    /// not carry, which the parser refused.
    fn worth<'a>(&self, event: &'a HookEvent) -> Option<Worth<'a>> {
        match (self, event) {
            (
                Self::Depth,
                HookEvent::PreSend { depth, .. }
                | HookEvent::PreDeliver { depth, .. }
                | HookEvent::OnSpawn { depth, .. }
                | HookEvent::OnExit { depth, .. }
                | HookEvent::OnBudget { depth, .. },
            ) => Some(Worth::Int(*depth)),
            (
                Self::Driver,
                HookEvent::PreSend { driver, .. } | HookEvent::PreDeliver { driver, .. },
            ) => Some(Worth::Driver(driver.name())),
            (Self::Kind, HookEvent::PreDeliver { kind, .. }) => Some(Worth::MsgKind(*kind)),
            (Self::PayloadLen, _) => payload(event).map(|p| Worth::Int(p.len() as u64)),
            (
                Self::Remaining(dim),
                HookEvent::PreSend { remaining, .. }
                | HookEvent::PreDeliver { remaining, .. }
                | HookEvent::OnSpawn { remaining, .. }
                | HookEvent::OnBudget { remaining, .. },
            ) => remaining.get(dim).map(Worth::Int),
            _ => None,
        }
    }
}
