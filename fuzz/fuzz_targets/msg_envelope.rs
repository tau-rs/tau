//!  `Msg` envelope deserialization over arbitrary bytes.
//!
//! The envelope is the frozen shape every effect travels in (ADR-0004), and a
//! driver's reply is the one place bytes from outside the kernel become one.
//! Any bytes must yield a `Msg` or a `serde_json::Error`; a `Msg` that comes
//! out must serialize and deserialize back to itself.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_kernel::abi::Msg;

fuzz_target!(|data: &[u8]| {
    let Ok(msg) = serde_json::from_slice::<Msg>(data) else {
        return;
    };
    let again = serde_json::to_vec(&msg)
        .and_then(|bytes| serde_json::from_slice::<Msg>(&bytes))
        .map_err(|e| e.to_string());
    assert_eq!(again, Ok(msg), "envelope did not survive a round trip");
});
