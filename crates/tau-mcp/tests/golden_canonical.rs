//! Golden-vector test for canonical-hash determinism.
//!
//! Reads two fixture `ServerContract` JSONs, computes canonical hashes,
//! and asserts they match the recorded constants below. If the canonical
//! encoder changes shape, these constants change too — the test fails
//! and the test author updates them with the new values (treat that as
//! an intentional protocol-format bump, NOT just a test fix).

use std::fs;

use tau_mcp::contract::{canonical_hash, hash_to_hex, ServerContract};

fn load_fixture(name: &str) -> ServerContract {
    let path = format!(
        "{}/tests/fixtures/canonical/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    let bytes = fs::read(&path).expect("read fixture");
    serde_json::from_slice(&bytes).expect("decode fixture")
}

#[test]
fn empty_contract_golden_hash() {
    let c = load_fixture("empty.json");
    let h = canonical_hash(&c).expect("hash");
    // First-time author: leave the assert below pointing at a
    // placeholder, run the test, capture the value, fill it in, re-run.
    // After this lands, any future change to the canonical encoder MUST
    // intentionally update this constant.
    let expected = include_str!("expected_hashes/empty.hex").trim();
    assert_eq!(hash_to_hex(&h), expected);
}

#[test]
fn weather_contract_golden_hash() {
    let c = load_fixture("weather.json");
    let h = canonical_hash(&c).expect("hash");
    let expected = include_str!("expected_hashes/weather.hex").trim();
    assert_eq!(hash_to_hex(&h), expected);
}
