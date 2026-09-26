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
//! [`CodexDriver`] is `impl Driver` over decode → flights → run → settle,
//! with nothing of its own to decide: every `stop`, every bill, and every
//! rung of the ladder is the shared module's.
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
//! Every mapping below was recorded by #128 on 0.157.1 and is tested against
//! the committed transcripts under `drivers/tests/cassettes/cli/codex-0.157.1/`.
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
use std::sync::Arc;

use serde_json::Value;
use tau_kernel::abi::{Budget as KernelBudget, Consumption, Corr};
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{BoxFuture, Delivery};

use super::process::{self, Interrupt, Invocation, Run};
use super::wire::{self, Caps, ErrorKind, Reply, Stop, Truncated, Usage, VERSION};
use super::{
    decode, encode, envelope, probe_login, probe_version, refusal, refused, settle, Accepted,
    AgentConfig, Availability, ConfigError, Flights, LoginOutput, Outcome, Probe, Verdict,
};

/// What `codex` can honour of the wire at 0.157.1: no per-session tool
/// allowlist (the cage is `-s` in the configuration), no cost or turn
/// bound (no flag), and `resume` — `exec resume` grew `--json` and
/// `--output-schema` since 0.46.0 (#128 run 4).
pub const CAPS: Caps = Caps {
    tools: false,
    budget: false,
    resume: true,
};

/// The worker contract (ADR-0013 §4), versioned with [`VERSION`] and
/// prepended to the task: `exec` has no system-prompt flag.
///
/// The same text the `claude` adapter appends to the system prompt; the
/// `Cli` trait extraction hoists it.
pub const CONTRACT: &str = "# tau worker contract v1
You are a headless worker spawned by the tau kernel. Rules:
- Never ask questions. If something is ambiguous, make the conservative assumption and record it.
- Stay inside the working directory you were started in.
- If the task is too large to finish, stop, and report what is done and what is left with status \"partial\".
- When you finish, your final message must be exactly one JSON object and nothing else (no prose, no code fences):
{\"status\":\"ok | partial | failed | cancelled\",\"summary\":\"<= 3 sentences\",\"artifacts\":[{\"path\":\"relative/to/workspace\",\"kind\":\"file | patch | report\"}],\"assumptions\":[\"...\"],\"events\":[\"notable decisions\"],\"error\":null}";

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
/// bills the ceiling regardless of what is here.
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
/// Unauthorized` is #130 §3's; the rest unpinned, read tolerantly.
fn names_auth(lowered: &str) -> bool {
    [
        "401",
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
        "429",
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
/// ends however it ends.
struct SchemaFile {
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

// --- the driver ---------------------------------------------------------------

/// The `codex` agent driver: one registration, one tool, one CLI binary
/// under one login the driver never sees. Cheap to clone; every clone shares
/// one flight registry, and dropping the last clone abandons every open run.
#[derive(Clone)]
pub struct CodexDriver {
    inner: Arc<Inner>,
}

struct Inner {
    shared: Arc<Shared>,
    flights: Arc<Flights>,
    ceiling: KernelBudget,
    schema: ToolSchema,
}

/// What a run thread needs, and nothing that would keep [`Inner`] alive: a
/// run holds this, not `Inner`, so that dropping the last driver handle
/// runs `Inner`'s `Drop` while runs are still open.
struct Shared {
    config: AgentConfig,
    version: String,
    probe: Probe,
    availability: Availability,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // A harness shutting down leaves no CLI behind (ADR-0013 §5).
        self.flights.abandon_all();
    }
}

impl CodexDriver {
    /// Builds the driver: checks the config, runs the version probe, and
    /// runs the login probe once (ADR-0013 §6).
    ///
    /// # Errors
    ///
    /// [`ConfigError`] for a config the host cannot honour, a binary that
    /// cannot be run, or a version that does not match the pin. **Not** for
    /// a CLI that is logged out: that is a runtime state a human changes,
    /// and every `send` until then answers `error.unavailable`.
    pub fn new(config: AgentConfig) -> Result<Self, ConfigError> {
        config.check()?;
        let probe = probe();
        let version = probe_version(&config, &probe)?;
        let availability = Availability::new(probe_login(&config, &probe));
        let ceiling = config.ceiling();
        let schema = ToolSchema {
            description: config.describe(CAPS),
            input_schema: serde_json::to_vec(&wire::schema(CAPS)).unwrap_or_default(),
        };
        Ok(Self {
            inner: Arc::new(Inner {
                shared: Arc::new(Shared {
                    config,
                    version,
                    probe,
                    availability,
                }),
                flights: Arc::new(Flights::default()),
                ceiling,
                schema,
            }),
        })
    }

    /// The ceiling to register this driver with. See
    /// [`AgentConfig::ceiling`].
    #[must_use]
    pub fn ceiling(&self) -> KernelBudget {
        self.inner.ceiling.clone()
    }

    /// The config this driver was built from.
    #[must_use]
    pub fn config(&self) -> &AgentConfig {
        &self.inner.shared.config
    }

    /// What the binary printed at construction: the reply's `cli.version`.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.inner.shared.version
    }

    /// The login verdict as it stands, without probing.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        self.inner.shared.availability.verdict()
    }

    /// How many runs are open right now.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.inner.flights.in_flight()
    }
}

