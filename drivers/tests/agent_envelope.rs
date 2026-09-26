//! The worker envelope and its tolerant parser (ADR-0013 §4), against the
//! final messages #130 actually recorded and against the ways a session can
//! get the contract wrong.
//!
//! The parser is tolerant because a session that ignored its contract still
//! ran a task somebody paid for; it is never *creative*, because a
//! synthesized envelope would be indistinguishable from one the session
//! wrote.

#![cfg(all(feature = "agent", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use serde_json::{json, Value};
use tau_drivers::agent::envelope::{self, Envelope, Kind, Status};

/// #130 run 1's final message, verbatim but for the three assumptions that
/// were the user's own plugins leaking into the worker — the leak
/// `--safe-mode` now closes (ADR-0013 §7).
const RECORDED: &str = include_str!("fixtures/agent/envelope.json");
const SCHEMA: &str = include_str!("fixtures/agent/envelope-schema.json");

#[test]
fn the_recorded_envelope_round_trips_and_parses() {
    let expected: Value = serde_json::from_str(RECORDED).unwrap();
    let typed: Envelope = serde_json::from_value(expected.clone()).unwrap();
    assert_eq!(serde_json::to_value(&typed).unwrap(), expected);

    let parsed = envelope::parse(RECORDED.as_bytes()).unwrap();
    assert_eq!(parsed, typed);
    assert_eq!(parsed.status, Status::Ok);
    assert_eq!(parsed.artifacts[0].kind, Kind::File);
    assert!(
        parsed.artifacts[0].path.starts_with('/'),
        "the contract asks for relative paths; #130's session wrote an \
         absolute one, and the driver rewrites neither"
    );
}

#[test]
fn a_fenced_envelope_is_unwrapped() {
    for fenced in [
        format!("```json\n{RECORDED}\n```"),
        format!("```\n{RECORDED}\n```"),
        format!("   ```json\n{RECORDED}\n```   "),
    ] {
        let parsed = envelope::parse(fenced.as_bytes()).unwrap();
        assert_eq!(parsed.status, Status::Ok, "{fenced}");
    }
}

#[test]
fn the_last_balanced_object_wins() {
    // A session that narrates first, and one that quotes the schema it was
    // given before answering: the report is the last object either way.
    let narrated = format!("I wrote the file, and here is the report:\n\n{RECORDED}");
    assert_eq!(
        envelope::parse(narrated.as_bytes()).unwrap().status,
        Status::Ok
    );
    let decoy = format!(
        "My contract was {{\"status\":\"ok | partial\",\"summary\":\"<= 3 sentences\"}}.\n{RECORDED}"
    );
    let parsed = envelope::parse(decoy.as_bytes()).unwrap();
    assert!(
        parsed.summary.starts_with("Created hello.txt"),
        "{parsed:?}"
    );
}

#[test]
fn braces_inside_strings_do_not_move_the_boundaries() {
    let tricky = r#"{"status":"failed","summary":"the template `{ \"a\": 1 }` broke","error":"} unbalanced {"}"#;
    let parsed = envelope::parse(tricky.as_bytes()).unwrap();
    assert_eq!(parsed.status, Status::Failed);
    assert_eq!(parsed.error.as_deref(), Some("} unbalanced {"));
    assert!(parsed.summary.contains("{ \"a\": 1 }"));
}

