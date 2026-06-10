//! Integration: `:reload` with malformed tau.toml keeps old config.

use assert_fs::prelude::*;
use std::sync::atomic::Ordering;

#[tokio::test(flavor = "current_thread")]
async fn malformed_reload_keeps_previous_config() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "malformed-test"
version = "0.0.1"

[agents.a]
display_name = "A"
package      = "a@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "valid"
"#,
        )
        .unwrap();

    let mut session = tau_cli::cmd::dev::session::DevSession::load(
        tmp.path().to_path_buf(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(session.current_agent_name(), "a");
    assert_eq!(session.project.agents.len(), 1);

    // Overwrite tau.toml with invalid TOML.
    tmp.child("tau.toml")
        .write_str("this is not valid toml [[[")
        .unwrap();

    // Arm the flag that the watcher would normally set.
    session.pending_reload.store(true, Ordering::Release);

    let err = session.reload().await.expect_err("should fail on malformed TOML");
    let msg = format!("{err}");
    assert!(
        msg.contains("parse") || msg.contains("TOML") || msg.contains("toml"),
        "expected parse error mention, got: {msg}"
    );

    // Old config must still be in effect.
    assert_eq!(session.current_agent_name(), "a");
    assert_eq!(session.project.agents.len(), 1);

    // pending_reload must be restored so the user can retry after fixing.
    assert!(
        session.pending_reload.load(Ordering::Acquire),
        "pending_reload must be restored after failed reload"
    );
}
