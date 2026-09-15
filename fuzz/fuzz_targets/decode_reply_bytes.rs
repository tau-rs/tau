//! `libtau::decode_reply` over arbitrary bytes.
//!
//! Model output is attacker-controlled input, and this is the exact path it
//! enters the tool loop by (ADR-0006 §3). Any bytes must yield a `ModelReply`
//! or an `InferError`; a reply that comes out is stamped with the bridge
//! version this crate speaks and survives a round trip through its own
//! serializer.
//!
//! The seed corpus carries a reply with a six-figure `content` string.
//! libFuzzer sizes `-max_len` to its largest seed, so the mutator reaches
//! the sizes a real completion has without a flag anyone has to remember.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_kernel::bridge::VERSION;

fuzz_target!(|data: &[u8]| {
    let Ok(reply) = libtau::decode_reply(data) else {
        return;
    };
    assert_eq!(reply.v, VERSION, "decode_reply accepted a foreign version");
    let again = serde_json::to_vec(&reply)
        .map_err(|e| e.to_string())
        .and_then(|bytes| libtau::decode_reply(&bytes).map_err(|e| e.to_string()));
    assert_eq!(again, Ok(reply), "reply did not survive a round trip");
});
