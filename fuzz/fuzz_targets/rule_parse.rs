//! `Rule::parse` over arbitrary bytes.
//!
//! Rule text crosses a boundary too, from the operator's configuration into
//! the kernel, and a parser that panics on hostile input faults the kernel
//! at boot (ADR-0008 §5). Any bytes must yield a `Rule` or a `RuleError`;
//! a `Rule` that comes out must print as its own source and parse back to
//! itself, and must answer every point's event without panicking.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_kernel::abi::{AgentId, Budget, Corr, DimKey, DriverId, MsgKind, Name, Namespace, Seq};
use tau_kernel::hook::{HookEvent, Rule};
use tau_kernel::reducer::Outcome;

fn events() -> Option<[HookEvent; 6]> {
    let shell = DriverId::new(Name::new("shell").ok()?);
    let remaining = Budget::from_dims([(DimKey::Tokens, 900), (DimKey::Calls, 3)]);
    let payload = b"bash: /etc/shadow: Permission denied".to_vec();
    Some([
        HookEvent::PreSend {
            seq: Seq::new(1),
            subject: AgentId::new(7),
            parent: Some(AgentId::new(1)),
            depth: 1,
            driver: shell.clone(),
            corr: Corr::new(0),
            payload: payload.clone(),
            remaining: remaining.clone(),
        },
        HookEvent::PreDeliver {
            seq: Seq::new(2),
            subject: AgentId::new(7),
            parent: None,
            depth: 0,
            driver: shell,
            corr: Corr::new(0),
            kind: MsgKind::Partial,
            payload,
            remaining: remaining.clone(),
        },
        HookEvent::OnSpawn {
            seq: Seq::new(3),
            subject: AgentId::new(8),
            parent: Some(AgentId::new(7)),
            depth: 2,
            ns: Namespace::empty(),
            remaining: remaining.clone(),
        },
        HookEvent::OnExit {
            seq: Seq::new(4),
            subject: AgentId::new(8),
            parent: Some(AgentId::new(7)),
            depth: 2,
            outcome: Outcome::Aborted,
            result: None,
            unspent: Budget::empty(),
        },
        HookEvent::OnBudget {
            seq: Seq::new(5),
            subject: AgentId::new(7),
            parent: Some(AgentId::new(1)),
            depth: 1,
            dim: DimKey::Tokens,
            below: 1000,
            remaining: remaining.clone(),
        },
        HookEvent::OnBudget {
            seq: Seq::new(6),
            subject: AgentId::new(7),
            parent: None,
            depth: 0,
            dim: DimKey::Calls,
            below: 5,
            remaining,
        },
    ])
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(rule) = Rule::parse(text) else {
        return;
    };
    let canonical = rule.to_string();
    assert_eq!(rule.source(), canonical, "source is the canonical text");
    assert_eq!(
        Rule::parse(&canonical).as_ref(),
        Ok(&rule),
        "rule did not survive a round trip"
    );
    if let Some(events) = events() {
        for event in &events {
            let _ = rule.evaluate(event);
        }
    }
});
