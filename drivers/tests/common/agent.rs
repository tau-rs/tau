//! What the agent tests share: the ADR-0013 §9 configuration, a scratch
//! directory that cleans up after itself, and `/bin/sh` scripts standing in
//! for a CLI.
//!
//! Reached by `#[path]` rather than through `common/mod.rs`, so that a build
//! with only the `agent` feature does not have to compile the stub HTTP
//! server and its runtime.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tau_drivers::agent::process::{Bounds, Cancel, Interrupt, Invocation};
use tau_drivers::agent::AgentConfig;

/// The ADR-0013 §9 registration: Claude Code over a workspace, three tools,
/// two dollars, 400k tokens, forty turns.
pub(crate) fn adr_config(root: impl Into<PathBuf>) -> AgentConfig {
    let mut config = AgentConfig::new("claude", "/bin/sh", root, 400_000, Duration::from_secs(900));
    config.description =
        Some("Delegate a whole task to a Claude Code session in the shared workspace.".to_owned());
    config.tools = ["Read", "Edit", "Bash"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    config.task_cost_microusd = Some(2_000_000);
    config.task_turns = Some(40);
    config
}

/// A fresh directory, removed on drop.
pub(crate) struct Temp {
    pub(crate) dir: PathBuf,
}

static SEQ: AtomicU64 = AtomicU64::new(0);

impl Temp {
    pub(crate) fn new(what: &str) -> Self {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("tau-agent-{what}-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `/bin/sh -c <script>` as an invocation, with `line` deciding which
/// printed line is the CLI's terminal event.
pub(crate) fn sh(script: &str, cwd: &Path, terminal: fn(&[u8]) -> bool) -> Invocation {
    Invocation {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_owned(), script.to_owned()],
        cwd: cwd.to_path_buf(),
        env: vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())],
        first_stdin: None,
        interrupt: Interrupt::Signal,
        terminal,
    }
}

/// A line is the terminal event when it mentions `"type":"result"`, as
/// `claude`'s does (#130 run 1).
pub(crate) fn result_line(line: &[u8]) -> bool {
    String::from_utf8_lossy(line).contains("\"type\":\"result\"")
}

/// Nothing is ever terminal.
pub(crate) fn never(_: &[u8]) -> bool {
    false
}

/// Bounds with a short wall and grace, for tests that climb the ladder.
pub(crate) fn bounds(wall_ms: u64, grace_ms: u64) -> Bounds {
    Bounds {
        wall: Duration::from_millis(wall_ms),
        grace: Duration::from_millis(grace_ms),
        transcript_bytes: 64 * 1024,
        stderr_bytes: 8 * 1024,
    }
}

/// Abandons a run once the child has had time to install its signal
/// handlers.
///
/// An abandon that lands in the microseconds between `spawn` and the
/// child's own `trap` kills it at the first rung — correct, and the row the
/// flight registry's `abandoned_early` covers, but not the ladder this is
/// testing.
pub(crate) fn abandon_after(cancel: &Arc<Cancel>, ms: u64) {
    let cancel = Arc::clone(cancel);
    drop(std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(ms));
        cancel.abandon();
    }));
}
