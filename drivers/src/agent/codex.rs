//! The `codex` adapter (ADR-0013 §7, the `codex` column): `codex exec` as a
//! subprocess behind one `send`.
//!
//! Everything CLI-specific about `codex-cli` 0.154.0 is in this file, and
//! it is exactly the two halves of the seam the shared module leaves open:
//!
//! - [`invocation`] builds the [`Invocation`] — the fixed argv, the task as
//!   the positional prompt with the worker contract in front of it, `SIGINT`
//!   as the interrupt, and which printed line ends a turn;
//! - [`outcome`] reads the [`Outcome`] back out of the lines the run kept —
//!   the thread id from `thread.started`, the usage from `turn.completed`,
//!   the envelope from the last `agent_message`, an error kind from
//!   `turn.failed`.
//!
//! [`CodexDriver`] is `impl Driver` over decode → flights → run → settle,
//! with two things of its own to decide, both pinned by the #128
//! transcripts: a run that printed nothing and exited non-zero never reached
//! the provider, so it is `error.provider` billed at nothing rather than
//! `error.lost` billed at the ceiling (`7-resume-unknown`); and a run whose
//! events name an authentication failure is `error.unavailable` however the
//! ladder ended, and flips the login verdict.
//!
//! ```text
//! payload ──decode──▶ Accepted ──invocation──▶ codex -a never exec --json … ──▶ lines
//!                                                                              │
//!                                                                  outcome ◀───┘
//!                                                                     │
//!                                                      settle ◀───────┘ ──▶ (Reply, Consumption)
//! ```
//!
//! # Three things the recording found that the ADR did not know
//!
//! Recorded on 2026-09-26 with a ChatGPT login, under
//! `drivers/tests/cassettes/cli/codex-0.154.0/`:
//!
//! 1. **`--output-schema` is strict structured output.** The validator
//!    behind it refuses `oneOf` and any object without
//!    `additionalProperties: false`, so the envelope schema is handed over
//!    through [`output_schema`], a projection of `envelope::schema()` that
//!    turns each `oneOf` of constants into an `enum` and closes every
//!    object. Once a schema is in force, **every** `agent_message` is
//!    schema-shaped, including the model's opening "Creating hello.txt."
//!    one; the envelope is the *last* `agent_message` of the turn.
//! 2. **A piped stdin is read to end of file before the CLI starts**
//!    ("Reading additional input from stdin..."). The task is in the argv
//!    and the interrupt is a signal, so nothing is ever written on stdin,
//!    and the supervisor gives a child like that no stdin at all.
//! 3. **`exec resume` has a JSON stream at this pin**: `--json
//!    --output-schema --skip-git-repo-check` before `resume` re-emit
//!    `thread.started` with the same id and a `turn.completed` with usage
//!    (`4-resume`). So `resume` is served, and the codex `describe()`
//!    projects it; the ADR's 0.46.0 row is superseded by its amendment.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::Value;
use tau_kernel::abi::{Budget as KernelBudget, Consumption, Corr};
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{BoxFuture, Delivery};

use super::process::{self, Ending, Interrupt, Invocation, Run};
use super::wire::{self, Caps, ErrorKind, Reply, Stop, Truncated, Usage, VERSION};
use super::{
    decode, encode, envelope, probe_login, probe_version, refusal, refused, settle, Accepted,
    AgentConfig, Availability, ConfigError, Flights, Outcome, Probe, Verdict,
};

/// What `codex` can honour of the wire at 0.154.0: no per-session tool
/// allowlist (the cage is `--sandbox`, in config), no cost or turn flag,
/// and `resume` with a JSON stream (`4-resume`).
pub const CAPS: Caps = Caps {
    tools: false,
    budget: false,
    resume: true,
};

