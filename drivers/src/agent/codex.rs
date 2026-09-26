//! The `codex` adapter (ADR-0013 §7, the `codex` column): `codex exec
//! --json` as a subprocess behind one `send`.
//!
//! Everything CLI-specific about `codex` 0.157.1 is in this file, and it is
//! exactly the two halves of the seam the shared module leaves open:
//!
//! - [`invocation`] builds the [`Invocation`] — the fixed argv, the worker
//!   contract prepended to the task as the positional prompt, `SIGINT` as
//!   the interrupt, and which printed line is the terminal `turn.*` event;
//! - [`outcome`] reads the [`Outcome`] back out of the lines the run kept —
//!   the thread id from `thread.started`, the usage from `turn.completed`,
//!   the final message from the last `agent_message` item, an error kind
//!   from `turn.failed`.
//!
//! [`Codex`] is the `impl Cli` that hands those two functions to the
//! shared [`AgentDriver`], and [`CodexDriver`] is that driver over it,
//! with nothing of its own to decide: every `stop`, every bill, and every
//! rung of the ladder is the shared module's. Two habits are its own,
//! and the trait has a method for each: the schema file below is the run's
//! [`Cli::Scratch`], and the reply's `mode` is read from the login verdict
//! rather than from an event, because no `--json` event states it.
//!
//! ```text
//! payload ──decode──▶ Accepted ──invocation──▶ codex -a never exec --json … ──▶ lines
//!                                                                            │
//!                                                                outcome ◀───┘
//!                                                                   │
//!                                                    settle ◀───────┘ ──▶ (Reply, Consumption)
//! ```
//!
//! # What is pinned here, and what is not
//!
//! Every mapping below was recorded by #128 and #223 on 0.157.1 and is
//! tested against the committed transcripts under
//! `drivers/tests/cassettes/cli/codex-0.157.1/`. Two of them are the CLI
//! saying nothing on stdout: `exec` reads a piped stdin to end of file
//! before it starts (run 8), so the supervisor gives it none; and a
//! `resume` of a thread the CLI has no rollout for exits 1 with the reason
//! on stderr (run 9), which is `error.provider` billed at nothing.
//! Two rows the recording could not reach are read tolerantly and marked
//! *#128* in the ADR's amendment: the text a `turn.failed` carries when the
//! CLI is logged out, and when a rate limit or a quota window is hit. Each
//! is read by name — `401`, `unauthorized`, `429`, `rate limit`, `quota` —
//! so the drift job (§10) knows which rows to pin when it first sees one.
//!
//! # The schema file
//!
//! `--output-schema` takes a path, so every run writes the strict envelope
//! schema to a scratch file under the host's temporary directory, named by
//! the request's corr, and removes it when the run ends. A path the CLI can
//! read from inside its sandbox, because the sandbox restricts writes, not
//! reads (#128 run 1 read one from outside the workspace).

use std::path::{Path, PathBuf};

use serde_json::Value;
use tau_kernel::abi::Corr;

use super::driver::sealed::Sealed;
use super::process::{Ending, Interrupt, Invocation, Run};
use super::wire::{Caps, ErrorKind, RunError, Stop, Usage};
use super::{
    envelope, refusal, Accepted, AgentConfig, AgentDriver, Cli, LoginOutput, Outcome, Probe,
    Verdict,
};

pub use super::CONTRACT;

/// What `codex` can honour of the wire at 0.157.1: no per-session tool
/// allowlist (the cage is `-s` in the configuration), no cost or turn
/// bound (no flag), and `resume` — `exec resume` grew `--json` and
/// `--output-schema` since 0.46.0 (#128 run 4).
pub const CAPS: Caps = Caps {
    tools: false,
    budget: false,
    resume: true,
};

/// The probe (ADR-0013 §6, §7): `codex --version` prints `codex-cli
/// 0.157.1`; `codex login status` exits 0 when signed in and prints its one
/// line — `Logged in using ChatGPT` — on **stderr**, nothing on stdout
/// (#128 run 0). That line is the reply's `mode`, verbatim; it names no
/// account.
#[must_use]
pub fn probe() -> Probe {
    Probe {
        version_args: vec!["--version".to_owned()],
        login_args: vec!["login".to_owned(), "status".to_owned()],
        mode_from_login,
    }
}

