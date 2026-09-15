//! The Rule language (ADR-0008 §5): every example in the ADR parses,
//! evaluates as the ADR says against a hand-built event, and round-trips
//! through `Display`; every row of the "cannot say" table is a parse error
//! that names its home; a fact a point does not carry, a `deny` where none
//! is admitted, and a value of the wrong type are refused at parse.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use tau_kernel::abi::{AgentId, Budget, Corr, DimKey, DriverId, MsgKind, Name, Namespace, Seq};
use tau_kernel::hook::rule::{Wish, MAX_NESTING};
use tau_kernel::hook::{HookEvent, HookPoint, Rule, RuleError, Verdict};
use tau_kernel::reducer::Outcome;

fn name(s: &str) -> Name {
    Name::new(s).unwrap()
}

fn driver(s: &str) -> DriverId {
    DriverId::new(name(s))
}

fn tokens(n: u64) -> Budget {
    Budget::from_dims([(DimKey::Tokens, n)])
}

fn pre_send(driver_name: &str, depth: u64, payload: &[u8]) -> HookEvent {
    HookEvent::PreSend {
        seq: Seq::new(41),
        subject: AgentId::new(7),
        parent: Some(AgentId::new(1)),
        depth,
        driver: driver(driver_name),
        corr: Corr::new(3),
        payload: payload.to_vec(),
        remaining: tokens(500),
    }
}

fn pre_deliver(driver_name: &str, kind: MsgKind, payload: &[u8], parent: Option<u64>) -> HookEvent {
    HookEvent::PreDeliver {
        seq: Seq::new(42),
        subject: AgentId::new(7),
        parent: parent.map(AgentId::new),
        depth: 1,
        driver: driver(driver_name),
        corr: Corr::new(3),
        kind,
        payload: payload.to_vec(),
        remaining: tokens(500),
    }
}

fn on_spawn(depth: u64) -> HookEvent {
    HookEvent::OnSpawn {
        seq: Seq::new(43),
        subject: AgentId::new(8),
        parent: Some(AgentId::new(7)),
        depth,
        ns: Namespace::empty(),
        remaining: tokens(100),
    }
}

fn on_exit(depth: u64) -> HookEvent {
    HookEvent::OnExit {
        seq: Seq::new(44),
        subject: AgentId::new(8),
        parent: Some(AgentId::new(7)),
        depth,
        outcome: Outcome::Aborted,
        result: None,
        unspent: tokens(0),
    }
}

fn on_budget(dim: DimKey, below: u64, remaining: Budget) -> HookEvent {
    HookEvent::OnBudget {
        seq: Seq::new(45),
        subject: AgentId::new(7),
        parent: Some(AgentId::new(1)),
        depth: 1,
        dim,
        below,
        remaining,
    }
}

fn parse(text: &str) -> Rule {
    Rule::parse(text).unwrap_or_else(|e| panic!("{text:?}: {e}"))
}

fn err(text: &str) -> RuleError {
    match Rule::parse(text) {
        Ok(rule) => panic!("{text:?} parsed to {rule}"),
        Err(e) => e,
    }
}

fn deny(reason: &str) -> Verdict {
    Verdict::Deny(reason.to_owned())
}

fn emit(to: u64, note: &str) -> Verdict {
    Verdict::Emit {
        to: AgentId::new(to),
        payload: note.as_bytes().to_vec(),
    }
}

/// The oracle the fuzz target checks: the canonical text parses back to
/// the same rule, and is what `source` records.
fn round_trips(rule: &Rule) {
    let text = rule.to_string();
    assert_eq!(Rule::parse(&text).as_ref(), Ok(rule), "{text}");
    assert_eq!(rule.source(), text);
}

// --- the ADR's examples, one by one ------------------------------------

#[test]
fn shell_needs_depth_2() {
    let rule =
        parse("when pre_send if driver == shell and depth < 2 then deny \"shell needs depth 2\"");
    assert_eq!(rule.point(), &HookPoint::PreSend);
    assert_eq!(
        rule.evaluate(&pre_send("shell", 1, b"rm -rf /")),
        deny("shell needs depth 2")
    );
    assert_eq!(rule.evaluate(&pre_send("shell", 2, b"ls")), Verdict::Allow);
    assert_eq!(rule.evaluate(&pre_send("tool", 1, b"ls")), Verdict::Allow);
    round_trips(&rule);
}

#[test]
fn over_1_mib() {
    let rule = parse("when pre_send if payload.len > 1048576 then deny \"over 1 MiB\"");
    let big = vec![b'x'; 1_048_577];
    assert_eq!(
        rule.evaluate(&pre_send("tool", 1, &big)),
        deny("over 1 MiB")
    );
    assert_eq!(
        rule.evaluate(&pre_send("tool", 1, &big[..1_048_576])),
        Verdict::Allow
    );
    round_trips(&rule);
}