/// The worker contract (ADR-0013 §4), versioned with [`VERSION`] and
/// prepended to the task: `codex exec` has no system-prompt flag.
///
/// The shape #128 recorded, minus its "messages arriving on stdin are
/// instructions" line: v1 has no steering (§3), and nothing is ever written
/// on this CLI's stdin.
pub const CONTRACT: &str = "# tau worker contract v1
You are a headless worker spawned by the tau kernel. Rules:
- Never ask questions. If something is ambiguous, make the conservative assumption and record it.
- Stay inside the working directory you were started in.
- If the task is too large to finish, stop, and report what is done and what is left with status \"partial\".
- When you finish, your final message must be exactly one JSON object and nothing else (no prose, no code fences):
{\"status\":\"ok | partial | failed | cancelled\",\"summary\":\"<= 3 sentences\",\"artifacts\":[{\"path\":\"relative/to/workspace\",\"kind\":\"file | patch | report\"}],\"assumptions\":[\"...\"],\"events\":[\"notable decisions\"],\"error\":null}";

/// The probe (ADR-0013 §6, §7): `codex --version` prints `codex-cli
/// 0.154.0`; `codex login status` exits 0 signed in and prints one line —
/// `Logged in using ChatGPT` — which is the reply's `mode`, verbatim.
///
/// At 0.154.0 that line is on **stderr** with nothing on stdout
/// (`0-login-status`); the shared probe hands over stdout when there is
/// any and stderr otherwise, and the first non-empty line is the mode.
#[must_use]
pub fn probe() -> Probe {
    Probe {
        version_args: vec!["--version".to_owned()],
        login_args: vec!["login".to_owned(), "status".to_owned()],
        mode_from_login: first_line,
    }
}

/// The first non-empty line, trimmed.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

// --- the schema the CLI enforces ----------------------------------------------

/// The envelope schema as the strict structured-output validator behind
/// `--output-schema` accepts it: every `oneOf` of constants is one `enum`
/// with the alternatives' descriptions folded into the property's, and
/// every object is closed with `additionalProperties: false` and all of
/// its properties required.
///
/// A projection of [`envelope::schema`], never a second source of truth:
/// the fixture `tests/fixtures/agent/envelope-schema.openai-strict.json`
/// is what the #128 runs were recorded with, and the test that compares
/// this function to it is what keeps them one.
#[must_use]
pub fn output_schema() -> Value {
    let mut schema = envelope::schema();
    strict(&mut schema);
    schema
}

fn strict(value: &mut Value) {
    let Value::Object(object) = value else {
        if let Value::Array(items) = value {
            for item in items {
                strict(item);
            }
        }
        return;
    };
    if object.get("type").and_then(Value::as_str) == Some("object") {
        if let Some(Value::Object(properties)) = object.get("properties") {
            let names: Vec<Value> = properties
                .keys()
                .map(|k| Value::String(k.clone()))
                .collect();
            object.insert("required".to_owned(), Value::Array(names));
            object.insert("additionalProperties".to_owned(), Value::Bool(false));
        }
    }
    let constants: Option<Vec<(String, String)>> = object
        .get("oneOf")
        .and_then(Value::as_array)
        .and_then(|alternatives| {
            alternatives
                .iter()
                .map(|alt| {
                    Some((
                        alt.get("const")?.as_str()?.to_owned(),
                        alt.get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    ))
                })
                .collect::<Option<Vec<_>>>()
        });
    if let Some(constants) = constants {
        object.remove("oneOf");
        object.insert("type".to_owned(), Value::String("string".to_owned()));
        object.insert(
            "enum".to_owned(),
            Value::Array(
                constants
                    .iter()
                    .map(|(name, _)| Value::String(name.clone()))
                    .collect(),
            ),
        );
        let told = constants
            .iter()
            .map(|(name, about)| format!("`{name}`: {about}"))
            .collect::<Vec<_>>()
            .join("; ");
        let about = object
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        object.insert(
            "description".to_owned(),
            Value::String(format!("{about} One of: {told}")),
        );
    }
    for (_, child) in object.iter_mut() {
        strict(child);
    }
}

// --- the Invocation half of the seam ------------------------------------------

/// The fixed argv (ADR-0013 §7, as re-pinned at 0.154.0), then the
/// configuration, then the op.
///
/// `-a never` goes **before** the subcommand: `exec --ask-for-approval` is
/// rejected (#130). Everything else is `exec`'s own: `--json` for the
/// event stream, `--output-schema` for the strict envelope,
/// `--skip-git-repo-check` because a workspace is not necessarily a
/// checkout, and `--ignore-user-config` so the user's own
/// `~/.codex/config.toml` — MCP servers, profiles, instructions — stays out
/// of a headless run while the login under the same home is still read
/// (`8-hello-ignore-user-config`: the same task at 26,954 input tokens
/// instead of 71,179). On a `run`, the sandbox (config's `permission`),
/// `--cd`, the model and the effort follow; on a `resume`, only `resume
/// <session>` does. No `-o`: the envelope is read from the event stream,
/// where it is anyway.
#[must_use]
pub fn invocation(config: &AgentConfig, accepted: &Accepted, schema: &Path) -> Invocation {
    let mut args: Vec<String> = [
        "-a",
        "never",
        "exec",
        "--json",
        "--output-schema",
        &schema.display().to_string(),
        "--skip-git-repo-check",
        "--ignore-user-config",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    match &accepted.session {
        Some(session) => {
            args.push("resume".to_owned());
            args.push(session.clone());
            args.push(accepted.task.clone());
        }
        None => {
            if let Some(permission) = &config.permission {
                args.push("--sandbox".to_owned());
                args.push(permission.clone());
            }
            args.push("--cd".to_owned());
            args.push(accepted.workspace.display().to_string());
            if let Some(model) = &config.model {
                args.push("-m".to_owned());
                args.push(model.clone());
            }
            if let Some(effort) = &config.effort {
                args.push("-c".to_owned());
                args.push(format!("model_reasoning_effort={effort}"));
            }
            args.push(format!("{CONTRACT}\n\n{}", accepted.task));
        }
    }
    Invocation {
        program: config.binary.clone(),
        args,
        cwd: accepted.workspace.clone(),
        env: config.env.clone(),
        // Nothing is ever written on stdin, and the supervisor gives a
        // child like that none: `codex exec` reads a piped stdin to end of
        // file before it starts (`6-stdin-held`).
        first_stdin: None,
        interrupt: Interrupt::Signal,
        terminal: is_turn_end,
    }
}

/// Whether a printed line ends the turn: a JSON object whose `type` is
/// `turn.completed` or `turn.failed`. The substring check first keeps the
/// drain cheap; the parse confirms it, so an `agent_message` that *quotes*
/// one is not mistaken for it.
fn is_turn_end(line: &[u8]) -> bool {
    if !contains(line, b"\"type\":\"turn.completed\"")
        && !contains(line, b"\"type\":\"turn.failed\"")
    {
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

/// Reads the [`Outcome`] out of a finished run (ADR-0013 §7, the `codex`
/// column at 0.154.0).
///
/// | field | from |
/// |---|---|
/// | `session` | the first `thread.started`'s `thread_id` |
/// | `model` | nothing: the event stream never names the model, and the driver reports only what the CLI said |
/// | `mode` | nothing here: the driver copies the login probe's line into the reply |
/// | `usage` | `turn.completed.usage`: `input_tokens` + `cache_write_input_tokens`, `output_tokens`; no cost, no turn count |
/// | `final_message` | the `text` of the last `item.completed` whose item is an `agent_message` |
/// | `stop` | `None` on `turn.completed`; an error kind from `turn.failed`'s message, or from an `error` event naming an authentication failure |
///
/// `cached_input_tokens` is **not** added: at this provider it is the
/// cached part *of* `input_tokens`, already counted, and adding it would
/// bill the cache twice. `reasoning_output_tokens` is inside
/// `output_tokens` the same way.
#[must_use]
pub fn outcome(run: &Run) -> Outcome {
    let transcript = run.transcript();
    let mut out = Outcome {
        session: transcript
            .iter()
            .find(|event| event["type"] == "thread.started")
            .and_then(|event| string(event, "thread_id")),
        final_message: transcript
            .iter()
            .filter(|event| event["type"] == "item.completed")
            .filter_map(|event| event.get("item"))
            .filter(|item| item["type"] == "agent_message")
            .filter_map(|item| string(item, "text"))
            .next_back()
            .map(String::into_bytes),
        ..Outcome::default()
    };
    let terminal = run
        .terminal_line()
        .and_then(|line| serde_json::from_slice::<Value>(line).ok());
    match terminal {
        Some(end) if end.get("type").and_then(Value::as_str) == Some("turn.completed") => {
            out.usage = usage(&end);
        }
        Some(end) => {
            out.stop = Some(Stop::Error(classify(&said(&end, &transcript))));
        }
        None => {
            // No end of turn: `settle` reports what the ladder says. Unless
            // the events name an authentication failure, which is the one
            // thing worth saying over it (ADR-0013 §6, §7).
            if let Some(message) = auth_failure(&transcript) {
                out.stop = Some(Stop::Error(refusal(ErrorKind::Unavailable, message)));
            }
        }
    }
    out
}

fn string(event: &Value, key: &str) -> Option<String> {
    event.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// `turn.completed.usage`, per the table on [`outcome`].
fn usage(end: &Value) -> Usage {
    let counts = end.get("usage");
    let count = |key: &str| {
        counts
            .and_then(|usage| usage.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Usage {
        input_tokens: count("input_tokens").saturating_add(count("cache_write_input_tokens")),
        output_tokens: count("output_tokens"),
        cost_microusd: None,
        turns: None,
    }
}

/// What the CLI said about a failed turn: `turn.failed.error.message`,
/// else the last `error` event's message, else the fact of the failure.
fn said(end: &Value, transcript: &[Value]) -> String {
    if let Some(message) = end
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
    {
        return message.trim().to_owned();
    }
    transcript
        .iter()
        .rev()
        .find(|event| event["type"] == "error")
        .and_then(|event| string(event, "message"))
        .unwrap_or_else(|| "the turn failed and the CLI said nothing".to_owned())
}

/// The message of an `error` event naming an authentication failure, if
/// the transcript has one: #130 §3's `401 Unauthorized` retry loop.
fn auth_failure(transcript: &[Value]) -> Option<String> {
    transcript
        .iter()
        .filter(|event| event["type"] == "error")
        .filter_map(|event| string(event, "message"))
        .find(|message| names_auth(&message.to_ascii_lowercase()))
}

/// An error kind from the CLI's text (ADR-0013 §2, §7): authentication is
/// `unavailable`, a rate limit or a quota window is `throttled`, anything
/// else — `5-model-rejected`'s 400, an unknown model, a 5xx — is
/// `provider`, with the text.
fn classify(message: &str) -> wire::RunError {
    let lowered = message.to_ascii_lowercase();
    if names_auth(&lowered) {
        refusal(ErrorKind::Unavailable, message)
    } else if names_rate_limit(&lowered) {
        refusal(ErrorKind::Throttled, message)
    } else {
        refusal(ErrorKind::Provider, message)
    }
}

/// The words an unauthenticated run uses: #130 §3 pinned `401
/// Unauthorized`; the rest is read tolerantly, unpinned.
fn names_auth(lowered: &str) -> bool {
    [
        "401",
        "unauthorized",
        "unauthenticated",
        "not logged in",
        "not authenticated",
        "please log in",
        "login required",
        "invalid api key",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

/// The words a rate limit or a quota window is expected to use. Unpinned;
/// read tolerantly.
fn names_rate_limit(lowered: &str) -> bool {
    [
        "429",
        "rate limit",
        "rate_limit",
        "too many requests",
        "usage limit",
        "quota",
        "overloaded",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

// --- the driver ---------------------------------------------------------------

/// The `codex` agent driver: one registration, one tool, one CLI binary
/// under one login the driver never sees. Cheap to clone; every clone
/// shares one flight registry, and dropping the last clone abandons every
/// open run and removes the schema file.
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
    /// Where [`output_schema`] was written at construction, for
    /// `--output-schema`. The CLI reads it at start-up, so a run that is
    /// open when the file goes is unaffected.
    schema_file: PathBuf,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // A harness shutting down leaves no CLI behind (ADR-0013 §5).
        self.flights.abandon_all();
        let _ = std::fs::remove_file(&self.shared.schema_file);
    }
}

static SCHEMA_SEQ: AtomicU64 = AtomicU64::new(0);

impl CodexDriver {
    /// Builds the driver: checks the config, runs the version probe, runs
    /// the login probe once (ADR-0013 §6), and writes the strict envelope
    /// schema to a scratch file for `--output-schema`.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] for a config the host cannot honour, a binary that
    /// cannot be run, a version that does not match the pin, or a scratch
    /// file that cannot be written. **Not** for a CLI that is logged out:
    /// that is a runtime state a human changes, and every `send` until
    /// then answers `error.unavailable`.
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
        let schema_file = write_schema()?;
        Ok(Self {
            inner: Arc::new(Inner {
                shared: Arc::new(Shared {
                    config,
                    version,
                    probe,
                    availability,
                    schema_file,
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

    /// Where the strict envelope schema was written for `--output-schema`.
    #[must_use]
    pub fn schema_file(&self) -> &Path {
        &self.inner.shared.schema_file
    }

    /// How many runs are open right now.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.inner.flights.in_flight()
    }
}

/// Writes [`output_schema`] to a fresh file under the host's temporary
/// directory, named by this process and a counter so two drivers in one
/// harness never share one.
fn write_schema() -> Result<PathBuf, ConfigError> {
    let seq = SCHEMA_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "tau-codex-{}-{seq}.envelope-schema.json",
        std::process::id()
    ));
    let text = serde_json::to_string_pretty(&output_schema()).unwrap_or_default();
    std::fs::write(&path, text).map_err(|e| ConfigError::Scratch {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    Ok(path)
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
                let (reply, consumed) = on_thread.run(&request.payload, flight.cancel());
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
    /// run, the read-back, the settlement.
    fn run(&self, payload: &[u8], cancel: &process::Cancel) -> (Reply, Consumption) {
        let config = &self.config;
        let accepted = match decode(payload, config, CAPS) {
            Ok(accepted) => accepted,
            Err(error) => return (refused(config, &self.version, error), Consumption::none()),
        };
        // Refused while logged out, and re-probed once per refused `send`
        // (ADR-0013 §6): never a retry loop, never a login attempt. For
        // `codex` this is the only gate that works: an unsigned run starts
        // a thread and retries 401s rather than saying so (#130 §3).
        let verdict = self.availability.check(config, &self.probe);
        if let Verdict::Unavailable { message } = verdict {
            return (
                refused(
                    config,
                    &self.version,
                    refusal(ErrorKind::Unavailable, message),
                ),
                Consumption::none(),
            );
        }
        let invocation = invocation(config, &accepted, &self.schema_file);
        let run = match process::run(&invocation, config.bounds(), cancel) {
            Ok(run) => run,
            Err(e) => {
                return (
                    refused(
                        config,
                        &self.version,
                        refusal(
                            ErrorKind::Host,
                            format!("cannot start `{}`: {e}", config.binary.display()),
                        ),
                    ),
                    Consumption::none(),
                )
            }
        };
        if let Some(error) = refused_before_start(&run) {
            // `7-resume-unknown`: nothing on stdout, exit 1, the reason on
            // stderr. No thread was started, so nothing reached the
            // provider, and nothing is billed.
            return (refused(config, &self.version, error), Consumption::none());
        }
        let mut outcome = outcome(&run);
        outcome.mode = verdict.mode().map(str::to_owned);
        let (mut reply, mut consumed) = settle(config, &self.version, &run, &outcome);
        if let Some(Stop::Error(error)) = &outcome.stop {
            if error.kind == ErrorKind::Unavailable {
                // The run's own events say the CLI is logged out, whatever
                // the probe said earlier — and whatever the ladder said,
                // because the 401 loop ends only when the driver ends it.
                // Nothing was served, so nothing is billed unless the CLI
                // reported otherwise (ADR-0013 §2's `unavailable` row).
                self.availability.fail(error.message.clone());
                reply.stop = Stop::Error(error.clone());
                reply.envelope = None;
                if !run.usage_is_reported() {
                    consumed = Consumption::none();
                }
            }
        }
        (reply, consumed)
    }
}

/// A run that ended by itself having printed nothing, with a non-zero
/// exit: the CLI refused before starting a thread. The reason is on
/// stderr — `7-resume-unknown`'s `no rollout found for thread id` — and
/// the kind is `provider`, because it is the CLI's own refusal.
fn refused_before_start(run: &Run) -> Option<wire::RunError> {
    if run.ending != Ending::Completed || !run.lines.is_empty() || run.code() == Some(0) {
        return None;
    }
    let said = run.stderr.trim();
    let message = match (run.code(), said.is_empty()) {
        (Some(code), true) => format!("the CLI exited {code} before starting a thread"),
        (Some(code), false) => format!("the CLI exited {code} before starting a thread: {said}"),
        (None, true) => "the CLI was signalled before starting a thread".to_owned(),
        (None, false) => format!("the CLI was signalled before starting a thread: {said}"),
    };
    Some(classify(&message))
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
