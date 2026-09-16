//! The ABI guard, part two: the invariants the types exist to carry.
//!
//! Snapshots pin the *shape*. These pin the *meaning* — the handful of
//! properties that, if they ever stopped holding, would make the frozen
//! boundary a lie regardless of how stable its serialization was.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use tau_kernel::abi::{
    AgentId, BlobRef, Budget, BudgetError, Capability, Corr, DimKey, DriverId, Endpoint, Entry,
    FailureMode, HookId, HookPoint, HookSource, LogHeader, Msg, MsgKind, Name, NameError,
    Namespace, Ruling, Seq, ABI,
};
use tau_kernel::log::{Log, LogError};
use tau_kernel::reducer::{fold, Refusal};

fn name(s: &str) -> Name {
    Name::new(s).expect("fixture name is valid")
}

// ---------------------------------------------------------------- namespaces

#[test]
fn authority_only_narrows_down_the_tree() {
    let parent = Namespace::from_caps([
        Capability::mint(1),
        Capability::mint(2),
        Capability::mint(3),
    ]);
    let child = Namespace::from_caps([Capability::mint(1), Capability::mint(3)]);
    let stranger = Namespace::from_caps([Capability::mint(1), Capability::mint(9)]);

    assert!(
        child.is_subset_of(&parent),
        "a subset namespace is a legal child"
    );
    assert!(
        !stranger.is_subset_of(&parent),
        "a capability the parent lacks cannot be granted"
    );
    assert!(
        Namespace::empty().is_subset_of(&parent),
        "an agent may be granted nothing"
    );
    assert!(!parent.is_subset_of(&child), "authority never widens");
}

// ------------------------------------------------------------------- budgets

#[test]
fn carve_is_all_or_nothing() {
    let mut parent = Budget::from_dims([(DimKey::Tokens, 100), (DimKey::Calls, 10)]);

    // The second dimension is short. If the carve were applied dimension by
    // dimension, `tokens` would already be gone by the time `calls` failed,
    // and the parent would have silently lost budget it never granted.
    let err = parent
        .carve(&Budget::from_dims([
            (DimKey::Tokens, 50),
            (DimKey::Calls, 999),
        ]))
        .unwrap_err();

    assert_eq!(
        err,
        BudgetError::Insufficient {
            dim: DimKey::Calls,
            available: 10,
            requested: 999
        }
    );
    assert_eq!(
        parent.get(&DimKey::Tokens),
        Some(100),
        "a failed carve deducts nothing"
    );
    assert_eq!(parent.get(&DimKey::Calls), Some(10));
}

#[test]
fn an_ungranted_dimension_is_not_unlimited() {
    let mut parent = Budget::from_dims([(DimKey::Tokens, 100)]);

    let err = parent
        .carve(&Budget::from_dims([(DimKey::CostMicroUsd, 1)]))
        .unwrap_err();

    assert_eq!(
        err,
        BudgetError::NoGrant {
            dim: DimKey::CostMicroUsd
        }
    );
}

#[test]
fn budget_is_conserved_across_carve_and_restore() {
    // The Tier 2 property test generalises this over arbitrary trees. Here it
    // is at its smallest: what the parent loses, the child holds, and what the
    // child returns, the parent regains — exactly.
    let mut parent = Budget::from_dims([(DimKey::Tokens, 100)]);
    let child = parent
        .carve(&Budget::from_dims([(DimKey::Tokens, 30)]))
        .unwrap();

    assert_eq!(parent.get(&DimKey::Tokens), Some(70));
    assert_eq!(child.get(&DimKey::Tokens), Some(30));

    parent.restore(&child).unwrap();
    assert_eq!(parent.get(&DimKey::Tokens), Some(100));
}

#[test]
fn restoring_twice_is_an_error_not_a_wrap() {
    let mut parent = Budget::from_dims([(DimKey::Tokens, u64::MAX)]);

    let err = parent
        .restore(&Budget::from_dims([(DimKey::Tokens, 1)]))
        .unwrap_err();

    assert_eq!(
        err,
        BudgetError::Overflow {
            dim: DimKey::Tokens
        }
    );
}

