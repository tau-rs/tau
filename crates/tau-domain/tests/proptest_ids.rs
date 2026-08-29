//! Property tests for `PackageName` and `AgentId` grammar.

use proptest::prelude::*;
use std::str::FromStr;

use tau_domain::{AgentId, AgentIdError, PackageName, PackageNameError};

proptest! {
    #[test]
    fn package_name_round_trips(s in "[a-z][a-z0-9-]{0,63}") {
        let n = PackageName::from_str(&s).unwrap();
        prop_assert_eq!(n.to_string(), s);
    }

    #[test]
    fn agent_id_round_trips(s in "[a-z][a-z0-9-]{0,63}") {
        let id = AgentId::from_str(&s).unwrap();
        prop_assert_eq!(id.to_string(), s);
    }

    /// The widened grammar (ADR-0070): `/`-separated segments, each starting
    /// with a letter or digit, `_` legal inside. Bounded at 3 segments of
    /// <=19 bytes so the generator can never exceed the 64-byte cap.
    #[test]
    fn agent_id_round_trips_namespaced(
        s in "[a-z0-9][a-z0-9_-]{0,18}(/[a-z0-9][a-z0-9_-]{0,18}){0,2}"
    ) {
        let id = AgentId::from_str(&s).unwrap();
        prop_assert_eq!(id.to_string(), s);
    }

    /// `AgentId` is a strict superset of `PackageName`: anything the narrow
    /// grammar accepts, the wide one accepts too. The converse is false by
    /// design (ADR-0070, Decision 2) — pinned by the unit test
    /// `id::agent_id_tests::diverges_from_package_name`.
    #[test]
    fn agent_id_accepts_everything_package_name_does(s in "[a-z][a-z0-9-]{0,63}") {
        prop_assume!(PackageName::from_str(&s).is_ok());
        prop_assert!(AgentId::from_str(&s).is_ok());
    }

    #[test]
    fn package_name_invalid_leading_rejected(s in "[A-Z0-9-][a-z0-9-]{0,63}") {
        let result = PackageName::from_str(&s);
        let ok = matches!(
            result,
            Err(PackageNameError::InvalidLeadingCharacter { .. }) | Err(PackageNameError::Empty)
        );
        prop_assert!(ok);
    }

    /// Digits are NO LONGER an invalid leading character for an `AgentId`
    /// (ADR-0070 relaxed the rule to `[a-z0-9]` so the domain grammar covers
    /// every segment the `[dirs]` scanner can produce), so the generator here
    /// is narrower than `package_name_invalid_leading_rejected`'s: uppercase,
    /// `-`, and `_` only. A leading `-` in particular must stay illegal —
    /// `tau build --agent <id>` must never be ambiguous with a flag.
    #[test]
    fn agent_id_invalid_leading_rejected(s in "[A-Z_-][a-z0-9-]{0,63}") {
        let result = AgentId::from_str(&s);
        let ok = matches!(
            result,
            Err(AgentIdError::InvalidLeadingCharacter { .. }) | Err(AgentIdError::Empty)
        );
        prop_assert!(ok, "should reject {:?}, got {:?}", s, result);
    }

    /// The per-segment leading rule applies to every segment, not just the
    /// first — the case a whole-string charset check would miss.
    #[test]
    fn agent_id_invalid_leading_in_later_segment_rejected(
        head in "[a-z0-9][a-z0-9_-]{0,18}",
        bad in "[A-Z_-][a-z0-9-]{0,18}"
    ) {
        let s = alloc_join(&head, &bad);
        let result = AgentId::from_str(&s);
        prop_assert!(
            matches!(result, Err(AgentIdError::InvalidLeadingCharacter { .. })),
            "should reject {:?}, got {:?}", s, result
        );
    }

    /// A leading, trailing, or doubled separator leaves an empty segment.
    #[test]
    fn agent_id_empty_segments_rejected(s in "[a-z][a-z0-9-]{0,20}") {
        for candidate in [
            format!("/{s}"),
            format!("{s}/"),
            format!("{s}//{s}"),
        ] {
            prop_assert!(
                matches!(
                    AgentId::from_str(&candidate),
                    Err(AgentIdError::EmptySegment { .. })
                ),
                "should reject {:?}", candidate
            );
        }
    }
}

/// `format!` helper kept out of the `proptest!` macro body, which does not
/// tolerate arbitrary statements well.
fn alloc_join(head: &str, tail: &str) -> String {
    format!("{head}/{tail}")
}
