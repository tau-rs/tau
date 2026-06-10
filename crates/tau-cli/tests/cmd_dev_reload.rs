//! Integration: `:reload` applies a manifest edit.

use assert_fs::prelude::*;
use std::sync::atomic::Ordering;

#[tokio::test(flavor = "current_thread")]
async fn reload_picks_up_new_agent_after_edit() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "reload-test"
version = "0.0.1"

[agents.first]
display_name = "First"
package      = "first@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "first"
"#,
        )
        .unwrap();

    let mut session = tau_cli::cmd::dev::session::DevSession::load(
        tmp.path().to_path_buf(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(session.current_agent_name(), "first");
    assert_eq!(session.project.agents.len(), 1);

    // Edit tau.toml — add a second agent.
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "reload-test"
version = "0.0.1"

[agents.first]
display_name = "First"
package      = "first@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "first-EDITED"

[agents.second]
display_name = "Second"
package      = "second@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "second"
"#,
        )
        .unwrap();

    // Arm the flag that the watcher would normally set (avoid FSEvents timing).
    session.pending_reload.store(true, Ordering::Release);

    let did_reload = session.reload().await.unwrap();
    assert!(did_reload, "reload() should return true when pending was set");
    assert_eq!(session.project.agents.len(), 2, "new manifest has 2 agents");
    // current_agent falls back to first alphabetically — still "first".
    assert_eq!(session.current_agent_name(), "first");
}
