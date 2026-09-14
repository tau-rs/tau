//! Scratch: a deliberate heap leak so the Tier 2 `asan+lsan (kernel)` job can
//! be shown to go red on a real fault (#38). Never merge.

#[test]
fn a_deliberate_leak_for_the_red_proof() {
    let block: Vec<u8> = vec![0xAB; 1 << 16];
    std::mem::forget(block);
}
