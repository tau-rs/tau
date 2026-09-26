//! What the agent tests share: the ADR-0013 §9 configuration, a scratch
//! directory that cleans up after itself, `/bin/sh` scripts standing in
//! for a CLI, and `tau-fake-cli` replaying a script — or a committed #130
//! transcript turned into one.
//!
//! Reached by `#[path]` rather than through `common/mod.rs`, so that a build
//! with only the `agent` feature does not have to compile the stub HTTP
//! server and its runtime.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
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

/// The scripted stand-in for an agent CLI, cargo-built alongside this test
/// binary (ADR-0013 §10). See its module doc for the script format.
pub(crate) const FAKE_CLI: &str = env!("CARGO_BIN_EXE_tau-fake-cli");

/// The environment variable `tau-fake-cli` reads its script's path from.
pub(crate) const SCRIPT_VAR: &str = "TAU_FAKE_CLI_SCRIPT";

/// The whole environment a fake run gets: its script, and what the build's
/// own instrumentation needs forwarded, for the reasons `common/sandbox.rs`
/// gives for the shim — the driver hands the child exactly this list.
pub(crate) fn fake_env(script: &Path) -> Vec<(String, String)> {
    let mut env = vec![(SCRIPT_VAR.to_owned(), script.display().to_string())];
    for forwarded in ["LLVM_PROFILE_FILE", "ASAN_OPTIONS"] {
        if let Ok(value) = std::env::var(forwarded) {
            env.push((forwarded.to_owned(), value));
        }
    }
    env
}

/// `tau-fake-cli` replaying `script`, as an invocation.
pub(crate) fn fake(script: &Path, cwd: &Path, terminal: fn(&[u8]) -> bool) -> Invocation {
    Invocation {
        program: PathBuf::from(FAKE_CLI),
        args: Vec::new(),
        cwd: cwd.to_path_buf(),
        env: fake_env(script),
        first_stdin: None,
        interrupt: Interrupt::Signal,
        terminal,
    }
}

/// The ADR-0013 §9 registration, with `binary` pointed at `tau-fake-cli`
/// replaying `script`.
pub(crate) fn fake_config(script: &Path, root: impl Into<PathBuf>) -> AgentConfig {
    let mut config = adr_config(root);
    config.binary = PathBuf::from(FAKE_CLI);
    config.env = fake_env(script);
    config
}

/// Writes `directives` as a JSONL script under `dir`; returns its path.
pub(crate) fn script(dir: &Path, name: &str, directives: &[Value]) -> PathBuf {
    let path = dir.join(format!("{name}.jsonl"));
    let mut text = String::new();
    for directive in directives {
        text.push_str(&directive.to_string());
        text.push('\n');
    }
    std::fs::write(&path, text).unwrap();
    path
}

/// The committed CLI transcripts (`drivers/tests/cassettes/cli/README.md`).
pub(crate) fn transcripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("cassettes")
        .join("cli")
}

/// The records of one committed transcript, e.g. `("claude-2.1.272",
/// "2-stdin-cancel")`.
pub(crate) fn transcript(pin: &str, run: &str) -> Vec<Value> {
    let path = transcripts_dir().join(pin).join(format!("{run}.jsonl"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The lines the CLI printed in a transcript, in order.
pub(crate) fn transcript_stdout(records: &[Value]) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record["tau"] == "stdout")
        .map(|record| record["line"].clone())
        .collect()
}

/// The stimulus a transcript's runner applied after the task, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Stimulus {
    /// A second stdin line whose `request.subtype` is `interrupt`.
    Interrupt,
    /// This signal.
    Signal(String),
}

