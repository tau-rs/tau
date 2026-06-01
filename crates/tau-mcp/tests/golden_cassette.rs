//! Golden-vector test for the cassette format.
//!
//! Asserts the on-disk fixture is parseable and replays in the
//! expected order.

use std::fs;

use tau_mcp::cassette::{Direction, MessageKind, Replayer};
use tau_mcp::protocol::jsonrpc::RequestId;

fn load_cassette() -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/cassette/weather-happy-path.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    fs::read(&path).expect("read fixture")
}

#[test]
fn weather_happy_path_parses() {
    let bytes = load_cassette();
    let _r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
}

#[test]
fn weather_happy_path_full_replay() {
    let bytes = load_cassette();
    let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");

    // initialize
    let resp = r
        .match_request(
            "initialize",
            &serde_json::json!({"protocolVersion":"2025-03-26"}),
        )
        .expect("init");
    assert_eq!(resp.id, Some(RequestId::Number(0)));

    // tools/list
    let resp = r
        .match_request("tools/list", &serde_json::Value::Null)
        .expect("list");
    assert_eq!(resp.id, Some(RequestId::Number(1)));

    // tools/call
    let resp = r
        .match_request(
            "tools/call",
            &serde_json::json!({"name":"get_forecast","arguments":{"lat":40.7,"lon":-74.0}}),
        )
        .expect("call");
    assert_eq!(resp.id, Some(RequestId::Number(2)));

    // progress notification was queued between request + response
    let pending = r.next_pending_outbound().expect("progress notification");
    assert_eq!(pending.dir, Direction::Out);
    assert_eq!(pending.kind, MessageKind::Notification);
    assert_eq!(pending.method.as_deref(), Some("notifications/progress"));
}
