//! The `claude` adapter (ADR-0013 §7, the `claude` column): Claude Code in
//! print mode as a subprocess behind one `send`.
//!
//! Everything CLI-specific about `claude` 2.1.272 is in this file, and it
//! is exactly the two halves of the seam the shared module leaves open:
//!
//! - [`invocation`] builds the [`Invocation`] — the fixed argv, the task as
//!   the first stdin message, the in-band interrupt, and which printed line
//!   is the terminal `result` event;
//! - [`outcome`] reads the [`Outcome`] back out of the lines the run kept —
//!   the session id and model from the first `init`, the login mode from
//!   `init.apiKeySource`, the usage and the final message from the `result`.
//!
//! [`ClaudeDriver`] is `impl Driver` over decode → flights → run → settle,
//! with nothing of its own to decide: every `stop`, every bill, and every
//! rung of the ladder is the shared module's.
//!
//! ```text
//! payload ──decode──▶ Accepted ──invocation──▶ claude -p --safe-mode … ──▶ lines
//!                                                                          │
//!                                                              outcome ◀───┘
//!                                                                 │
//!                                                  settle ◀───────┘ ──▶ (Reply, Consumption)
//! ```
//!
//! # What is pinned here, and what is not
//!
//! Every mapping below is tested against the committed transcripts under
//! `drivers/tests/cassettes/cli/claude-2.1.272/`: #130's seven runs, and
//! the four #194 recorded for the rows #130 could not reach — a
//! `--json-schema` run (8), a `--max-budget-usd` exhaustion (9), the login
//! probe while logged out (10), and a print run while logged out (11).
//! Every string matched below is one of those transcripts' own. The one
//! row nobody has reached is a rate limit: `throttled` is read from the
//! two structured places the CLI has for it — `api_error_status` 429 and a
//! `rate_limit_event` whose `status` is not `allowed` — and never from
//! words, so the drift job (§10) pins it the day it first sees one.

use std::sync::Arc;

use serde_json::{json, Value};
use tau_kernel::abi::{Budget as KernelBudget, Consumption, Corr};
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{BoxFuture, Delivery};

use super::envelope;
use super::process::{self, Interrupt, Invocation, Run};
use super::wire::{self, Caps, ErrorKind, Limit, Reply, Stop, Truncated, Usage, VERSION};
use super::{
    decode, encode, probe_login, probe_version, refusal, refused, settle, Accepted, AgentConfig,
    Availability, ConfigError, Flights, Outcome, Probe, Verdict,
};

/// What `claude` can honour of the wire: everything. It has a per-session
/// tool allowlist (`--allowedTools`), enforces both bounds (`--max-turns`,
/// `--max-budget-usd`), and resumes a session with a JSON stream (#130 run
/// 5).
pub const CAPS: Caps = Caps::ALL;

