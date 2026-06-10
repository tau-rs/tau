//! Integration: `tau dev` boots in under 2500 ms (lenient bound for CI).
//!
//! Uses the simplified-fan-monitor example with `-p` (one-shot) to measure
//! load + IR-lower + agent-pick + dispatch-and-fail time. Since there is no
//! real LLM, `run_turn` fails gracefully at plugin-loading or LLM invocation,
//! but the BOOT and TEAR-DOWN happen inside the measured window.
//!
//! The 2500 ms ceiling is intentionally loose — this is a regression net for
//! "something pathologically wrong", not a tight latency budget. Slow CI
//! runners (cross-compilation + debug builds) commonly hit ~500-800 ms for
//! the full load + attempt; 2500 ms leaves at least 3x headroom before we
//! fail loud.

#[test]
fn dev_boots_under_2500ms_for_minimal_project() {
    let example_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/dev-smoke-fan-monitor");
    assert!(
        example_dir.exists(),
        "example dir must exist at {}",
        example_dir.display()
    );

    let start = std::time::Instant::now();
    let _ = assert_cmd::Command::cargo_bin("tau")
        .unwrap()
        .current_dir(&example_dir)
        .args(["dev", ".", "-p", "noop"])
        .timeout(std::time::Duration::from_secs(10))
        .output()
        .expect("run tau dev -p noop");
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 2500,
        "boot took {}ms (limit 2500ms)",
        elapsed.as_millis()
    );
}
