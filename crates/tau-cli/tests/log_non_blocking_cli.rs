//! Integration test for sub-project E Task 4:
//! the global `--log-non-blocking` flag requires `--log-file`. When the
//! invocation supplies the former without the latter, the CLI must
//! exit with a non-zero status AND a recognizable message on stderr.
//!
//! The test scrubs `TAU_LOG_FILE` (and `TAU_LOG_NON_BLOCKING`) from the
//! inherited env so a developer's shell export cannot mask the
//! misconfiguration the test is validating. `TAU_HOME` is pointed at a
//! per-test tempdir to keep us off the user's real `~/.tau` and to
//! avoid the Windows config-write race documented in memory.

use assert_cmd::Command;

#[test]
fn non_blocking_without_log_file_exits_with_error_message() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("tau")
        .unwrap()
        .args(["--log-non-blocking", "list", "packages"])
        .env("HOME", tmp.path())
        .env("TAU_HOME", tmp.path())
        .env_remove("TAU_LOG_FILE")
        .env_remove("TAU_LOG_NON_BLOCKING")
        .env_remove("TAU_LOG_ROTATION")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "--log-non-blocking requires --log-file",
        ));
}