#[test]
fn child_hit_a_permission_wall() {
    let rule = parse(
        "when pre_deliver if driver == shell and payload contains \"Permission denied\" then emit to parent \"child hit a permission wall\"",
    );
    assert_eq!(rule.point(), &HookPoint::PreDeliver);
    let hit = b"bash: /etc/shadow: Permission denied";
    assert_eq!(
        rule.evaluate(&pre_deliver("shell", MsgKind::Reply, hit, Some(1))),
        emit(1, "child hit a permission wall")
    );
    assert_eq!(
        rule.evaluate(&pre_deliver("shell", MsgKind::Partial, hit, Some(1))),
        emit(1, "child hit a permission wall"),
        "a partial is driver traffic too"
    );
    assert_eq!(
        rule.evaluate(&pre_deliver("shell", MsgKind::Reply, b"ok", Some(1))),
        Verdict::Allow
    );
    assert_eq!(
        rule.evaluate(&pre_deliver("model", MsgKind::Reply, hit, Some(1))),
        Verdict::Allow
    );
    assert_eq!(
        rule.evaluate(&pre_deliver("shell", MsgKind::Reply, hit, None)),
        Verdict::Allow,
        "the root has no parent; the note has nowhere to go"
    );
    round_trips(&rule);
}

#[test]
fn tokens_low() {
    let rule = parse("when on_budget(tokens, 1000) then emit \"tokens low\"");
    assert_eq!(
        rule.point(),
        &HookPoint::OnBudget {
            dim: DimKey::Tokens,
            below: 1000
        }
    );
    assert_eq!(
        rule.evaluate(&on_budget(DimKey::Tokens, 1000, tokens(1200))),
        emit(7, "tokens low"),
        "no target: the subject"
    );
    assert_eq!(
        rule.evaluate(&on_budget(DimKey::Tokens, 500, tokens(600))),
        Verdict::Allow,
        "another line is another point"
    );
    round_trips(&rule);
}

#[test]
fn tree_too_deep() {
    let rule = parse("when on_spawn if depth > 6 then deny \"tree too deep\"");
    assert_eq!(rule.evaluate(&on_spawn(7)), deny("tree too deep"));
    assert_eq!(rule.evaluate(&on_spawn(6)), Verdict::Allow);
    round_trips(&rule);
}

// --- the rest of the grammar -------------------------------------------

#[test]
fn every_shape_round_trips_to_its_canonical_text() {
    let cases = [
        ("when pre_send then allow", "when pre_send then allow"),
        (
            "  when   pre_send   if   depth==001  then   deny   \"x\"  ",
            "when pre_send if depth == 1 then deny \"x\"",
        ),
        (
            "when pre_send if not not depth == 1 then allow",
            "when pre_send if depth == 1 then allow",
        ),
        (
            "when pre_send if not (depth == 1 or depth == 2) then allow",
            "when pre_send if not (depth == 1 or depth == 2) then allow",
        ),
        (
            "when pre_send if depth == 1 or depth == 2 and depth == 3 then allow",
            "when pre_send if depth == 1 or depth == 2 and depth == 3 then allow",
        ),
        (
            "when pre_send if (depth == 1 or depth == 2) and depth == 3 then allow",
            "when pre_send if (depth == 1 or depth == 2) and depth == 3 then allow",
        ),
        (
            "when pre_send if ((((depth == 1)))) then allow",
            "when pre_send if depth == 1 then allow",
        ),
        (
            "when pre_send if driver in {shell,tool} then deny \"no\"",
            "when pre_send if driver in { shell, tool } then deny \"no\"",
        ),
        (
            "when pre_deliver if kind in { reply, partial } and kind != notice then allow",
            "when pre_deliver if kind in { reply, partial } and kind != notice then allow",
        ),
        (
            "when pre_send if remaining.tokens <= 10 or remaining.my-gpu_ms >= 5 then emit to subject \"n\"",
            "when pre_send if remaining.tokens <= 10 or remaining.my-gpu_ms >= 5 then emit \"n\"",
        ),
        (
            "when pre_send if payload starts_with \"{\" then deny \"say \\\"hi\\\"\\n\\t\\\\\"",
            "when pre_send if payload starts_with \"{\" then deny \"say \\\"hi\\\"\\n\\t\\\\\"",
        ),
        (
            "when on_budget(cost_microusd,0) then emit to parent \"\"",
            "when on_budget(cost_microusd, 0) then emit to parent \"\"",
        ),
        (
            "when on_exit if depth in { 1 } then emit \"bye\"",
            "when on_exit if depth in { 1 } then emit \"bye\"",
        ),
    ];
    for (text, canonical) in cases {
        let rule = parse(text);
        assert_eq!(rule.to_string(), canonical, "{text}");
        round_trips(&rule);
        assert_eq!(text.parse::<Rule>().as_ref(), Ok(&rule));
    }
}