/// Turns a committed transcript into a `tau-fake-cli` script, delays
/// dropped: what the CLI printed before the runner's stimulus is the
/// timeline; what it printed after, and its exit status, is the reaction to
/// that stimulus. A run without a stimulus is a timeline ending in its
/// exit. The first stdin record is the task and is not scripted: it arrives
/// through `Invocation::first_stdin`.
pub(crate) fn script_from_transcript(records: &[Value]) -> (Vec<Value>, Option<Stimulus>) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut stimulus = None;
    let mut exit = None;
    let mut task_seen = false;
    for record in records {
        match record["tau"].as_str().unwrap() {
            "stdout" => {
                let line = json!({ "line": record["line"] });
                if stimulus.is_some() {
                    after.push(record["line"].clone());
                } else {
                    before.push(line);
                }
            }
            "stdin" if !task_seen => task_seen = true,
            "stdin" => {
                assert_eq!(
                    record["line"]["request"]["subtype"], "interrupt",
                    "the only scripted stdin stimulus is the interrupt"
                );
                stimulus = Some(Stimulus::Interrupt);
            }
            "signal" => {
                stimulus = Some(Stimulus::Signal(
                    record["name"].as_str().unwrap().to_owned(),
                ))
            }
            "exit" => exit = Some(record["code"].clone()),
            _ => {}
        }
    }
    let mut directives = before;
    match &stimulus {
        None => directives.push(json!({ "exit": exit })),
        Some(Stimulus::Interrupt) => directives.push(json!({
            "on": { "stdin": "\"subtype\":\"interrupt\"" },
            "lines": after,
            "exit": exit,
        })),
        Some(Stimulus::Signal(name)) => directives.push(json!({
            "on": { "signal": name },
            "lines": after,
            "exit": exit,
        })),
    }
    (directives, stimulus)
}

/// The interrupt `claude` acknowledges (#130 §5a), as the driver writes it.
pub(crate) const INTERRUPT: &str =
    "{\"type\":\"control_request\",\"request_id\":\"tau-cancel-1\",\"request\":{\"subtype\":\"interrupt\"}}\n";

// --- a `claude` the driver can construct ------------------------------------

/// The environment variable the stub reads its login marker from: the file
/// exists, the stub is "logged in".
pub(crate) const LOGIN_VAR: &str = "TAU_TEST_LOGIN";
/// Where the stub appends one byte per login probe it answers.
pub(crate) const PROBES_VAR: &str = "TAU_TEST_PROBES";
/// Where the stub writes the argv of the last run it handed to the fake,
/// NUL-separated (the worker contract spans lines).
pub(crate) const ARGV_VAR: &str = "TAU_TEST_ARGV";
/// When set, a run prints this on stderr and exits 1 before the fake ever
/// starts: `codex`'s refusal of an unknown thread id (#128 run 7).
pub(crate) const REFUSE_VAR: &str = "TAU_TEST_REFUSE";

/// A stand-in `claude` binary the driver can be *constructed* over.
///
/// `tau-fake-cli` replays its script whatever its argv, so it cannot answer
/// `--version` or `auth status` on its own. This `/bin/sh` wrapper answers
/// both the way #130 recorded them and `exec`s the fake for everything
/// else, recording the argv it was given. `TAU_TEST_LOGIN` names a marker
/// file: present, the stub is logged in (exit 0, the JSON #130 saw, minus
/// the email); absent, it is logged out (exit 1, a line on stderr).
pub(crate) struct ClaudeStub {
    pub(crate) binary: PathBuf,
    pub(crate) login: PathBuf,
    pub(crate) probes: PathBuf,
    pub(crate) argv: PathBuf,
}