#[test]
fn the_three_lists_default_and_the_two_facts_do_not() {
    let minimal = envelope::parse(br#"{"status":"partial","summary":"half of it"}"#).unwrap();
    assert_eq!(minimal.status, Status::Partial);
    assert!(minimal.artifacts.is_empty() && minimal.assumptions.is_empty());
    assert!(minimal.events.is_empty() && minimal.error.is_none());

    for missing in [
        br#"{"summary":"no verdict"}"#.as_slice(),
        br#"{"status":"ok"}"#.as_slice(),
    ] {
        let violation = envelope::parse(missing).unwrap_err();
        assert!(violation.reason.contains("not an envelope"), "{violation}");
    }
}

#[test]
fn a_shape_the_contract_does_not_name_is_a_violation_not_a_guess() {
    for wrong in [
        // A verdict the contract does not have.
        br#"{"status":"done","summary":"x"}"#.as_slice(),
        // An artifact kind it does not have.
        br#"{"status":"ok","summary":"x","artifacts":[{"path":"a","kind":"blob"}]}"#.as_slice(),
        // The right fields, the wrong types.
        br#"{"status":"ok","summary":["x"]}"#.as_slice(),
    ] {
        assert!(
            envelope::parse(wrong).is_err(),
            "{}",
            String::from_utf8_lossy(wrong)
        );
    }
}

#[test]
fn a_final_message_with_no_object_says_so_and_never_panics() {
    let empty = envelope::parse(b"").unwrap_err();
    assert!(empty.reason.contains("empty"), "{empty}");

    for hostile in [
        b"I could not do it, sorry.".as_slice(),
        // #130 run 6: a turn bound ends the session with `result: null`,
        // and the model never got a turn to write an envelope.
        b"null".as_slice(),
        b"[]".as_slice(),
        br#"{"status":"ok","summary":"unterminated"#.as_slice(),
        b"{{{{{{{{{{".as_slice(),
        b"}}}}}}}}}}".as_slice(),
        br#""{\"status\":\"ok\"}""#.as_slice(),
        // Invalid UTF-8 is read lossily: one bad byte is not a reason to
        // throw away a report.
        &[0xff, 0xfe, b'{', b'}'],
    ] {
        let violation = envelope::parse(hostile).unwrap_err();
        assert!(!violation.reason.is_empty());
    }

    // Depth is serde's to refuse, and it refuses rather than recursing into
    // the stack: this is the input a fuzz target would find first.
    let deep = format!("{}{}", "{\"a\":".repeat(2_000), "}".repeat(2_000));
    assert!(envelope::parse(deep.as_bytes()).is_err());
    let wide = format!(
        "{{\"status\":\"ok\",\"summary\":\"{}\"}}",
        "x".repeat(100_000)
    );
    assert_eq!(
        envelope::parse(wide.as_bytes()).unwrap().summary.len(),
        100_000
    );
}

#[test]
fn the_schema_is_strict_where_the_parser_is_tolerant() {
    let fixture: Value = serde_json::from_str(SCHEMA).unwrap();
    assert_eq!(envelope::schema(), fixture);
    assert_eq!(fixture["type"], json!("object"));
    assert_eq!(fixture["additionalProperties"], json!(false));
    assert_eq!(
        fixture["required"],
        json!([
            "artifacts",
            "assumptions",
            "error",
            "events",
            "status",
            "summary"
        ]),
        "every field, including the three the parser defaults"
    );
    assert_eq!(
        fixture["properties"]["error"]["type"],
        json!(["string", "null"])
    );
    let text = serde_json::to_string(&fixture).unwrap();
    assert!(
        !text.contains("\"default\""),
        "a structured-output schema states requirements, not defaults"
    );
    for status in ["ok", "partial", "failed", "cancelled"] {
        assert!(
            text.contains(&format!("\"{status}\"")),
            "{status} is missing"
        );
    }
    // Structured-output modes are strict all the way down: OpenAI's rejects
    // a schema whose nested object leaves `additionalProperties` open
    // (`invalid_json_schema` at `properties.artifacts.items`, #128's first
    // `codex` recording), so every object closes, not only the root.
    let item = &fixture["properties"]["artifacts"]["items"];
    assert_eq!(item["type"], json!("object"));
    assert_eq!(item["additionalProperties"], json!(false));
    assert_eq!(item["required"], json!(["kind", "path"]));
    // ... and it forbids `oneOf` outright (`'oneOf' is not permitted` at
    // `...items.properties.kind`); `anyOf` says the same of a constant set.
    assert!(!text.contains("\"oneOf\""), "oneOf is not permitted");
    assert!(fixture["properties"]["status"]["anyOf"].is_array());
    assert!(item["properties"]["kind"]["anyOf"].is_array());
}
