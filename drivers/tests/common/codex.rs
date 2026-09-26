//! What the `codex` adapter's tests share, on top of `common/agent.rs`: a
//! stand-in `codex` binary the driver can be *constructed* over, scripts
//! from the committed 0.157.1 transcripts, and the request payloads.
//!
//! Reached by `#[path]`, like `common/agent.rs`, so that a build with only
//! the agent features does not compile the stub HTTP server.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tau_drivers::agent::AgentConfig;

use super::agent;

/// The pin the transcripts under `cassettes/cli/` were recorded at.
pub(crate) const PIN: &str = "codex-0.157.1";
/// The thread run 1 started, and runs 4 and 5 resumed.
pub(crate) const THREAD_HELLO: &str = "01a0dd5a-ff32-7853-90d2-7362de945134";
/// What `codex --version` printed at the pin.
pub(crate) const VERSION: &str = "codex-cli 0.157.1";
/// What `codex login status` printed, on stderr, signed in (run 0).
pub(crate) const MODE: &str = "Logged in using ChatGPT";

/// The environment variable the stub reads its login marker from: the file
/// exists, the stub is "logged in".
pub(crate) const LOGIN_VAR: &str = "TAU_TEST_LOGIN";
/// Where the stub appends one byte per login probe it answers.
pub(crate) const PROBES_VAR: &str = "TAU_TEST_PROBES";
/// Where the stub writes the argv of the last run it handed to the fake,
/// NUL-separated (the worker contract spans lines).
pub(crate) const ARGV_VAR: &str = "TAU_TEST_ARGV";
/// Set, the stub refuses every run before starting one: that text on
/// stderr, nothing on stdout, exit 1 — the shape of `exec resume` on a
/// thread the CLI has no rollout for (run 9).
pub(crate) const REFUSE_VAR: &str = "TAU_TEST_REFUSE";

/// A stand-in `codex` binary the driver can be *constructed* over.
///
/// `tau-fake-cli` replays its script whatever its argv, so it cannot answer
/// `--version` or `login status` on its own. This `/bin/sh` wrapper answers
/// both the way #128 recorded them — the version on stdout, the mode line
/// on **stderr** — and `exec`s the fake for everything else, recording the
/// argv it was given. `TAU_TEST_LOGIN` names a marker file: present, the
/// stub is logged in (exit 0); absent, it is logged out (exit 1, a line on
/// stderr). Before handing a run to the fake it reads its stdin to end of
/// file, as `codex exec` does before it starts (run 8): a driver that held
/// the pipe open would hold the stub too.
pub(crate) struct CodexStub {
    pub(crate) binary: PathBuf,
    pub(crate) login: PathBuf,
    pub(crate) probes: PathBuf,
    pub(crate) argv: PathBuf,
}