impl std::fmt::Debug for CodexDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexDriver")
            .field("config", &self.inner.shared.config)
            .field("version", &self.inner.shared.version)
            .field("in_flight", &self.in_flight())
            .finish_non_exhaustive()
    }
}

impl Driver for CodexDriver {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        // Registered now, not when the future is first polled: `abandon`
        // may arrive in between, and it must find the entry.
        let (flight, abandoned_early) = self.inner.flights.enter(request.corr);
        let shared = Arc::clone(&self.inner.shared);
        if abandoned_early {
            drop(flight);
            let reply = abandoned(&shared.config, &shared.version);
            return Box::pin(async move { (encode(&reply), Consumption::none()) });
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let on_thread = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("tau-agent-codex".to_owned())
            .spawn(move || {
                let flight = flight;
                let (reply, consumed) =
                    on_thread.run(request.corr, &request.payload, flight.cancel());
                drop(flight);
                let _ = tx.send((encode(&reply), consumed));
            });
        match spawned {
            Ok(_) => Box::pin(async move {
                rx.await.unwrap_or_else(|_| {
                    let reply = refused(
                        &shared.config,
                        &shared.version,
                        refusal(ErrorKind::Host, "the run thread ended without a reply"),
                    );
                    (encode(&reply), Consumption::none())
                })
            }),
            Err(e) => {
                let reply = refused(
                    &shared.config,
                    &shared.version,
                    refusal(ErrorKind::Host, format!("cannot start a run thread: {e}")),
                );
                Box::pin(async move { (encode(&reply), Consumption::none()) })
            }
        }
    }

    fn describe(&self) -> Option<ToolSchema> {
        Some(self.inner.schema.clone())
    }

    fn abandon(&self, corr: Corr) {
        self.inner.flights.abandon(corr);
    }
}

impl Shared {
    /// One `send`, on its own thread: decode, the availability check, the
    /// schema file, the run, the read-back, the settlement.
    fn run(&self, corr: Corr, payload: &[u8], cancel: &process::Cancel) -> (Reply, Consumption) {
        let config = &self.config;
        let accepted = match decode(payload, config, CAPS) {
            Ok(accepted) => accepted,
            Err(error) => return (refused(config, &self.version, error), Consumption::none()),
        };
        // Refused while logged out, and re-probed once per refused `send`
        // (ADR-0013 §6): never a retry loop, never a login attempt. For
        // `codex` this is the gate that works: an unsigned run retries on
        // 401 and never reports "not authenticated" on its own (#130 §3).
        let mode = match self.availability.check(config, &self.probe) {
            Verdict::Unavailable { message } => {
                return (
                    refused(
                        config,
                        &self.version,
                        refusal(ErrorKind::Unavailable, message),
                    ),
                    Consumption::none(),
                )
            }
            Verdict::Ready { mode } => mode,
        };
        let host = |what: String| {
            (
                refused(config, &self.version, refusal(ErrorKind::Host, what)),
                Consumption::none(),
            )
        };
        let schema = match SchemaFile::write(corr) {
            Ok(schema) => schema,
            Err(e) => return host(format!("cannot write the envelope schema: {e}")),
        };
        let invocation = invocation(config, &accepted, schema.path());
        let run = match process::run(&invocation, config.bounds(), cancel) {
            Ok(run) => run,
            Err(e) => {
                return host(format!("cannot start `{}`: {e}", config.binary.display()));
            }
        };
        drop(schema);
        let mut outcome = outcome(&run);
        outcome.mode = mode;
        if let Some(message) = auth_failure(&run) {
            // The run's own events say the CLI is logged out, whatever the
            // probe said earlier.
            self.availability.fail(message);
        }
        settle(config, &self.version, &run, &outcome)
    }
}

/// The reply for an abandon that arrived before anything was spawned: the
/// first row of the ladder table — `abandoned`, zeros, nothing billed.
fn abandoned(config: &AgentConfig, version: &str) -> Reply {
    Reply {
        v: VERSION,
        stop: Stop::Abandoned,
        envelope: None,
        session: None,
        cli: wire::Cli {
            name: config.name.clone(),
            version: version.to_owned(),
        },
        model: None,
        mode: None,
        usage: Usage::default(),
        transcript: Vec::new(),
        truncated: Truncated::default(),
    }
}