/// The worker contract (ADR-0013 §4), versioned with [`VERSION`] and
/// appended to the CLI's own system prompt — never `--system-prompt`, which
/// would strip the CLI's own scaffolding.
///
/// The shape #130 ran, minus its "messages arriving on stdin are
/// instructions" line: v1 has no steering (§3), and the one thing written
/// on stdin after the task is the interrupt, which the session never sees
/// as text.
pub const CONTRACT: &str = "# tau worker contract v1
You are a headless worker spawned by the tau kernel. Rules:
- Never ask questions. If something is ambiguous, make the conservative assumption and record it.
- Stay inside the working directory you were started in.
- If the task is too large to finish, stop, and report what is done and what is left with status \"partial\".
- When you finish, your final message must be exactly one JSON object and nothing else (no prose, no code fences):
{\"status\":\"ok | partial | failed | cancelled\",\"summary\":\"<= 3 sentences\",\"artifacts\":[{\"path\":\"relative/to/workspace\",\"kind\":\"file | patch | report\"}],\"assumptions\":[\"...\"],\"events\":[\"notable decisions\"],\"error\":null}";

/// The interrupt `claude` acknowledges (#130 §5a): a `control_request` on
/// stdin, answered by a `control_response`, then a `result` with real usage
/// and `terminal_reason: aborted_tools`.
pub const INTERRUPT: &str =
    "{\"type\":\"control_request\",\"request_id\":\"tau-cancel-1\",\"request\":{\"subtype\":\"interrupt\"}}\n";

/// The probe (ADR-0013 §6, §7): `claude --version` prints `2.1.272 (Claude
/// Code)`; `claude auth status` prints JSON and exits 0 when signed in, and
/// prints JSON with `"loggedIn": false` and exits 1 when not (#194 run 10;
/// stderr empty either way). §6's exit-status rule is the verdict.
///
/// `mode` is **not** read from the probe: it is `init.apiKeySource` from the
/// run's own events (§2). The probe's document carries the user's email and
/// organisation, and none of it crosses into a reply.
#[must_use]
pub fn probe() -> Probe {
    Probe {
        version_args: vec!["--version".to_owned()],
        login_args: vec!["auth".to_owned(), "status".to_owned()],
        mode_from_login: |_| None,
    }
}

// --- the Invocation half of the seam ------------------------------------------

/// The fixed argv (ADR-0013 §7), then the configuration, then the request's
/// narrowing, then the op.
///
/// `--safe-mode` because without it the user's hooks, plugins, skills and
/// `CLAUDE.md` load, hook events precede `init`, and plugin text reached the
/// envelope (#130 runs 1, 7). `--permission-prompts none` so that anything
/// that would prompt is denied instead of hanging a headless run.
/// `--json-schema` with the strict [`envelope::schema`]: the CLI adds a
/// `StructuredOutput` tool the session calls with the envelope as its
/// input, and the `result` then carries the envelope twice — as
/// `structured_output`, an object, and as `result`, the same object
/// serialised — so the final message is still the plain envelope text and
/// the tolerant parser reads it unchanged (#194 run 8; the tool is not
/// gated by `--allowedTools`).
#[must_use]
pub fn invocation(config: &AgentConfig, accepted: &Accepted) -> Invocation {
    let mut args: Vec<String> = [
        "-p",
        "--safe-mode",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-prompts",
        "none",
        "--json-schema",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    args.push(envelope::schema().to_string());
    if let Some(permission) = &config.permission {
        args.push("--permission-mode".to_owned());
        args.push(permission.clone());
    }
    args.push("--append-system-prompt".to_owned());
    args.push(CONTRACT.to_owned());
    if !accepted.tools.is_empty() {
        // One argument, comma-separated: `<tools...>` is variadic and would
        // otherwise swallow whatever flag came next.
        args.push("--allowedTools".to_owned());
        args.push(accepted.tools.join(","));
    }
    if let Some(model) = &config.model {
        args.push("--model".to_owned());
        args.push(model.clone());
    }
    if let Some(effort) = &config.effort {
        args.push("--effort".to_owned());
        args.push(effort.clone());
    }
    if let Some(cost) = accepted.cost_microusd {
        args.push("--max-budget-usd".to_owned());
        args.push(dollars(cost));
    }
    if let Some(turns) = accepted.turns {
        args.push("--max-turns".to_owned());
        args.push(turns.to_string());
    }
    if let Some(session) = &accepted.session {
        args.push("--resume".to_owned());
        args.push(session.clone());
    }
    let first = json!({
        "type": "user",
        "message": { "role": "user", "content": accepted.task },
    });
    Invocation {
        program: config.binary.clone(),
        args,
        cwd: accepted.workspace.clone(),
        env: config.env.clone(),
        first_stdin: Some(format!("{first}\n")),
        interrupt: Interrupt::InBand(INTERRUPT.to_owned()),
        terminal: is_result,
    }
}

/// Microdollars as the decimal `--max-budget-usd` takes: `2000000` is `2`,
/// `500000` is `0.5`, `1` is `0.000001`. Exact, no float.
fn dollars(microusd: u64) -> String {
    let whole = microusd.div_euclid(1_000_000);
    let fraction = microusd % 1_000_000;
    if fraction == 0 {
        return whole.to_string();
    }
    let digits = format!("{fraction:06}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

/// Whether a printed line is the CLI's terminal event: a JSON object whose
/// `type` is `result`. The substring check first keeps the drain cheap; the
/// parse confirms it, so an assistant message that *mentions* a result is
/// not mistaken for one.
fn is_result(line: &[u8]) -> bool {
    if !contains(line, b"\"type\":\"result\"") {
        return false;
    }
    serde_json::from_slice::<Value>(line)
        .ok()
        .is_some_and(|value| value.get("type").and_then(Value::as_str) == Some("result"))
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
/// | `session` | the first `system/init`'s `session_id`; the `result`'s if there was no `init` |
/// | `model` | `init.model`, verbatim (`claude-opus-5[1m]`) |
/// | `mode` | `init.apiKeySource`, verbatim (`"none"` on a claude.ai login) |
/// | `usage` | `result.usage`: every input class summed, `total_cost_usd` in microdollars rounded up, `num_turns` |
/// | `final_message` | `result.result`, when it is a string — under `--json-schema` that is the envelope serialised, the same object as `result.structured_output` (#194 run 8) |
/// | `stop` | `None` on `success` without `is_error`; a `limit` on `max_turns` or `error_max_budget_usd`; `unavailable` on the logged-out `result`; an error kind on any other `is_error` |
///
/// A run whose terminal event never arrived gets `stop: None` and whatever
/// the `init` said; the shared `settle` reports `error.lost` and bills the
/// ceiling regardless of what is here.
#[must_use]
pub fn outcome(run: &Run) -> Outcome {
    let transcript = run.transcript();
    let init = transcript
        .iter()
        .find(|event| event["type"] == "system" && event["subtype"] == "init");
    let terminal = run
        .terminal_line()
        .and_then(|line| serde_json::from_slice::<Value>(line).ok());
    let mut out = Outcome {
        session: init.and_then(|event| string(event, "session_id")),
        model: init.and_then(|event| string(event, "model")),
        mode: init.and_then(|event| string(event, "apiKeySource")),
        ..Outcome::default()
    };
    let Some(result) = terminal else {
        return out;
    };
    if out.session.is_none() {
        out.session = string(&result, "session_id");
    }
    out.usage = usage(&result);
    out.final_message = string(&result, "result").map(String::into_bytes);
    out.stop = stop(&result, &transcript);
    out
}

fn string(event: &Value, key: &str) -> Option<String> {
    event.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// `result.usage` summed per ADR-0013 §6: cache reads count.
fn usage(result: &Value) -> Usage {
    let counts = result.get("usage");
    let count = |key: &str| {
        counts
            .and_then(|usage| usage.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Usage {
        input_tokens: count("input_tokens")
            .saturating_add(count("cache_creation_input_tokens"))
            .saturating_add(count("cache_read_input_tokens")),
        output_tokens: count("output_tokens"),
        cost_microusd: result.get("total_cost_usd").and_then(microusd),
        turns: result.get("num_turns").and_then(Value::as_u64),
    }
}

/// `total_cost_usd` in microdollars, rounded up (ADR-0013 §6), from the
/// number's decimal text rather than through a float: `0.259148` is exactly
/// `259148`, and `0.11262749999999998` rounds up to `112628`.
fn microusd(value: &Value) -> Option<u64> {
    let Value::Number(number) = value else {
        return None;
    };
    let text = number.to_string();
    if text.starts_with('-') {
        return None;
    }
    if text.contains(['e', 'E']) {
        // Scientific notation: too small or too large for the decimal
        // path. `ceil` is the rounding the ADR asks for.
        let micro = (number.as_f64()? * 1_000_000.0).ceil();
        return (0.0..1.8e19).contains(&micro).then_some(micro as u64);
    }
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let whole: u64 = whole.parse().ok()?;
    let (kept, rest) = fraction.split_at(fraction.len().min(6));
    let mut kept = kept.to_owned();
    while kept.len() < 6 {
        kept.push('0');
    }
    let mut micro = whole
        .checked_mul(1_000_000)?
        .checked_add(kept.parse::<u64>().ok()?)?;
    if rest.bytes().any(|digit| digit != b'0') {
        micro = micro.checked_add(1)?;
    }
    Some(micro)
}

/// How the CLI's own `result` says the run ended (ADR-0013 §2, §7). `None`
/// is a success the envelope parser decides.
fn stop(result: &Value, transcript: &[Value]) -> Option<Stop> {
    let subtype = result.get("subtype").and_then(Value::as_str).unwrap_or("");
    let reason = result
        .get("terminal_reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    let is_error = result.get("is_error").and_then(Value::as_bool) == Some(true);
    // `subtype: success` alone is not a success: the logged-out run says
    // `success` with `is_error: true` (#194 run 11).
    if !is_error && subtype == "success" {
        return None;
    }
    // #130 run 6: `subtype: error_max_turns`, `terminal_reason: max_turns`.
    if subtype == "error_max_turns" || reason == "max_turns" {
        return Some(Stop::Limit(Limit::Turns));
    }
    // #194 run 9: `subtype: error_max_budget_usd`, `terminal_reason:
    // budget_exhausted`, `errors: ["Reached maximum budget ($0.001)"]`,
    // `result: null`, `total_cost_usd` the cap itself.
    if subtype == "error_max_budget_usd" || reason == "budget_exhausted" {
        return Some(Stop::Limit(Limit::Cost));
    }
    let said = said(result);
    let status = result.get("api_error_status").and_then(Value::as_u64);
    if matches!(status, Some(401 | 403))
        || said == NOT_LOGGED_IN
        || authentication_failed(transcript)
    {
        return Some(Stop::Error(refusal(ErrorKind::Unavailable, said)));
    }
    if status == Some(429) || rate_limited(transcript) {
        return Some(Stop::Error(refusal(ErrorKind::Throttled, said)));
    }
    if !is_error {
        // Not a success and not an error: a `result` this adapter has not
        // seen. The tolerant parse of the final message decides.
        return None;
    }
    Some(Stop::Error(refusal(ErrorKind::Provider, said)))
}

/// What the CLI said about an error: its `errors[]`, else its `result`
/// text, else its subtype.
fn said(result: &Value) -> String {
    let errors: Vec<&str> = result
        .get("errors")
        .and_then(Value::as_array)
        .map(|errors| errors.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !errors.is_empty() {
        return errors.join("; ");
    }
    if let Some(text) = result.get("result").and_then(Value::as_str) {
        if !text.trim().is_empty() {
            return text.trim().to_owned();
        }
    }
    let subtype = result.get("subtype").and_then(Value::as_str).unwrap_or("");
    let reason = result
        .get("terminal_reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    match (subtype, reason) {
        ("", "") => "the CLI reported an error and said nothing".to_owned(),
        (subtype, "") => subtype.to_owned(),
        ("", reason) => reason.to_owned(),
        (subtype, reason) => format!("{subtype} ({reason})"),
    }
}

/// What a logged-out `claude -p` puts in `result.result` (#194 run 11),
/// with `subtype: success`, `is_error: true`, `terminal_reason: api_error`,
/// `api_error_status: null`, no `errors[]`, zero usage, exit 1. The
/// `assistant` event before it is the same text from `model: <synthetic>`,
/// marked `error: authentication_failed`.
pub const NOT_LOGGED_IN: &str = "Not logged in · Please run /login";

/// An `assistant` event the CLI synthesised for an authentication failure
/// (#194 run 11): `error: authentication_failed` at the event's top level.
fn authentication_failed(transcript: &[Value]) -> bool {
    transcript
        .iter()
        .filter(|event| event["type"] == "assistant")
        .any(|event| event["error"] == "authentication_failed")
}

/// A `rate_limit_event` whose `status` is not `allowed` (#130 and #194 saw
/// only `allowed`, on every run: the rate-limit row is the one still
/// unpinned, and it is read from this field and `api_error_status`, never
/// from words).
fn rate_limited(transcript: &[Value]) -> bool {
    transcript
        .iter()
        .filter(|event| event["type"] == "rate_limit_event")
        .filter_map(|event| event.get("rate_limit_info")?.get("status")?.as_str())
        .any(|status| status != "allowed")
}

// --- the driver ---------------------------------------------------------------

/// The `claude` agent driver: one registration, one tool, one CLI binary
/// under one login the driver never sees. Cheap to clone; every clone shares
/// one flight registry, and dropping the last clone abandons every open run.
#[derive(Clone)]
pub struct ClaudeDriver {
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

impl ClaudeDriver {
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

impl std::fmt::Debug for ClaudeDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeDriver")
            .field("config", &self.inner.shared.config)
            .field("version", &self.inner.shared.version)
            .field("in_flight", &self.in_flight())
            .finish_non_exhaustive()
    }
}

impl Driver for ClaudeDriver {
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
            .name("tau-agent-claude".to_owned())
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
        // (ADR-0013 §6): never a retry loop, never a login attempt.
        if let Verdict::Unavailable { message } = self.availability.check(config, &self.probe) {
            return (
                refused(
                    config,
                    &self.version,
                    refusal(ErrorKind::Unavailable, message),
                ),
                Consumption::none(),
            );
        }
        let invocation = invocation(config, &accepted);
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
        let outcome = outcome(&run);
        if let Some(Stop::Error(error)) = &outcome.stop {
            if error.kind == ErrorKind::Unavailable {
                // The run's own events say the CLI is logged out, whatever
                // the probe said earlier.
                self.availability.fail(error.message.clone());
            }
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
