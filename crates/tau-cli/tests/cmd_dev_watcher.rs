//! Integration tests: file watcher fires when tau.toml changes.

use std::sync::atomic::Ordering;
use std::time::Duration;

use assert_fs::prelude::*;

#[tokio::test(flavor = "current_thread")]
async fn watcher_flips_pending_reload_on_tau_toml_edit() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "watcher-test"

[agents.a]
display_name = "A"
package      = "agent-a@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "first"
"#,
        )
        .expect("write");

    let session = tau_cli::cmd::dev::session::DevSession::load(
        tmp.path().to_path_buf(),
        None,
    )
    .await
    .expect("load");

    assert!(
        !session.pending_reload.load(Ordering::Acquire),
        "pending_reload must start false"
    );

    // Edit tau.toml to trigger the watcher.
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "watcher-test"

[agents.a]
display_name = "A"
package      = "agent-a@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "second"
"#,
        )
        .expect("edit");

    // Poll for up to 2 s (FSEvents on macOS can be a little slow).
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if session.pending_reload.load(Ordering::Acquire) {
            return; // success
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("pending_reload did not flip within 2 s after tau.toml edit");
}

#[tokio::test(flavor = "current_thread")]
async fn watcher_ignores_tau_lock_changes() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "watcher-test"

[agents.a]
display_name = "A"
package      = "agent-a@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "x"
"#,
        )
        .expect("write");

    let session = tau_cli::cmd::dev::session::DevSession::load(
        tmp.path().to_path_buf(),
        None,
    )
    .await
    .expect("load");

    // Write tau-lock.toml — NOT a watched path.
    tmp.child("tau-lock.toml")
        .write_str(
            r#"schema_version = 7
created_at = "2026-06-10T00:00:00Z"
tau_version = "0.0.0"
packages = []
"#,
        )
        .expect("write lock");

    // Give the watcher 700 ms to (incorrectly) fire.
    tokio::time::sleep(Duration::from_millis(700)).await;

    assert!(
        !session.pending_reload.load(Ordering::Acquire),
        "tau-lock.toml changes must not trigger reload"
    );
}