#[test]
fn a_custom_dimension_cannot_shadow_a_reserved_one() {
    // Otherwise a driver could report its spending under a `tokens` of its own
    // and detach it from the dimension the kernel enforces.
    for reserved in DimKey::reserved() {
        let parsed: DimKey = reserved.parse().expect("reserved keys parse");
        assert!(
            !matches!(parsed, DimKey::Custom(_)),
            "`{reserved}` must parse to its reserved variant, not a custom dimension"
        );
    }

    assert_eq!(
        DimKey::Custom(name("gpu_seconds")).to_string(),
        "gpu_seconds"
    );
}

// --------------------------------------------------------------------- names

#[test]
fn names_are_validated_once_at_the_abi() {
    assert!(Name::new("anthropic").is_ok());
    assert!(Name::new("vllm-local_2").is_ok());

    assert_eq!(Name::new(""), Err(NameError::Empty));
    assert_eq!(
        Name::new("Anthropic"),
        Err(NameError::BadLeadingChar { found: 'A' })
    );
    assert_eq!(
        Name::new("9lives"),
        Err(NameError::BadLeadingChar { found: '9' })
    );
    assert_eq!(
        Name::new("a/b"),
        Err(NameError::BadChar { found: '/', at: 1 })
    );
    assert!(matches!(
        Name::new(&"a".repeat(64)),
        Err(NameError::TooLong { len: 64 })
    ));
}

#[test]
fn an_invalid_name_cannot_be_deserialized_into_existence() {
    // The v1 failure this prevents: a value that the grammar rejects arriving
    // through a deserialization path that never consulted the grammar, then
    // flowing onward as if it were valid.
    let err = serde_json::from_str::<Name>("\"Not A Name\"").unwrap_err();
    assert!(err.to_string().contains("must start with"), "got: {err}");
}

// ----------------------------------------------------------------- log header

#[test]
fn a_newer_reader_accepts_an_older_log() {
    let mut header = LogHeader::current();
    assert!(header.is_readable());

    header.abi = ABI.saturating_add(1);
    assert!(
        !header.is_readable(),
        "a log from the future is refused, not guessed at"
    );

    let mut wrong_magic = LogHeader::current();
    wrong_magic.magic = *b"NOPE";
    assert!(!wrong_magic.is_readable());
}

// -------------------------------------------------------------------- entries
//
// ADR-0010 §5: the shapes the fold's refusals key on. Each of these fails only
// by someone changing the shape; that is the point.

fn blob(byte: u8) -> BlobRef {
    BlobRef::from_bytes([byte; 32])
}

fn tool() -> DriverId {
    DriverId::new(name("tool"))
}

fn json(entry: &Entry) -> serde_json::Value {
    serde_json::to_value(entry).expect("an entry serializes")
}