impl ClaudeStub {
    pub(crate) fn new(dir: &Path) -> Self {
        let binary = dir.join("claude");
        let body = format!(
            "#!/bin/sh\n\
             case \"$1\" in\n\
               --version) echo \"2.1.272 (Claude Code)\"; exit 0 ;;\n\
               auth)\n\
                 printf . >> \"${PROBES_VAR}\"\n\
                 if [ -f \"${LOGIN_VAR}\" ]; then\n\
                   echo '{{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"apiProvider\":\"firstParty\"}}'\n\
                   exit 0\n\
                 fi\n\
                 echo 'Not logged in. Run `claude auth login` first.' >&2\n\
                 exit 1 ;;\n\
             esac\n\
             printf '%s\\0' \"$@\" > \"${ARGV_VAR}\"\n\
             exec \"{fake}\" \"$@\"\n",
            fake = FAKE_CLI,
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

    /// The ADR-0013 §9 registration over this stub, replaying `script`,
    /// logged in, rooted at `root`.
    pub(crate) fn config(&self, script: &Path, root: impl Into<PathBuf>) -> AgentConfig {
        self.set_logged_in(true);
        let mut config = adr_config(root);
        config.binary = self.binary.clone();
        config.permission = Some("acceptEdits".to_owned());
        config.env = fake_env(script);
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

/// A stand-in `codex` binary the driver can be *constructed* over.
///
/// `tau-fake-cli` replays its script whatever its argv, so it cannot answer
/// `--version` or `login status` on its own. This `/bin/sh` wrapper answers
/// both the way #128 recorded them at 0.154.0 — the version on stdout, the
/// login line on **stderr** with nothing on stdout — and `exec`s the fake
/// for everything else, recording the argv it was given. `TAU_TEST_LOGIN`
/// names a marker file: present, the stub is logged in (exit 0); absent, it
/// is logged out (exit 1, #130's `Error checking login status` line).
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
               --version) echo \"codex-cli 0.154.0\"; exit 0 ;;\n\
               login)\n\
                 printf . >> \"${PROBES_VAR}\"\n\
                 if [ -f \"${LOGIN_VAR}\" ]; then\n\
                   echo 'Logged in using ChatGPT' >&2\n\
                   exit 0\n\
                 fi\n\
                 echo 'Error checking login status: No such file or directory (os error 2)' >&2\n\
                 exit 1 ;;\n\
             esac\n\
             printf '%s\\0' \"$@\" > \"${ARGV_VAR}\"\n\
             if [ -n \"${REFUSE_VAR}\" ]; then printf '%s\\n' \"${REFUSE_VAR}\" >&2; exit 1; fi\n\
             exec \"{fake}\" \"$@\"\n",
            fake = FAKE_CLI,
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

    /// The ADR-0013 §9 registration over this stub, replaying `script`,
    /// logged in, rooted at `root`, named `codex` with the `workspace-write`
    /// sandbox #128 recorded under.
    pub(crate) fn config(&self, script: &Path, root: impl Into<PathBuf>) -> AgentConfig {
        self.set_logged_in(true);
        let mut config = adr_config(root);
        config.name = "codex".to_owned();
        config.description = None;
        config.tools = Vec::new();
        config.task_cost_microusd = Some(2_000_000);
        config.task_turns = None;
        config.binary = self.binary.clone();
        config.permission = Some("workspace-write".to_owned());
        config.env = fake_env(script);
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

/// Polls until `path` exists, or about five seconds pass.
///
/// For a script with a `touch` directive: the fake installs its signal
/// handlers before its first timeline step, so once the marker is there a
/// signal is caught, however slowly the binary started under load — which
/// is what a test that climbs past the first rung must know before it
/// abandons the run (#182 is the shape of the flake this prevents).
pub(crate) fn wait_for(path: &Path) {
    for _ in 0..500 {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{} never appeared", path.display());
}

/// The `run` payload the tests send: the ADR-0013 §2 shape, task only.
pub(crate) fn run_payload(task: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "op": "run", "task": task })).unwrap()
}

/// The `resume` payload: a session and the amendment.
pub(crate) fn resume_payload(session: &str, task: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "op": "resume", "session": session, "task": task })).unwrap()
}

/// A script that prints a scrubbed #130 transcript's stdout and exits as
/// it did — `1-hello` is the happy path, `6-max-turns-1` the turn bound.
pub(crate) fn replay_script(dir: &Path, pin: &str, run: &str) -> PathBuf {
    let records = transcript(pin, run);
    let (directives, _) = script_from_transcript(&records);
    script(dir, run, &directives)
}

/// A one-run script: `init` with `session_id`, then this `result` line,
/// then `exit` with `code`. For the rows no #130 transcript reaches.
pub(crate) fn synthetic_script(dir: &Path, name: &str, result: Value, code: i64) -> PathBuf {
    script(
        dir,
        name,
        &[
            json!({ "line": {
                "type": "system", "subtype": "init",
                "session_id": "11111111-2222-3333-4444-555555555555",
                "model": "claude-opus-5[1m]", "apiKeySource": "none",
                "permissionMode": "acceptEdits",
            }}),
            json!({ "line": result }),
            json!({ "exit": code }),
        ],
    )
}

/// A line the fake would print only after a minute: a CLI that is still
/// working. A child with no stdin to wait on exits as soon as its timeline
/// is spent, so a script that must still be running when a signal arrives
/// keeps one of these pending.
pub(crate) fn still_working() -> Value {
    json!({ "line": { "type": "never.printed" }, "delay_ms": 60_000 })
}