#[test]
fn precedence_is_or_under_and_under_not() {
    // depth == 1 or (depth == 2 and depth == 3): true at depth 1.
    let rule = parse("when on_spawn if depth == 1 or depth == 2 and depth == 3 then deny \"x\"");
    assert_eq!(rule.evaluate(&on_spawn(1)), deny("x"));
    assert_eq!(rule.evaluate(&on_spawn(2)), Verdict::Allow);
    // (depth == 1 or depth == 2) and depth == 3: never true.
    let rule = parse("when on_spawn if (depth == 1 or depth == 2) and depth == 3 then deny \"x\"");
    assert_eq!(rule.evaluate(&on_spawn(1)), Verdict::Allow);
    assert_eq!(rule.evaluate(&on_spawn(3)), Verdict::Allow);
    let rule = parse("when on_spawn if not depth == 1 and depth < 3 then deny \"x\"");
    assert_eq!(rule.evaluate(&on_spawn(2)), deny("x"));
    assert_eq!(rule.evaluate(&on_spawn(1)), Verdict::Allow);
}

#[test]
fn sets_and_kinds() {
    let rule = parse("when pre_send if driver in { shell, sandbox } then deny \"no\"");
    assert_eq!(rule.evaluate(&pre_send("sandbox", 1, b"")), deny("no"));
    assert_eq!(rule.evaluate(&pre_send("model", 1, b"")), Verdict::Allow);
    let rule = parse("when pre_deliver if kind == partial then emit \"streaming\"");
    assert_eq!(
        rule.evaluate(&pre_deliver("model", MsgKind::Partial, b"", None)),
        emit(7, "streaming")
    );
    assert_eq!(
        rule.evaluate(&pre_deliver("model", MsgKind::Reply, b"", None)),
        Verdict::Allow
    );
}

#[test]
fn payload_scans() {
    let rule = parse("when pre_send if payload starts_with \"shell\" then deny \"no\"");
    assert_eq!(rule.evaluate(&pre_send("t", 1, b"shell ls")), deny("no"));
    assert_eq!(rule.evaluate(&pre_send("t", 1, b" shell")), Verdict::Allow);
    let rule = parse("when pre_send if payload contains \"\" then deny \"no\"");
    assert_eq!(
        rule.evaluate(&pre_send("t", 1, b"")),
        deny("no"),
        "an empty needle is in every payload, the empty one included"
    );
    let rule = parse("when pre_send if payload contains \"abc\" then deny \"no\"");
    assert_eq!(rule.evaluate(&pre_send("t", 1, b"ab")), Verdict::Allow);
    assert_eq!(rule.evaluate(&pre_send("t", 1, b"xxabc")), deny("no"));
}

#[test]
fn a_dimension_the_subject_holds_no_grant_on_makes_the_atom_false() {
    let rule = parse("when on_spawn if remaining.calls < 100 then deny \"x\"");
    assert_eq!(rule.evaluate(&on_spawn(1)), Verdict::Allow);
    let rule = parse("when on_spawn if not remaining.calls < 100 then deny \"x\"");
    assert_eq!(rule.evaluate(&on_spawn(1)), deny("x"));
    let rule = parse("when on_spawn if remaining.tokens < 200 then deny \"x\"");
    assert_eq!(rule.evaluate(&on_spawn(1)), deny("x"));
}

#[test]
fn an_event_from_another_point_is_allowed() {
    // `attach` refuses the mismatch; evaluate stays total regardless.
    let rule = parse("when pre_send then deny \"x\"");
    assert_eq!(rule.evaluate(&on_exit(1)), Verdict::Allow);
}

#[test]
fn nesting_stops_at_the_limit() {
    let deep = |n: usize| {
        format!(
            "when pre_send if {}depth == 1{} then allow",
            "(".repeat(n),
            ")".repeat(n)
        )
    };
    let rule = parse(&deep(MAX_NESTING));
    assert_eq!(rule.evaluate(&pre_send("t", 1, b"")), Verdict::Allow);
    round_trips(&rule);
    assert_eq!(
        err(&deep(MAX_NESTING + 1)),
        RuleError::TooDeep { limit: MAX_NESTING }
    );
    // A `not` chain is counted, not nested.
    let nots = format!(
        "when pre_send if {}depth == 1 then deny \"x\"",
        "not ".repeat(10_001)
    );
    assert_eq!(parse(&nots).evaluate(&pre_send("t", 2, b"")), deny("x"));
}

