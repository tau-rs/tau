//! Integration: `:reload` preserves conversation history.

use assert_fs::prelude::*;
use std::sync::atomic::Ordering;
use tau_domain::{Address, AgentInstanceId, Message, MessagePayload};

#[tokio::test(flavor = "current_thread")]
async fn reload_preserves_history_length() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "history-test"
version = "0.0.1"

[agents.a]
display_name = "A"
package      = "a@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "v1"
"#,
        )
        .unwrap();

    let mut session = tau_cli::cmd::dev::session::DevSession::load(tmp.path().to_path_buf(), None)
        .await
        .unwrap();

    // Push a real Message into history so we have something concrete to
    // verify survives the reload.
    let msg = Message::new(
        Address::User,
        Address::Agent(AgentInstanceId::new()),
        MessagePayload::Text {
            content: "hello before reload".to_string(),
        },
    );
    session.history.push(msg);
    let before_len = session.history.len();
    assert_eq!(before_len, 1, "one message in history before reload");

    // Edit the manifest.
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "history-test"
version = "0.0.1"

[agents.a]
display_name = "A"
package      = "a@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
prompt.system = "v2"
"#,
        )
        .unwrap();

    // Arm the flag that the watcher would normally set.
    session.pending_reload.store(true, Ordering::Release);

    session.reload().await.unwrap();

    // History must be identical — same length and same content.
    assert_eq!(
        session.history.len(),
        before_len,
        "reload must not clear history"
    );
    match &session.history[0].payload {
        MessagePayload::Text { content } => {
            assert_eq!(content, "hello before reload");
        }
        other => panic!("unexpected payload variant: {other:?}"),
    }
}