/// The first non-empty line the login command printed, stderr first
/// because that is where 0.157.1 puts it, stdout for a pin that moves it.
fn mode_from_login(out: &LoginOutput) -> Option<String> {
    out.stderr
        .lines()
        .chain(out.stdout.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

// --- the Invocation half of the seam ------------------------------------------

/// The fixed argv (ADR-0013 §7), then the configuration, then the op.
///
/// `-a never` **before** the subcommand: `exec --ask-for-approval` is
/// rejected (#130), and `--full-auto` is `-a on-failure`, which can still
/// block. `--ignore-user-config` so that the user's `config.toml` — its
/// model, its MCP servers, its instructions — does not reach a
/// kernel-spawned worker (#128 run 7; the login is read regardless).
/// `--skip-git-repo-check` because a workspace need not be a repository.
///
/// Flags go **after** `exec`: a top-level `-m` parses and is silently
/// ignored (#128 probed it). `exec resume` has no `--sandbox` and no `--cd`,
/// so the cage travels as `-c sandbox_mode="…"` and the workspace is the
/// child's `cwd`, which is where a resumed thread works (#128 runs 4, 5).
#[must_use]
pub fn invocation(config: &AgentConfig, accepted: &Accepted, schema: &Path) -> Invocation {
    let mut args: Vec<String> = ["-a", "never", "exec"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let prompt = match &accepted.session {
        Some(session) => {
            args.push("resume".to_owned());
            args.push(session.clone());
            accepted.task.clone()
        }
        None => format!("{CONTRACT}\n\nTask: {}", accepted.task),
    };
    args.extend(
        [
            "--json",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "--output-schema",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    args.push(schema.display().to_string());
    if let Some(permission) = &config.permission {
        if accepted.session.is_some() {
            args.push("-c".to_owned());
            args.push(format!("sandbox_mode={}", toml_string(permission)));
        } else {
            args.push("--sandbox".to_owned());
            args.push(permission.clone());
        }
    }
    if accepted.session.is_none() {
        args.push("--cd".to_owned());
        args.push(accepted.workspace.display().to_string());
    }
    if let Some(model) = &config.model {
        args.push("-m".to_owned());
        args.push(model.clone());
    }
    if let Some(effort) = &config.effort {
        args.push("-c".to_owned());
        args.push(format!("model_reasoning_effort={}", toml_string(effort)));
    }
    args.push(prompt);
    Invocation {
        program: config.binary.clone(),
        args,
        cwd: accepted.workspace.clone(),
        env: config.env.clone(),
        // Nothing is ever written on this CLI's stdin, and the supervisor
        // gives a child like that none: `exec` reads a piped stdin to end of
        // file before it starts (run 8).
        first_stdin: None,
        interrupt: Interrupt::Signal,
        terminal: is_turn_end,
    }
}

/// A `-c key=value` value is parsed as TOML: a string is quoted, with the
/// two characters a basic string cannot carry bare escaped.
fn toml_string(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Whether a printed line is the CLI's terminal event: a JSON object whose
/// `type` is `turn.completed` or `turn.failed`. The substring check first
/// keeps the drain cheap; the parse confirms it, so an agent message that
/// *mentions* one is not mistaken for it.
fn is_turn_end(line: &[u8]) -> bool {
    if !contains(line, b"\"type\":\"turn.") {
        return false;
    }
    serde_json::from_slice::<Value>(line)
        .ok()
        .and_then(|value| value.get("type").and_then(Value::as_str).map(str::to_owned))
        .is_some_and(|kind| kind == "turn.completed" || kind == "turn.failed")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

// --- the Outcome half of the seam ---------------------------------------------

/// Reads the [`Outcome`] out of a finished run (ADR-0013 §7).
///
/// | field | from |
/// |---|---|
/// | `session` | the first `thread.started`'s `thread_id` |
/// | `model` | `None`: no event names it at 0.157.1 |
/// | `mode` | `None` here; the driver fills it from the login probe (§2) |
/// | `usage` | `turn.completed.usage`: `input_tokens` and `output_tokens` as stated — `cached_input_tokens` and `reasoning_output_tokens` are *subsets*, not further classes, so nothing is summed; no cost, no turn count |
/// | `final_message` | the last `item.completed` whose `item.type` is `agent_message`, its `text` |
/// | `stop` | `None` on `turn.completed`; an error kind read from `turn.failed.error.message` |
///
/// A run whose terminal event never arrived gets `stop: None` and whatever
/// `thread.started` said; the shared `settle` reports `error.lost` and
/// bills the ceiling regardless of what is here — except a run that
/// printed nothing at all and exited non-zero, which the driver refuses
/// before asking (run 9, an unknown thread: `error.provider`, nothing
/// billed).
#[must_use]
pub fn outcome(run: &Run) -> Outcome {
    let transcript = run.transcript();
    let mut out = Outcome {
        session: transcript
            .iter()
            .find(|event| event.get("type").and_then(Value::as_str) == Some("thread.started"))
            .and_then(|event| string(event, "thread_id")),
        final_message: transcript
            .iter()
            .rev()
            .filter(|event| event.get("type").and_then(Value::as_str) == Some("item.completed"))
            .filter_map(|event| event.get("item"))
            .find(|item| item.get("type").and_then(Value::as_str) == Some("agent_message"))
            .and_then(|item| string(item, "text"))
            .map(String::into_bytes),
        ..Outcome::default()
    };
    let Some(terminal) = run
        .terminal_line()
        .and_then(|line| serde_json::from_slice::<Value>(line).ok())
    else {
        return out;
    };
    match terminal.get("type").and_then(Value::as_str) {
        Some("turn.completed") => out.usage = usage(&terminal),
        Some("turn.failed") => {
            let said = terminal
                .get("error")
                .and_then(|error| string(error, "message"))
                .unwrap_or_else(|| "the turn failed and the CLI said nothing".to_owned());
            out.stop = Some(Stop::Error(refusal(classify(&said), said)));
        }
        _ => {}
    }
    out
}

/// What a run's `error` events said about authentication, if anything: the
/// text of the first one that names it. An unsigned `codex exec` does not
/// fail fast — it emits `thread.started`, `turn.started`, then retries on
/// `401 Unauthorized` (#130 §3) — so the driver reads this after every run
/// and flips the verdict, whatever the probe said earlier.
#[must_use]
pub fn auth_failure(run: &Run) -> Option<String> {
    run.transcript()
        .iter()
        .filter(|event| {
            matches!(
                event.get("type").and_then(Value::as_str),
                Some("error" | "turn.failed")
            )
        })
        .filter_map(|event| {
            string(event, "message").or_else(|| {
                event
                    .get("error")
                    .and_then(|error| string(error, "message"))
            })
        })
        .find(|message| names_auth(&message.to_ascii_lowercase()))
}

fn string(event: &Value, key: &str) -> Option<String> {
    event.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// `turn.completed.usage`, as stated (ADR-0013 §6, the `codex` cell).
fn usage(terminal: &Value) -> Usage {
    let counts = terminal.get("usage");
    let count = |key: &str| {
        counts
            .and_then(|usage| usage.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Usage {
        input_tokens: count("input_tokens"),
        output_tokens: count("output_tokens"),
        cost_microusd: None,
        turns: None,
    }
}

/// Which error kind a `turn.failed` message is (ADR-0013 §2, §7): the
/// authentication and rate-limit rows are read by name (*#128*), anything
/// else is `provider` — a backend 4xx/5xx, a rejected model, a rejected
/// schema (#128 run 6).
fn classify(said: &str) -> ErrorKind {
    let lowered = said.to_ascii_lowercase();
    if names_auth(&lowered) {
        ErrorKind::Unavailable
    } else if names_rate_limit(&lowered) {
        ErrorKind::Throttled
    } else {
        ErrorKind::Provider
    }
}

/// *#128*: the words a logged-out `codex` is expected to use. `401
/// Unauthorized` is #130 §3's; the rest unpinned, read tolerantly. Never a
/// bare `401`: the messages carry hex request ids, and three digits in one
/// of those would turn a backend error into a logout.
fn names_auth(lowered: &str) -> bool {
    [
        "status: 401",
        "status 401",
        "unauthorized",
        "unauthenticated",
        "not logged in",
        "not authenticated",
        "please log in",
        "please run codex login",
        "token expired",
        "authentication",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

/// *#128*: the words a rate limit or a quota window is expected to use.
/// Unpinned; read tolerantly.
fn names_rate_limit(lowered: &str) -> bool {
    [
        "status: 429",
        "status 429",
        "rate limit",
        "rate_limit",
        "too many requests",
        "quota",
        "usage limit",
        "usage_limit",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

// --- the schema file ----------------------------------------------------------

/// The strict envelope schema on disk for one run, removed when the run
/// ends however it ends: the run's [`Cli::Scratch`].
pub struct SchemaFile {
    path: PathBuf,
}

impl SchemaFile {
    /// Writes the schema under the host's temporary directory, named by the
    /// process and the corr so that parallel runs never share a file.
    fn write(corr: Corr) -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "tau-agent-codex-{}-{}.schema.json",
            std::process::id(),
            corr.get()
        ));
        let text = serde_json::to_vec(&envelope::schema()).map_err(std::io::Error::other)?;
        std::fs::write(&path, text)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SchemaFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// --- the adapter --------------------------------------------------------------

/// The `codex` CLI, as [`AgentDriver`] drives it: the [`Cli`] over this
/// file's two functions. A unit — everything it knows is in the argv and
/// the event mapping, and the one thing it keeps on disk lives exactly one
/// run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Codex;

impl Sealed for Codex {}

impl Cli for Codex {
    const CAPS: Caps = self::CAPS;

    type Scratch = SchemaFile;

    fn probe() -> Probe {
        self::probe()
    }

    /// The schema file, or `error.host` with what the host said; nothing
    /// is spawned and nothing is billed.
    fn scratch(&self, corr: Corr) -> Result<SchemaFile, RunError> {
        SchemaFile::write(corr).map_err(|e| {
            refusal(
                ErrorKind::Host,
                format!("cannot write the envelope schema: {e}"),
            )
        })
    }

    fn invocation(
        &self,
        config: &AgentConfig,
        accepted: &Accepted,
        schema: &SchemaFile,
    ) -> Invocation {
        self::invocation(config, accepted, schema.path())
    }

    /// `mode` is the login probe's line (§2): no `--json` event names it,
    /// and the verdict this run proceeded under is the one that does.
    fn outcome(&self, run: &Run, verdict: &Verdict) -> Outcome {
        let mut out = self::outcome(run);
        out.mode = verdict.mode().map(str::to_owned);
        out
    }

    /// Read from the run's `error` events, not from its `stop`: an unsigned
    /// `codex exec` retries on `401` and may never reach a `turn.failed`
    /// that names it (#130 §3).
    fn auth_failure(&self, run: &Run, _outcome: &Outcome) -> Option<String> {
        self::auth_failure(run)
    }

    /// A run that ended by itself having printed nothing, with a non-zero
    /// exit: the CLI refused before starting a thread. The reason is on
    /// stderr — run 9's `no rollout found for thread id …` — and the kind
    /// is `provider` (ADR-0013 §2, the `session` row), unless the text
    /// names a login or a limit. `None` for every run that printed
    /// anything: a thread that started may have spent a turn, and the
    /// shared `settle` bills it.
    fn refused_before_start(&self, run: &Run) -> Option<RunError> {
        if run.ending != Ending::Completed || !run.lines.is_empty() || run.code() == Some(0) {
            return None;
        }
        let said = run.stderr.trim();
        let message = match (run.code(), said.is_empty()) {
            (Some(code), true) => format!("the CLI exited {code} before starting a thread"),
            (Some(code), false) => {
                format!("the CLI exited {code} before starting a thread: {said}")
            }
            (None, true) => "the CLI was signalled before starting a thread".to_owned(),
            (None, false) => format!("the CLI was signalled before starting a thread: {said}"),
        };
        Some(refusal(classify(&message), message))
    }
}

/// The `codex` agent driver: [`AgentDriver`] over [`Codex`]. One
/// registration, one tool, one CLI binary under one login the driver never
/// sees.
pub type CodexDriver = AgentDriver<Codex>;