// --- what a rule cannot say, with its home ------------------------------

#[test]
fn every_cannot_say_row_is_a_parse_error_naming_its_home() {
    let rows = [
        (
            "when pre_send if args.cmd == \"sudo\" then deny \"x\"",
            Wish::JsonField,
        ),
        (
            "when pre_send if payload.args.cmd == \"sudo\" then deny \"x\"",
            Wish::JsonField,
        ),
        (
            "when pre_send if payload[\"cmd\"] == \"sudo\" then deny \"x\"",
            Wish::JsonField,
        ),
        (
            "when pre_send if tool.name == \"bash\" then deny \"x\"",
            Wish::JsonField,
        ),
        (
            "when pre_send if payload matches \"rm -rf .*\" then deny \"x\"",
            Wish::Regex,
        ),
        (
            "when pre_send if payload ~ \"rm -rf .*\" then deny \"x\"",
            Wish::Regex,
        ),
        (
            "when pre_send if payload =~ \"rm -rf .*\" then deny \"x\"",
            Wish::Regex,
        ),
        (
            "when pre_send if count > 3 then deny \"x\"",
            Wish::AcrossEvents,
        ),
        (
            "when pre_send if rate > 3 then deny \"the third time\"",
            Wish::AcrossEvents,
        ),
        (
            "when pre_send if parent.depth > 1 then deny \"x\"",
            Wish::AnotherAgent,
        ),
        (
            "when pre_send if siblings > 4 then deny \"x\"",
            Wish::AnotherAgent,
        ),
        (
            "when pre_send if depth + 1 > 3 then deny \"x\"",
            Wish::Arithmetic,
        ),
        (
            "when pre_send if depth > 3 * 2 then deny \"x\"",
            Wish::Arithmetic,
        ),
        ("when pre_send if now > 5 then deny \"x\"", Wish::Time),
        ("when pre_send if elapsed > 5 then deny \"x\"", Wish::Time),
        (
            "when pre_send if lower(payload) contains \"sudo\" then deny \"x\"",
            Wish::StringTransform,
        ),
        ("when pre_send then rewrite \"redacted\"", Wish::Rewrite),
        ("when pre_send then redact \"secret\"", Wish::Rewrite),
    ];
    for (text, wish) in rows {
        let e = err(text);
        let RuleError::NotExpressible { wish: found, .. } = &e else {
            panic!("{text:?}: expected NotExpressible, got {e}");
        };
        assert_eq!(*found, wish, "{text}");
        let message = e.to_string();
        assert!(message.contains(wish.home()), "{text}: {message}");
        assert!(message.contains("ADR-0008 §5"), "{text}: {message}");
    }
}

#[test]
fn a_fact_the_point_does_not_carry_is_refused_at_parse() {
    let rows = [
        (
            "when on_exit if driver == shell then emit \"x\"",
            "driver",
            HookPoint::OnExit,
            "depth",
        ),
        (
            "when on_spawn if payload contains \"x\" then deny \"x\"",
            "payload",
            HookPoint::OnSpawn,
            "depth, remaining.<dim>",
        ),
        (
            "when on_spawn if payload.len > 1 then deny \"x\"",
            "payload.len",
            HookPoint::OnSpawn,
            "depth, remaining.<dim>",
        ),
        (
            "when pre_send if kind == reply then deny \"x\"",
            "kind",
            HookPoint::PreSend,
            "depth, driver, payload, remaining.<dim>",
        ),
        (
            "when on_exit if remaining.tokens < 1 then emit \"x\"",
            "remaining.tokens",
            HookPoint::OnExit,
            "depth",
        ),
        (
            "when on_budget(tokens, 5) if driver == shell then emit \"x\"",
            "driver",
            HookPoint::OnBudget {
                dim: DimKey::Tokens,
                below: 5,
            },
            "depth, remaining.<dim>",
        ),
    ];
    for (text, fact, point, carries) in rows {
        assert_eq!(
            err(text),
            RuleError::FactNotAtPoint {
                fact: fact.to_owned(),
                point,
                carries,
            },
            "{text}"
        );
    }
    // The same facts are fine where they live.
    parse("when pre_deliver if driver == shell and kind == reply and payload.len > 1 and remaining.tokens < 1 and depth > 0 then deny \"x\"");
    parse("when on_budget(tokens, 5) if remaining.calls > 1 and depth > 0 then emit \"x\"");
}

