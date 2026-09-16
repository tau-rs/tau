//! Guard and stub plumbing: every committed cassette is free of secret
//! shapes and of request headers outside the allowlist and names the
//! directory it sits in, and the stub's forward mode relays a request to a
//! real upstream and reports what happened.

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

#[tokio::test]
async fn forward_mode_relays_headers_body_status_and_reports_the_exchange() {
    // The "provider": a plain stub answering 418 with a fixed body.
    let mut upstream = common::start(common::Answer::Json(418, r#"{"ok":true}"#.into())).await;
    let (relay, mut relayed) = common::start_forwarding(upstream.base_url.clone()).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/messages", relay.base_url))
        .header("content-type", "application/json")
        .header("x-api-key", "sk-ant-not-a-real-key")
        .header("accept-encoding", "gzip")
        .header("connection", "keep-alive")
        .body(r#"{"hello":"world"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 418);
    assert_eq!(resp.text().await.unwrap(), r#"{"ok":true}"#);

    let seen_upstream = upstream.captured.recv().await.unwrap();
    assert_eq!(seen_upstream.path(), "/v1/messages");
    assert_eq!(
        seen_upstream.header("x-api-key"),
        Some("sk-ant-not-a-real-key")
    );
    assert_eq!(seen_upstream.json(), serde_json::json!({"hello":"world"}));
    // Hop-by-hop headers from the client are stripped, not relayed verbatim:
    // the client's `accept-encoding` must not reach upstream at all, and its
    // `connection: keep-alive` must not pass through (reqwest may set its
    // own `connection` for the relay -> upstream hop; that's fine).
    assert_eq!(seen_upstream.header("accept-encoding"), None);
    assert_ne!(seen_upstream.header("connection"), Some("keep-alive"));
    // The relay does not forward a stale content-length: what upstream sees
    // matches the body upstream actually received.
    let seen_content_length: usize = seen_upstream
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap();
    assert_eq!(seen_content_length, seen_upstream.body.len());

    let r = relayed.recv().await.unwrap();
    assert_eq!(r.status, 418);
    assert_eq!(r.body, br#"{"ok":true}"#);
    assert_eq!(r.request.path(), "/v1/messages");
}

use common::scenario::{self, Expect, Scenario, Step, Target};
use tau_drivers::model::anthropic::{AnthropicConfig, AnthropicDriver, ApiKey};
use tau_kernel::bridge::{Content, Message, ModelRequest, Role, VERSION};

fn hello() -> ModelRequest {
    ModelRequest {
        v: VERSION,
        system: None,
        messages: vec![Message {
            role: Role::User,
            content: vec![Content::Text { text: "hi".into() }],
        }],
        tools: vec![],
        max_tokens: 64,
        sampling: None,
    }
}

/// Removes the directory on drop, so the synthetic cassette is cleaned up
/// even if an assertion inside the test panics: nextest runs tests in
/// parallel, and a leftover directory is picked up by the secret-shape
/// guard on this run or a later one.
struct RemoveOnDrop(std::path::PathBuf);
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn replay_runs_a_synthetic_cassette_and_checks_request_equality() {
    // Build a cassette by hand from the ADR fixture the contract suite already trusts.
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/anthropic/response-tool-use.json")).unwrap();
    let make = |base: &str| -> Box<dyn tau_kernel::driver::Driver> {
        let mut cfg = AnthropicConfig::new(
            "claude-haiku-4-5-20251001",
            ApiKey::new("sk-test"),
            8_000,
            64,
            1,
            5,
        );
        cfg.base_url = base.to_owned();
        Box::new(AnthropicDriver::new(cfg).unwrap())
    };
    // What the driver will send for `hello()` — capture it once against a plain stub.
    let mut probe = common::start(common::Answer::Json(200, fixture.to_string())).await;
    let d = make(&probe.base_url);
    let _ = scenario::call(d.as_ref(), 1, &hello()).await;
    let sent = probe.captured.recv().await.unwrap();

    let c = common::cassette::Cassette {
        v: 1,
        recorded_at: "2026-09-15".into(),
        target: "synthetic".into(),
        model: Some("claude-haiku-4-5-20251001".into()),
        exchanges: vec![common::cassette::Exchange {
            request: common::cassette::redact(&sent).unwrap(),
            response: common::cassette::RecordedResponse {
                status: 200,
                body: fixture,
            },
        }],
    };
    let _cleanup = RemoveOnDrop(common::cassette::dir().join("synthetic"));
    common::cassette::save(&c, "synthetic_tool_use");

    let s = Scenario {
        name: "synthetic_tool_use",
        target: Target::Anthropic,
        model: "claude-haiku-4-5-20251001",
        steps: vec![Step {
            build: Box::new(|_| hello()),
            expect: Expect::ToolCall {
                name: "search",
                min: 1,
            },
        }],
    };
    // Point replay at the synthetic directory by overriding the target dir name.
    scenario::replay_from(&s, "synthetic", Box::new(make)).await;
}
