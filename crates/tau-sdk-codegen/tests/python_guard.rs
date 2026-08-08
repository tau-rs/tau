mod common;

/// The `TAU_REQUIRE_PYTHON3` policy: only a required-but-absent interpreter is
/// an error. Everything else is fine (the Python leg may self-skip). Tested via
/// the pure decision fn so no PATH/env manipulation is needed.
#[test]
fn errors_only_when_required_and_absent() {
    assert!(common::require_python_decision(true, false).is_err());
    assert!(common::require_python_decision(true, true).is_ok());
    assert!(common::require_python_decision(false, false).is_ok());
    assert!(common::require_python_decision(false, true).is_ok());
}
