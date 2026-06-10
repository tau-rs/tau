//! Integration: --watch auto-reloads without explicit :reload.
//!
//! Verifies that DevSession::auto_reload_if_pending applies a pending
//! manifest change without needing the user to type :reload.

use assert_fs::prelude::*;

#[tokio::test(flavor = "current_thread")]
async fn auto_reload_applies_pending_change() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("tau.toml").write_str(r#"
[project]
name = "watch-test"
version = "0.0.1"

[agents.first]
display_name = "First"
package      = "first@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "v1"
"#).unwrap();

    let mut session = tau_cli::cmd::dev::session::DevSession::load(
        tmp.path().to_path_buf(), None
    ).await.unwrap();
    assert_eq!(session.project.agents.len(), 1);

    // Manually arm pending_reload (skipping the watcher's debounce window).
    session.pending_reload.store(true, std::sync::atomic::Ordering::Release);

    tmp.child("tau.toml").write_str(r#"
[project]
name = "watch-test"
version = "0.0.1"

[agents.first]
display_name = "First"
package      = "first@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "v2"

[agents.second]
display_name = "Second"
package      = "second@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "v2"
"#).unwrap();

    let did_reload = session.auto_reload_if_pending().await.unwrap();
    assert!(did_reload, "watch mode should auto-reload when pending");
    assert_eq!(session.project.agents.len(), 2, "agents reloaded");
}

#[tokio::test(flavor = "current_thread")]
async fn auto_reload_is_noop_when_nothing_pending() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("tau.toml").write_str(r#"
[project]
name = "watch-noop-test"
version = "0.0.1"

[agents.a]
display_name = "A"
package      = "a@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "x"
"#).unwrap();

    let mut session = tau_cli::cmd::dev::session::DevSession::load(
        tmp.path().to_path_buf(), None
    ).await.unwrap();
    // No pending reload, no manifest edit.
    let did = session.auto_reload_if_pending().await.unwrap();
    assert!(!did, "no reload should happen when nothing is pending");
}