#[test]
fn deny_is_refused_where_nothing_can_be_stopped() {
    assert_eq!(
        err("when on_exit then deny \"x\""),
        RuleError::DenyNotAdmitted {
            point: HookPoint::OnExit
        }
    );
    assert_eq!(
        err("when on_budget(tokens, 1) if depth > 1 then deny \"x\""),
        RuleError::DenyNotAdmitted {
            point: HookPoint::OnBudget {
                dim: DimKey::Tokens,
                below: 1
            }
        }
    );
    parse("when on_exit then emit \"x\"");
    parse("when on_spawn then deny \"x\"");
}

#[test]
fn types_and_orders_are_checked_at_parse() {
    assert!(matches!(
        err("when pre_send if driver < shell then allow"),
        RuleError::Unordered { fact, .. } if fact == "driver"
    ));
    assert!(matches!(
        err("when pre_deliver if kind >= reply then allow"),
        RuleError::Unordered { fact, .. } if fact == "kind"
    ));
    assert!(matches!(
        err("when pre_send if depth == shell then allow"),
        RuleError::TypeMismatch { fact, expected, .. } if fact == "depth" && expected == "an integer"
    ));
    assert!(matches!(
        err("when pre_send if driver == 3 then allow"),
        RuleError::TypeMismatch { fact, .. } if fact == "driver"
    ));
    assert!(matches!(
        err("when pre_send if driver == \"shell\" then allow"),
        RuleError::TypeMismatch { fact, expected, .. } if fact == "driver" && expected == "a driver name, bare"
    ));
    assert!(matches!(
        err("when pre_deliver if kind == shell then allow"),
        RuleError::TypeMismatch { fact, .. } if fact == "kind"
    ));
    assert!(matches!(
        err("when pre_send if driver in { shell, 3 } then allow"),
        RuleError::TypeMismatch { fact, .. } if fact == "driver"
    ));
}

#[test]
fn syntax_errors_say_where_and_what() {
    assert_eq!(err(""), RuleError::Empty);
    assert_eq!(err("   \t "), RuleError::Empty);
    assert!(matches!(
        err("pre_send then allow"),
        RuleError::Unexpected {
            at: 0,
            expected: "`when`",
            ..
        }
    ));
    assert!(matches!(
        err("when pre_send then allow extra"),
        RuleError::Unexpected { at: 25, expected: "end of rule", found } if found == "`extra`"
    ));
    assert!(matches!(
        err("when pre_send if depth == 1"),
        RuleError::Unexpected { at: 27, expected: "`then`", found } if found == "end of rule"
    ));
    assert!(matches!(
        err("when on_send then allow"),
        RuleError::UnknownPoint { at: 5, found } if found == "on_send"
    ));
    assert!(matches!(
        err("when pre_send if speed > 1 then allow"),
        RuleError::UnknownFact { at: 17, found } if found == "speed"
    ));
    assert!(matches!(
        err("when pre_send then reject \"x\""),
        RuleError::UnknownVerdict { at: 19, found } if found == "reject"
    ));
    assert_eq!(
        err("when pre_send then deny \"x"),
        RuleError::UnterminatedString { at: 24 }
    );
    assert_eq!(
        err("when pre_send then deny \"\\x\""),
        RuleError::BadEscape { at: 25, found: 'x' }
    );
    assert_eq!(
        err("when pre_send if depth > 99999999999999999999 then allow"),
        RuleError::IntTooLarge { at: 25 }
    );
    assert_eq!(
        err("when pre_send if depth = 1 then allow"),
        RuleError::UnexpectedChar { at: 23, found: '=' }
    );
    assert_eq!(
        err("when pre_send if Depth == 1 then allow"),
        RuleError::UnexpectedChar { at: 17, found: 'D' }
    );
    assert_eq!(
        err("when on_budget(Tokens, 1) then allow"),
        RuleError::UnexpectedChar { at: 15, found: 'T' }
    );
    assert!(matches!(
        err(&format!("when on_budget({}, 1) then allow", "a".repeat(64))),
        RuleError::BadName { at: 15, .. }
    ));
    assert!(matches!(
        err("when pre_send if driver in { } then allow"),
        RuleError::Unexpected { at: 29, .. }
    ));
    assert!(matches!(
        err("when pre_send then emit to root \"x\""),
        RuleError::Unexpected {
            at: 27,
            expected: "`subject` or `parent`",
            ..
        }
    ));
    assert!(matches!(
        err("when pre_send if payload then allow"),
        RuleError::Unexpected { at: 25, .. }
    ));
}
