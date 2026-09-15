//! Every committed cassette is free of secret shapes and of request headers
//! outside the allowlist, and names the directory it sits in.

#![cfg(feature = "anthropic")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::cassette::{self, Cassette, ALLOWED_HEADERS};

#[test]
fn no_cassette_carries_a_secret_shape_or_a_foreign_header() {
    for file in cassette::all_files() {
        let text = std::fs::read_to_string(&file).unwrap();
        if let Some(pattern) = cassette::scan_for_secrets(&text) {
            panic!("{}: matches secret pattern {pattern}", file.display());
        }
        let c: Cassette =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        assert_eq!(c.v, 1, "{}: unknown cassette version", file.display());
        let dir_name = file
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(
            c.target,
            dir_name,
            "{}: target does not match its directory",
            file.display()
        );
        for ex in &c.exchanges {
            for name in ex.request.headers.keys() {
                assert!(
                    ALLOWED_HEADERS.contains(&name.as_str()),
                    "{}: header {name} is not allowlisted",
                    file.display()
                );
            }
        }
    }
}

#[test]
fn the_scan_catches_every_planted_shape() {
    for planted in [
        "x-api-key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz",
        "sk-proj-1234567890abcdefghijklmnop",
        "Authorization: Bearer abc.def.ghi",
        "\"sk-abcdefghijklmnopqrstuvwxyz0123\"",
    ] {
        assert!(
            cassette::scan_for_secrets(planted).is_some(),
            "{planted} slipped through"
        );
    }
    assert!(
        cassette::scan_for_secrets("{\"id\":\"msg_01ABC\",\"skip\":\"sk-\"}").is_none(),
        "a bare sk- prefix is not a key"
    );
}

#[test]
fn redaction_keeps_only_the_allowlist_and_refuses_strangers() {
    let ok = common::Captured {
        head: "POST /v1/messages HTTP/1.1\r\nhost: x\r\ncontent-type: application/json\r\nx-api-key: sk-ant-secret\r\nanthropic-version: 2023-06-01\r\ncontent-length: 2\r\naccept: */*\r\n\r\n".into(),
        body: b"{}".to_vec(),
    };
    let r = cassette::redact(&ok).unwrap();
    assert_eq!(r.method, "POST");
    assert_eq!(r.path, "/v1/messages");
    assert_eq!(
        r.headers.keys().cloned().collect::<Vec<_>>(),
        ["anthropic-version", "content-type"]
    );
    assert!(!serde_json::to_string(&r).unwrap().contains("secret"));

    let stranger = common::Captured {
        head: "POST /v1/messages HTTP/1.1\r\ncontent-type: application/json\r\nx-org-id: org_123\r\n\r\n".into(),
        body: b"{}".to_vec(),
    };
    assert_eq!(cassette::redact(&stranger).unwrap_err(), "x-org-id");
}
