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
packages = ["mock-llm"]

[project]
name = "watcher-test"

[models]
mock-1 = { backend = "mock-llm", model = "claude-haiku-4-5" }

[agents.a]
display_name = "A"
package      = "agent-a@^0.1"
model        = "mock-1"
prompt.system = "first"
"#,
        )
        .expect("write");

    let session = tau_cli::cmd::dev::session::DevSession::load(tmp.path().to_path_buf(), None)
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
packages = ["mock-llm"]

[project]
name = "watcher-test"

[models]
mock-1 = { backend = "mock-llm", model = "claude-haiku-4-5" }

[agents.a]
display_name = "A"
package      = "agent-a@^0.1"
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
async fn watcher_ignores_preexisting_dirs_files_then_flips_on_new_file() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("tau.toml")
        .write_str(
            r#"
packages = ["mock-llm"]

[project]
name = "watcher-dirs-test"

[dirs]
tools = "tools"

[models]
mock-1 = { backend = "mock-llm", model = "claude-haiku-4-5" }

[agents.a]
display_name = "A"
package      = "agent-a@^0.1"
model        = "mock-1"
prompt.system = "x"
"#,
        )
        .expect("write");
    // Pre-existing file under the [dirs] root, present *before* the watch
    // registers — the regression case for the boot-registration replay
    // (FSEvents replays a Create event for files that already existed
    // when the recursive watch was set up; see hash_watched_dirs' doc
    // comment / commit 6d1a8552 for the original file-watch version of
    // this race). A project with an empty `tools/` at boot can't catch
    // this: real projects always have pre-existing definitions here.
    tmp.child("tools/existing_tool.toml")
        .write_str("mcp = \"https://mcp.example.com\"\n")
        .expect("write pre-existing tool");

    let session = tau_cli::cmd::dev::session::DevSession::load(tmp.path().to_path_buf(), None)
        .await
        .expect("load");

    // Give any boot-registration replay a window to (incorrectly) fire
    // before asserting — same 700 ms budget as `watcher_ignores_tau_lock_changes`.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(
        !session.pending_reload.load(Ordering::Acquire),
        "pre-existing [dirs] files must not trigger a reload at boot"
    );

    // Now create a new file in a *nested* subdirectory of the watched
    // `[dirs]` root — exercises RecursiveMode::Recursive, not just the
    // root itself — and confirm a genuine change still flips the flag.
    tmp.child("tools/sub/new_tool.toml")
        .write_str("mcp = \"https://mcp2.example.com\"\n")
        .expect("write nested file");

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if session.pending_reload.load(Ordering::Acquire) {
            return; // success
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("pending_reload did not flip within 2 s after a new file appeared under a [dirs] root");
}

#[tokio::test(flavor = "current_thread")]
async fn watcher_ignores_sibling_write_with_dirs_declared() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("tau.toml")
        .write_str(
            r#"
packages = ["mock-llm"]

[project]
name = "watcher-dirs-sibling-test"

[dirs]
tools = "tools"

[models]
mock-1 = { backend = "mock-llm", model = "claude-haiku-4-5" }

[agents.a]
display_name = "A"
package      = "agent-a@^0.1"
model        = "mock-1"
prompt.system = "x"
"#,
        )
        .expect("write");
    tmp.child("tools/existing_tool.toml")
        .write_str("mcp = \"https://mcp.example.com\"\n")
        .expect("write pre-existing tool");

    let session = tau_cli::cmd::dev::session::DevSession::load(tmp.path().to_path_buf(), None)
        .await
        .expect("load");

    // Write tau-lock.toml — a sibling of `tau.toml`, NOT under the `[dirs]`
    // root. With `[dirs]` declared and its event-filter path active, an
    // unrelated sibling write anywhere in the project must still be
    // rejected (Minor finding from the Task 9 review).
    tmp.child("tau-lock.toml")
        .write_str(
            r#"schema_version = 7
created_at = "2026-06-10T00:00:00Z"
tau_version = "0.0.0"
packages = []
"#,
        )
        .expect("write lock");

    tokio::time::sleep(Duration::from_millis(700)).await;

    assert!(
        !session.pending_reload.load(Ordering::Acquire),
        "tau-lock.toml changes must not trigger reload even with [dirs] declared"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn watcher_ignores_tau_lock_changes() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("tau.toml")
        .write_str(
            r#"
packages = ["mock-llm"]

[project]
name = "watcher-test"

[models]
mock-1 = { backend = "mock-llm", model = "claude-haiku-4-5" }

[agents.a]
display_name = "A"
package      = "agent-a@^0.1"
model        = "mock-1"
prompt.system = "x"
"#,
        )
        .expect("write");

    let session = tau_cli::cmd::dev::session::DevSession::load(tmp.path().to_path_buf(), None)
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