impl CodexStub {
    pub(crate) fn new(dir: &Path) -> Self {
        let binary = dir.join("codex");
        let body = format!(
            "#!/bin/sh\n\
             case \"$1\" in\n\
               --version) echo \"{VERSION}\"; exit 0 ;;\n\
               login)\n\
                 printf . >> \"${PROBES_VAR}\"\n\
                 if [ -f \"${LOGIN_VAR}\" ]; then\n\
                   echo '{MODE}' >&2\n\
                   exit 0\n\
                 fi\n\
                 echo 'Not logged in. Run `codex login` first.' >&2\n\
                 exit 1 ;;\n\
             esac\n\
             printf '%s\\0' \"$@\" > \"${ARGV_VAR}\"\n\
             if [ -n \"${REFUSE_VAR}\" ]; then printf '%s\\n' \"${REFUSE_VAR}\" >&2; exit 1; fi\n\
             cat > /dev/null\n\
             prev=''\n\
             for a in \"$@\"; do\n\
               if [ \"$prev\" = --output-schema ]; then cp \"$a\" \"${ARGV_VAR}.schema\"; fi\n\
               prev=\"$a\"\n\
             done\n\
             exec \"{fake}\" \"$@\"\n",
            fake = agent::FAKE_CLI,
        );
        std::fs::write(&binary, body).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            binary,
            login: dir.join("logged-in"),
            probes: dir.join("probes"),
            argv: dir.join("argv"),
        }
    }

    /// Marks the stub logged in, or out.
    pub(crate) fn set_logged_in(&self, logged_in: bool) {
        if logged_in {
            std::fs::write(&self.login, "").unwrap();
        } else {
            let _ = std::fs::remove_file(&self.login);
        }
    }

    /// How many login probes the stub has answered.
    pub(crate) fn probes(&self) -> usize {
        std::fs::read_to_string(&self.probes)
            .map(|s| s.len())
            .unwrap_or(0)
    }

    /// The schema file the last run was pointed at, as the CLI would have
    /// read it, copied by the stub before the file was removed.
    pub(crate) fn schema_seen(&self) -> Option<Value> {
        let text = std::fs::read(format!("{}.schema", self.argv.display())).ok()?;
        serde_json::from_slice(&text).ok()
    }

    /// The argv of the last run.
    pub(crate) fn argv(&self) -> Vec<String> {
        std::fs::read_to_string(&self.argv)
            .map(|s| {
                s.split('\0')
                    .filter(|a| !a.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A `codex` registration over this stub, replaying `script`, logged
    /// in, rooted at `root`: the ADR-0013 §9 bounds, `workspace-write` as
    /// the cage, no tools (the cage is the configuration), no cost bound
    /// (no flag), 400k tokens.
    pub(crate) fn config(&self, script: &Path, root: impl Into<PathBuf>) -> AgentConfig {
        self.set_logged_in(true);
        let mut config = AgentConfig::new(
            "codex",
            self.binary.clone(),
            root,
            400_000,
            Duration::from_secs(900),
        );
        config.permission = Some("workspace-write".to_owned());
        config.env = agent::fake_env(script);
        config
            .env
            .push((LOGIN_VAR.to_owned(), self.login.display().to_string()));
        config
            .env
            .push((PROBES_VAR.to_owned(), self.probes.display().to_string()));
        config
            .env
            .push((ARGV_VAR.to_owned(), self.argv.display().to_string()));
        config
    }
}

/// The committed transcript `run` of the pin, as a `tau-fake-cli` script
/// written under `dir`.
pub(crate) fn replay_script(dir: &Path, pin: &str, run: &str) -> PathBuf {
    let records = agent::transcript(pin, run);
    let (directives, _) = agent::script_from_transcript(&records);
    agent::script(dir, run, &directives)
}

/// A script that prints `thread.started`, `turn.started`, then `terminal`,
/// and exits with `code`: the shape of a run whose only interesting event
/// is how it ended.
pub(crate) fn synthetic_script(dir: &Path, name: &str, terminal: Value, code: i32) -> PathBuf {
    agent::script(
        dir,
        name,
        &[
            json!({ "line": { "type": "thread.started", "thread_id": "t-synthetic" } }),
            json!({ "line": { "type": "turn.started" } }),
            json!({ "line": terminal }),
            json!({ "exit": code }),
        ],
    )
}

/// A `turn.failed` script whose error says `message`.
pub(crate) fn failed_script(dir: &Path, name: &str, message: &str) -> PathBuf {
    synthetic_script(
        dir,
        name,
        json!({ "type": "turn.failed", "error": { "message": message } }),
        1,
    )
}

/// `{ "op": "run", "task": … }`, as bytes.
pub(crate) fn run_payload(task: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "op": "run", "task": task })).unwrap()
}

/// `{ "op": "resume", "session": …, "task": … }`, as bytes.
pub(crate) fn resume_payload(session: &str, task: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "op": "resume", "session": session, "task": task })).unwrap()
}

/// Blocks until `path` exists, or panics after ten seconds: the fake's
/// readiness marker (`touch`).
pub(crate) fn wait_for(path: &Path) {
    for _ in 0..1_000 {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{} never appeared", path.display());
}
