//! Scratch red proof for #39 / PR #133: one test that always fails, so the
//! `serial (test-threads 1)` leg is seen to go red and name the test. Never
//! merged.

#[test]
fn serial_red_proof_deliberately_fails() {
    assert_eq!(1 + 1, 3, "deliberate: serial (test-threads 1) red proof");
}
