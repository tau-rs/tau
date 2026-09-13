//! The ABI guard, part two: the invariants the types exist to carry.
//!
//! Snapshots pin the *shape*. These pin the *meaning* — the handful of
//! properties that, if they ever stopped holding, would make the frozen
//! boundary a lie regardless of how stable its serialization was.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use tau_kernel::abi::{
    Budget, BudgetError, Capability, DimKey, LogHeader, Name, NameError, Namespace, ABI,
};

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