#[test]
fn position_is_top_level_seq_for_nine_kinds_and_msg_seq_for_three() {
    // `Refusal::OutOfOrder` reads `Entry::seq()`. Nine kinds carry `seq`
    // themselves; `Sent`, `Replied` and `Emitted` carry a `Msg` whose `seq`
    // *is* the position, and duplicating it at the top level would be a
    // second source of truth for the refusal to disagree with.
    let top_level = [
        Entry::DriverRegistered {
            seq: Seq::new(10),
            driver: tool(),
            cap: Capability::mint(0),
            ceiling: Budget::from_dims([(DimKey::Tokens, 1)]),
        },
        Entry::Spawned {
            seq: Seq::new(11),
            parent: None,
            agent: AgentId::new(0),
            ns: Namespace::empty(),
            budget: Budget::from_dims([(DimKey::Tokens, 1)]),
        },
        Entry::Resolved {
            seq: Seq::new(12),
            agent: AgentId::new(0),
            matched: Seq::new(3),
        },
        Entry::Exited {
            seq: Seq::new(13),
            agent: AgentId::new(0),
            result: blob(1),
        },
        Entry::Claimed {
            seq: Seq::new(14),
            agent: AgentId::new(0),
            by: None,
        },
        Entry::Cancelled {
            seq: Seq::new(15),
            by: None,
            agent: AgentId::new(0),
            grace: 0,
            reason: blob(2),
        },
        Entry::Tick {
            seq: Seq::new(16),
            now: 1,
        },
        Entry::Attached {
            seq: Seq::new(17),
            hook: HookId::new(0),
            point: HookPoint::OnExit,
            failure: FailureMode::Open,
            program: HookSource::Native(name("n")),
        },
        Entry::Verdicts {
            seq: Seq::new(18),
            point: HookPoint::OnExit,
            subject: AgentId::new(0),
            roll: vec![],
        },
    ];
    for (i, entry) in top_level.iter().enumerate() {
        let want = 10 + i as u64;
        assert_eq!(entry.seq().get(), want);
        assert_eq!(
            json(entry).get("seq"),
            Some(&serde_json::json!(want)),
            "{entry:?}"
        );
        assert!(json(entry).get("msg").is_none(), "{entry:?}");
    }

    let in_msg = [
        Entry::Sent {
            msg: Msg::new(
                Seq::new(20),
                Endpoint::Agent {
                    id: AgentId::new(0),
                },
                MsgKind::Request,
                blob(3),
            )
            .with_corr(Corr::new(0)),
            via: Capability::mint(0),
        },
        Entry::Replied {
            msg: Msg::new(
                Seq::new(21),
                Endpoint::Driver { id: tool() },
                MsgKind::Reply,
                blob(4),
            )
            .with_corr(Corr::new(0)),
            to: AgentId::new(0),
        },
        Entry::Emitted {
            hook: HookId::new(0),
            to: AgentId::new(0),
            msg: Msg::new(
                Seq::new(22),
                Endpoint::Hook { id: HookId::new(0) },
                MsgKind::Notice,
                blob(5),
            ),
        },
    ];
    for (i, entry) in in_msg.iter().enumerate() {
        let want = 20 + i as u64;
        assert_eq!(entry.seq().get(), want);
        assert!(json(entry).get("seq").is_none(), "{entry:?}");
        assert_eq!(
            json(entry).pointer("/msg/seq"),
            Some(&serde_json::json!(want)),
            "{entry:?}"
        );
    }
}

#[test]
fn a_reader_at_two_accepts_an_envelope_at_one_inside_a_log_at_one() {
    // `Refusal::Envelope` keys on `msg.abi`. The eight corpus logs carry
    // envelopes at 0 and 1 and must keep folding after the bump; the future
    // is refused, not guessed at.
    let mut text = String::from(r#"{"magic":[84,65,85,0],"abi":1}"#);
    text.push('\n');
    text.push_str(
        r#"{"entry":"driver_registered","seq":0,"driver":"tool","cap":0,"ceiling":{"tokens":30}}"#,
    );
    text.push('\n');
    text.push_str(
        r#"{"entry":"spawned","seq":1,"parent":null,"agent":0,"ns":{"caps":[0]},"budget":{"tokens":70,"calls":10,"depth":2}}"#,
    );
    text.push('\n');
    text.push_str(
        r#"{"entry":"sent","msg":{"abi":1,"seq":2,"from":{"kind":"agent","id":0},"corr":0,"kind":"request","consumed":null,"payload":"abababababababababababababababababababababababababababababababab"},"via":0}"#,
    );
    text.push('\n');

    let log = Log::read_from(text.as_bytes()).unwrap();
    assert_eq!(log.header().abi, 1);
    assert!(
        log.header().is_readable(),
        "a reader at {ABI} accepts a log at 1"
    );
    let state = fold(log.entries()).expect("an envelope at 1 folds on a reader at 2");
    assert_eq!(state.agents().count(), 1);

    let future = text.replace(r#""abi":1,"seq":2"#, r#""abi":99,"seq":2"#);
    let log = Log::read_from(future.as_bytes()).unwrap();
    assert_eq!(
        fold(log.entries()).unwrap_err(),
        Refusal::Envelope { found: 99 },
        "an envelope from the future is refused, not guessed at"
    );
}

#[test]
fn the_ids_a_kind_introduces_are_on_the_wire() {
    // ADR-0005: the reducer confirms an allocation on replay and never
    // re-derives it. That is only possible if the id is in the entry.
    let registered = Entry::DriverRegistered {
        seq: Seq::new(0),
        driver: tool(),
        cap: Capability::mint(7),
        ceiling: Budget::from_dims([(DimKey::Tokens, 1)]),
    };
    assert_eq!(json(&registered).get("cap"), Some(&serde_json::json!(7)));

    let spawned = Entry::Spawned {
        seq: Seq::new(1),
        parent: None,
        agent: AgentId::new(9),
        ns: Namespace::empty(),
        budget: Budget::from_dims([(DimKey::Tokens, 1)]),
    };
    assert_eq!(json(&spawned).get("agent"), Some(&serde_json::json!(9)));

    let sent = Entry::Sent {
        msg: Msg::new(
            Seq::new(2),
            Endpoint::Agent {
                id: AgentId::new(9),
            },
            MsgKind::Request,
            blob(1),
        )
        .with_corr(Corr::new(1001)),
        via: Capability::mint(7),
    };
    assert_eq!(
        json(&sent).pointer("/msg/corr"),
        Some(&serde_json::json!(1001))
    );

    let attached = Entry::Attached {
        seq: Seq::new(0),
        hook: HookId::new(4),
        point: HookPoint::PreSend,
        failure: FailureMode::Closed,
        program: HookSource::Native(name("n")),
    };
    assert_eq!(json(&attached).get("hook"), Some(&serde_json::json!(4)));
}

#[test]
fn a_roll_reads_back_in_written_order() {
    // `Refusal::BadRoll` and `UnknownHook` compare the recorded roll call to
    // the registry in `HookId` order, stopping where the roll stopped. A
    // container that re-sorted on read would hide a mis-ordered roll; the
    // wire form is a list of pairs precisely so it cannot.
    let text = r#"{"entry":"verdicts","seq":3,"point":"pre_send","subject":0,"roll":[[5,"allow"],[2,{"deny":"6666666666666666666666666666666666666666666666666666666666666666"}],[9,"allow"]]}"#;
    let entry: Entry = serde_json::from_str(text).unwrap();
    let Entry::Verdicts { roll, .. } = &entry else {
        panic!("not a verdicts entry: {entry:?}");
    };
    let order: Vec<u64> = roll.iter().map(|(h, _)| h.get()).collect();
    assert_eq!(order, vec![5, 2, 9], "the roll is a record, not a set");
    assert!(matches!(roll.get(1), Some((_, Ruling::Deny(_)))));
    assert_eq!(serde_json::to_string(&entry).unwrap(), text);
}

#[test]
fn an_unknown_entry_tag_is_a_malformed_entry_naming_its_line() {
    // ADR-0010 §2: a header this build accepts promises it can read every
    // line. A kind it does not know breaks that promise loudly — not by
    // skipping the line, and not by panicking.
    let mut text = String::new();
    text.push_str(&serde_json::to_string(&LogHeader::current()).unwrap());
    text.push('\n');
    text.push_str(r#"{"entry":"tick","seq":0,"now":1}"#);
    text.push('\n');
    text.push_str(r#"{"entry":"teleported","seq":1,"agent":0}"#);
    text.push('\n');

    let err = Log::read_from(text.as_bytes()).unwrap_err();
    assert!(
        matches!(err, LogError::MalformedEntry { line: 3, .. }),
        "got: {err:?}"
    );
}
