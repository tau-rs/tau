//! The shared half of the agent drivers: the `claude` and `codex` CLIs as
//! subprocess tools behind `send` (ADR-0013).
//!
//! # What an agent driver is
//!
//! A *model driver* (ADR-0006) is a typist: you dictate one message, read
//! the completion back, dictate the next, and the tool loop in `libtau`
//! does the stepping. An *agent driver* is a contractor: you hand over a
//! whole work order and the keys to one workshop, a command-line tool runs
//! its own loop as a child process, and one signed report comes back. The
//! CLI owns its login — a subscription or an API key, the driver cannot
//! tell and does not try — and tau never sees a credential.
//!
//! ```text
//! agent ──send(cap, request)──▶ kernel ──Delivery──▶ agent driver
//!                                                       │ spawn, new process group,
//!                                                       │ cwd = workspace, env = exactly config.env
//!                                                       ▼
//!                                                 the CLI, running its own tool loop
//!                                                       │ one JSON event per stdout line
//!                                                       ▼
//!                                                 the last message is the envelope
//! ```
//!
//! # What lives here, and what lives in an adapter
//!
//! This module is everything the two CLIs share:
//!
//! | | |
//! |---|---|
//! | [`wire`] | the request and reply shapes, and the schema `describe()` projects |
//! | [`envelope`] | the worker's report, its strict schema, and the tolerant parser |
//! | [`process`] | spawn with a scrubbed environment, bounded drain, the cancel ladder |
//! | this file | [`AgentConfig`] and its ceiling, request validation, the auth probe, the flight registry, the billing table |
//!
//! An adapter ([`claude`] behind `agent-claude`, [`codex`] behind
//! `agent-codex`) supplies exactly two things: a [`process::Invocation`]
//! going in — argv, the first stdin message, how to interrupt, which line is
//! terminal — and an [`Outcome`] coming out, read from the lines the run
//! collected. Those two types *are* the seam: an adapter that forgets a
//! field does not compile, which is the same contract a trait would give.
//!
//! ADR-0013 §1 writes that seam as a crate-private `Cli` trait with a
//! generic `AgentDriver<C>`. It is deferred, not dropped: a crate-private
//! trait cannot bound a public generic, a trait with no implementors fails
//! `-D warnings` as dead code, and nothing under `tests/` could implement
//! one — which would have shipped the cancel ladder, the part that decides
//! what a cancelled run costs, with no test at all. The trait is extracted
//! in #128, when two implementors exist to shape it; its methods return
//! these same two types, so the extraction is a move with no behaviour
//! change. See the ADR's amendment for 2026-09-20.
//!
//! # The one thing this module may never do
//!
//! Read a credential. Not a login file under the user's home, not the
//! system key store, and no provider endpoint: the driver spawns a binary
//! and reads its stdout, and the binary owns its login.
//!
//! `drivers/tests/agent_guard.rs` is the tripwire. It fails if any source
//! file in this directory so much as *names* one of those things, which is
//! why this paragraph describes them instead of spelling them — and it
//! checks that the `agent` feature pulls in no HTTP client, so that a
//! provider call is not merely discouraged here but unreachable.

#[cfg(feature = "agent-claude")]
pub mod claude;
#[cfg(feature = "agent-codex")]
pub mod codex;
pub mod envelope;
pub mod process;
pub mod wire;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::Value;
use tau_kernel::abi::{Budget as KernelBudget, Consumption, Corr, DimKey};

use envelope::Envelope;
use process::{Bounds, Cancel, Run};
use wire::{Caps, ErrorKind, Limit, Op, Reply, Request, RunError, Stop, Truncated, Usage, VERSION};

/// Everything the harness decides about one agent capability.
///
/// One registration is at most one tool (ADR-0006 §5): one CLI binary, one
/// model, one permission cage, one task bound. A harness that wants a cheap
/// reviewer and an expensive implementer registers two drivers, named by
/// the operator, each with its own ceiling and capability. The requester —
/// which may itself be a model — chooses none of this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentConfig {
    /// What the reply's `cli.name` says: `claude`, `codex`.
    pub name: String,
    /// The binary. An absolute path is safest: the child's environment is
    /// exactly [`env`](Self::env), so `PATH` is whatever the harness put
    /// there, or nothing.
    pub binary: PathBuf,
    /// A substring the binary's version output must contain, when the
    /// harness pins one. A mismatch is a `ConfigError`, at registration.
    pub expect_version: Option<String>,
    /// The root every request's `workspace` is resolved under. A path that
    /// escapes it is refused; nothing is spawned.
    pub workspace_root: PathBuf,
    /// The CLI-native tool names a session may use. A request may narrow
    /// this set, never widen it.
    pub tools: Vec<String>,
    /// The model the CLI is told to use, verbatim. Opaque here: the adapter
    /// knows which flag carries it.
    pub model: Option<String>,
    /// The reasoning effort, verbatim, where the CLI has one.
    pub effort: Option<String>,
    /// The permission cage, verbatim: `claude`'s `--permission-mode`,
    /// `codex`'s `-s`. Opaque here, and never the request's to choose.
    pub permission: Option<String>,
    /// The child's **whole** environment. Nothing is inherited. `HOME`
    /// belongs here, because the CLI's login lives under it; a harness that
    /// wants the API-key billing cell adds the key, one that wants the
    /// subscription cell does not, and the driver cannot tell which it got.
    pub env: Vec<(String, String)>,
    /// The largest `task` accepted. Over it is `error.unsupported`, and
    /// nothing is spawned.
    pub task_bytes: usize,
    /// How much of the CLI's event stream the reply carries.
    pub transcript_bytes: usize,
    /// How much of the CLI's stderr the driver keeps for its error messages.
    pub stderr_bytes: usize,
    /// The task bound in tokens, and the `tokens` ceiling.
    pub task_tokens: u64,
    /// The task bound in microdollars, when the harness budgets in money.
    pub task_cost_microusd: Option<u64>,
    /// The task bound in turns, where the CLI enforces one.
    pub task_turns: Option<u64>,
    /// Price per input token, in microdollars, when the harness has one.
    pub input_price_microusd: Option<u64>,
    /// Price per output token, in microdollars.
    pub output_price_microusd: Option<u64>,
    /// The driver's own wall bound. Reaching it climbs the cancel ladder
    /// and reports `{ "limit": "wall" }`. Not reported as consumption: the
    /// reducer charges `wall_ms` on every `Tick` the requester waits
    /// through, and the driver never reads a clock.
    pub wall: Duration,
    /// How long each rung of the ladder is given before the next.
    pub abandon_grace: Duration,
    /// The wall bound on a probe subprocess.
    pub probe_wall: Duration,
    /// The opening sentence of `describe()`. The harness may replace the
    /// sentence; it cannot remove the bound from it (ADR-0009 §7).
    pub description: Option<String>,
}

impl AgentConfig {
    /// A config with the defaults of ADR-0013: 64 KiB of task, 1 MiB of
    /// transcript, 8 KiB of stderr, an empty environment, a five-second
    /// abandon grace and a ten-second probe bound.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        binary: impl Into<PathBuf>,
        workspace_root: impl Into<PathBuf>,
        task_tokens: u64,
        wall: Duration,
    ) -> Self {
        Self {
            name: name.into(),
            binary: binary.into(),
            expect_version: None,
            workspace_root: workspace_root.into(),
            tools: Vec::new(),
            model: None,
            effort: None,
            permission: None,
            env: Vec::new(),
            task_bytes: 64 * 1024,
            transcript_bytes: 1024 * 1024,
            stderr_bytes: 8 * 1024,
            task_tokens,
            task_cost_microusd: None,
            task_turns: None,
            input_price_microusd: None,
            output_price_microusd: None,
            wall,
            abandon_grace: Duration::from_secs(5),
            probe_wall: Duration::from_secs(10),
            description: None,
        }
    }

    /// The registration ceiling (ADR-0013 §6): `tokens` is the task bound,
    /// and `cost_microusd` is the configured cost bound, or the task's
    /// tokens at the dearer of the two configured prices, or absent.
    ///
    /// One honest consequence, and the difference from a model call: **an
    /// agent task's ceiling is not enforced by the driver before the
    /// fact.** A model driver refuses a prompt over its input bound and
    /// clamps `max_tokens`; an agent task's cost is unknowable until the
    /// CLI stops. What bounds it is the CLI's own budget flags where it has
    /// them, the worker contract's instruction to stop and report
    /// `partial`, and the driver's wall bound. A CLI that overshoots, or
    /// has no bound flag at all, reports above the ceiling and the kernel
    /// records the excess as overdraft (#18). v1 does not pretend the fence
    /// is hard; it makes the overshoot visible.
    #[must_use]
    pub fn ceiling(&self) -> KernelBudget {
        let mut dims = vec![(DimKey::Tokens, self.task_tokens)];
        if let Some(cost) = self.cost_ceiling() {
            dims.push((DimKey::CostMicroUsd, cost));
        }
        KernelBudget::from_dims(dims)
    }

    /// The `cost_microusd` ceiling, when there is one to derive.
    fn cost_ceiling(&self) -> Option<u64> {
        if let Some(cost) = self.task_cost_microusd {
            return Some(cost);
        }
        let dearest = self
            .input_price_microusd
            .unwrap_or(0)
            .max(self.output_price_microusd.unwrap_or(0));
        (dearest > 0).then(|| self.task_tokens.saturating_mul(dearest))
    }

    /// What the caller's run is bounded by.
    #[must_use]
    pub fn bounds(&self) -> Bounds {
        Bounds {
            wall: self.wall,
            grace: self.abandon_grace,
            transcript_bytes: self.transcript_bytes,
            stderr_bytes: self.stderr_bytes,
        }
    }

    /// What a model that delegates reads (ADR-0013 §9): the harness's
    /// sentence, then the cage, the bound, and the report it will get back.
    #[must_use]
    pub fn describe(&self, caps: Caps) -> String {
        let about = self.description.clone().unwrap_or_else(|| {
            format!(
                "Delegate a whole task to a {} session in the workspace.",
                self.name
            )
        });
        let tools = match list(&self.tools) {
            Some(tools) => format!("headless with tools {tools}"),
            None => "headless".to_owned(),
        };
        let resume = if caps.resume {
            " Pass `session` from a previous report to continue it."
        } else {
            ""
        };
        format!(
            "{about} It runs {tools}, may {spend} per call, and answers with a JSON report: \
             status, summary, artifacts, assumptions, events.{resume}",
            spend = self.spend_text(),
        )
    }

    /// "spend up to $2 (400k tokens, 40 turns)", or what there is of it.
    fn spend_text(&self) -> String {
        let mut bounds = vec![format!("{} tokens", count(self.task_tokens))];
        if let Some(turns) = self.task_turns {
            bounds.push(format!("{turns} turns"));
        }
        let bounds = bounds.join(", ");
        match self.task_cost_microusd {
            Some(cost) => format!("spend up to {} ({bounds})", dollars(cost)),
            None => format!("use up to {bounds}"),
        }
    }

    /// Checks what can be checked without running anything.
    ///
    /// # Errors
    ///
    /// [`ConfigError`], as described on each variant. Note what is *not*
    /// here: being logged out. That is a runtime state a human changes by
    /// logging in, so it is `error.unavailable` on a `send`, never a
    /// refusal to boot (ADR-0013 §6).
    pub fn check(&self) -> Result<(), ConfigError> {
        if self.name.is_empty() {
            return Err(ConfigError::NoName);
        }
        if self.binary.as_os_str().is_empty() {
            return Err(ConfigError::NoBinary);
        }
        let zero =
            |ok: bool, what: &'static str| ok.then_some(()).ok_or(ConfigError::ZeroBound { what });
        zero(self.task_tokens >= 1, "task_tokens")?;
        zero(self.task_bytes >= 1, "task_bytes")?;
        zero(self.transcript_bytes >= 1, "transcript_bytes")?;
        zero(!self.wall.is_zero(), "wall")?;
        zero(!self.abandon_grace.is_zero(), "abandon_grace")?;
        zero(!self.probe_wall.is_zero(), "probe_wall")?;
        if !self.workspace_root.is_dir() {
            return Err(ConfigError::WorkspaceRoot {
                path: self.workspace_root.clone(),
            });
        }
        Ok(())
    }
}

/// `400k`, `1.5M`, or the number: a bound a model reads should be as short
/// as it is exact.
fn count(n: u64) -> String {
    const K: u64 = 1_000;
    const M: u64 = 1_000_000;
    const TENTH: u64 = 100_000;
    if n >= M && n.is_multiple_of(TENTH) {
        let whole = n.div_euclid(M);
        match n.div_euclid(TENTH) % 10 {
            0 => format!("{whole}M"),
            tenth => format!("{whole}.{tenth}M"),
        }
    } else if n >= K && n.is_multiple_of(K) {
        format!("{}k", n.div_euclid(K))
    } else {
        n.to_string()
    }
}

/// `$2`, `$0.50`.
fn dollars(microusd: u64) -> String {
    let whole = microusd.div_euclid(1_000_000);
    // Rounded up: a bound shown to a model should never read lower than it
    // is. A remainder that rounds to a whole dollar carries.
    let cents = (microusd % 1_000_000).div_ceil(10_000);
    match cents {
        0 => format!("${whole}"),
        100 => format!("${}", whole.saturating_add(1)),
        cents => format!("${whole}.{cents:02}"),
    }
}

/// `Read, Edit and Bash`.
fn list(items: &[String]) -> Option<String> {
    match items {
        [] => None,
        [only] => Some(only.clone()),
        [first @ .., last] => Some(format!("{} and {last}", first.join(", "))),
    }
}

/// Why an agent driver could not be built from its config.
///
/// Loud, at registration: a limit the host cannot honour is not something
/// to discover on the first `send` (the ADR-0009 pattern).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// `name` is empty: the reply could not say which CLI answered.
    #[error("name is empty")]
    NoName,
    /// `binary` is empty.
    #[error("binary is empty")]
    NoBinary,
    /// A bound of zero: nothing could run under it.
    #[error("{what} must be at least 1")]
    ZeroBound {
        /// Which bound.
        what: &'static str,
    },
    /// `workspace_root` is not a directory.
    #[error("workspace root `{path}` is not a directory", path = path.display())]
    WorkspaceRoot {
        /// The root as configured.
        path: PathBuf,
    },
    /// The binary could not be run at all.
    #[error("cannot run `{binary}`: {reason}", binary = binary.display())]
    Binary {
        /// The binary as configured.
        binary: PathBuf,
        /// What the host said.
        reason: String,
    },
    /// The binary's version output does not contain `expect_version`.
    #[error("`{binary}` reports `{found}`, which does not match the pinned `{expected}`", binary = binary.display())]
    Version {
        /// The binary as configured.
        binary: PathBuf,
        /// What the harness pinned.
        expected: String,
        /// What the binary printed.
        found: String,
    },
}

// --- the auth probe (ADR-0013 §6) --------------------------------------------

/// The two commands a CLI answers about itself, and how to read the one
/// string the reply's `mode` carries.
///
/// This is the hook each adapter fills in. `claude auth status` prints JSON
/// carrying the user's email, organisation id and name; `codex login
/// status` prints one line, on stderr. Only [`mode_from_login`](Self::mode_from_login)
/// ever reaches a reply, and it returns **one field, never the document**,
/// because a reply is a blob the log keeps forever.
pub struct Probe {
    /// Arguments that make the binary print its version (`["--version"]`).
    pub version_args: Vec<String>,
    /// Arguments that make it report its login state
    /// (`["auth", "status"]`, `["login", "status"]`).
    pub login_args: Vec<String>,
    /// Reads the one opaque mode string out of what the login command
    /// printed, or `None` when the CLI states nothing the driver should
    /// carry. Never interpreted, never enumerated: the policy behind such
    /// an enum changed twice in 2026. Both streams are offered because
    /// `codex login status` 0.157.1 prints its line on stderr (#128 run 0).
    pub mode_from_login: fn(&LoginOutput) -> Option<String>,
}

/// What the login command printed, for [`Probe::mode_from_login`] to read
/// one field out of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoginOutput {
    /// Standard output, lines joined by `\n`.
    pub stdout: String,
    /// Standard error, as the run kept it.
    pub stderr: String,
}

impl std::fmt::Debug for Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Probe")
            .field("version_args", &self.version_args)
            .field("login_args", &self.login_args)
            .finish_non_exhaustive()
    }
}

/// What the login probe found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The CLI is logged in, and said this about how.
    Ready {
        /// The opaque mode string, verbatim, if the CLI stated one.
        mode: Option<String>,
    },
    /// The CLI is not logged in. The fix is a human logging in; the driver
    /// never tries, and never retries in a loop.
    Unavailable {
        /// The CLI's own text, for the reply.
        message: String,
    },
}

impl Verdict {
    /// The mode string, when there is one.
    #[must_use]
    pub fn mode(&self) -> Option<&str> {
        match self {
            Self::Ready { mode } => mode.as_deref(),
            Self::Unavailable { .. } => None,
        }
    }

    /// Whether a `send` may proceed.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

/// Runs the binary's version command (ADR-0013 §6, step 1).
///
/// # Errors
///
/// [`ConfigError::Binary`] if it cannot be run or says nothing,
/// [`ConfigError::Version`] if `expect_version` is set and the output does
/// not contain it.
pub fn probe_version(config: &AgentConfig, probe: &Probe) -> Result<String, ConfigError> {
    let run = bare_run(config, &probe.version_args).map_err(|reason| ConfigError::Binary {
        binary: config.binary.clone(),
        reason,
    })?;
    let found = first_line(&run);
    if found.is_empty() {
        return Err(ConfigError::Binary {
            binary: config.binary.clone(),
            reason: "it printed no version".to_owned(),
        });
    }
    match &config.expect_version {
        Some(expected) if !found.contains(expected.as_str()) => Err(ConfigError::Version {
            binary: config.binary.clone(),
            expected: expected.clone(),
            found,
        }),
        _ => Ok(found),
    }
}

/// Runs the CLI's own login-status command (ADR-0013 §6, step 2).
///
/// Never fails: being logged out is a [`Verdict::Unavailable`], not an
/// error, and so is a probe that could not run at all — a `send` answers
/// `error.unavailable` with the text either way.
#[must_use]
pub fn probe_login(config: &AgentConfig, probe: &Probe) -> Verdict {
    let args = probe.login_args.clone();
    let command = format!("{} {}", config.name, args.join(" "));
    let run = match bare_run(config, &args) {
        Ok(run) => run,
        Err(reason) => {
            return Verdict::Unavailable {
                message: format!("{command}: {reason}"),
            }
        }
    };
    let out = run
        .lines
        .iter()
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    match run.code() {
        Some(0) => Verdict::Ready {
            mode: (probe.mode_from_login)(&LoginOutput {
                stdout: out,
                stderr: run.stderr.clone(),
            }),
        },
        code => {
            let said = if run.stderr.trim().is_empty() {
                out.trim().to_owned()
            } else {
                run.stderr.trim().to_owned()
            };
            let how = code.map_or_else(|| "was signalled".to_owned(), |c| format!("exit {c}"));
            Verdict::Unavailable {
                message: format!("{command}: {how}: {said}").trim_end().to_owned(),
            }
        }
    }
}

/// Runs a short, non-interactive subprocess under the config's environment
/// and the probe bound, with no interrupt path worth climbing.
fn bare_run(config: &AgentConfig, args: &[String]) -> Result<Run, String> {
    let invocation = process::Invocation {
        program: config.binary.clone(),
        args: args.to_vec(),
        cwd: config.workspace_root.clone(),
        env: config.env.clone(),
        first_stdin: None,
        interrupt: process::Interrupt::Signal,
        terminal: |_| false,
    };
    let bounds = Bounds {
        wall: config.probe_wall,
        grace: config.abandon_grace,
        transcript_bytes: 64 * 1024,
        stderr_bytes: config.stderr_bytes,
    };
    process::run(&invocation, bounds, &Cancel::default()).map_err(|e| e.to_string())
}

/// The probe's stdout as one trimmed line.
fn first_line(run: &Run) -> String {
    run.lines
        .first()
        .map(|line| String::from_utf8_lossy(line).trim().to_owned())
        .unwrap_or_default()
}

/// The driver's memory of the login verdict, with ADR-0013 §6's re-probe
/// rule.
///
/// Every `send` while the verdict is unavailable answers `error.unavailable`
/// and re-probes **once**, so a user who logs in mid-run is not stuck behind
/// a stale verdict, and a CLI that is genuinely logged out costs one short
/// subprocess per refused `send` — never a retry loop. A run whose own
/// events name an authentication failure flips the verdict back, through
/// [`Availability::fail`].
#[derive(Debug)]
pub struct Availability {
    verdict: Mutex<Verdict>,
}

impl Availability {
    /// Remembers a verdict, usually the one the probe returned at
    /// construction.
    #[must_use]
    pub fn new(verdict: Verdict) -> Self {
        Self {
            verdict: Mutex::new(verdict),
        }
    }

    /// The verdict as it stands, without probing.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        lock(&self.verdict).clone()
    }

    /// The verdict a `send` should act on: the stored one when it is ready,
    /// and otherwise one fresh probe.
    pub fn check(&self, config: &AgentConfig, probe: &Probe) -> Verdict {
        {
            let current = lock(&self.verdict);
            if current.is_ready() {
                return current.clone();
            }
        }
        let fresh = probe_login(config, probe);
        *lock(&self.verdict) = fresh.clone();
        fresh
    }

    /// A run's own events named an authentication failure: the CLI is not
    /// logged in after all, whatever the probe said earlier.
    pub fn fail(&self, message: impl Into<String>) {
        *lock(&self.verdict) = Verdict::Unavailable {
            message: message.into(),
        };
    }
}

// --- request validation (ADR-0013 §2, the `unsupported` row) ------------------

/// A request the driver will act on: every field checked, the workspace
/// resolved, the bound tightened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted {
    /// Which op.
    pub op: Op,
    /// The work order, or the amendment.
    pub task: String,
    /// The session to continue, on a `resume`.
    pub session: Option<String>,
    /// The child's `cwd`, resolved under the configured root and proven to
    /// be inside it.
    pub workspace: PathBuf,
    /// The tools this session may use: the request's narrowing, or the
    /// configured set.
    pub tools: Vec<String>,
    /// The cost bound this run is held to, where the CLI enforces one.
    pub cost_microusd: Option<u64>,
    /// The turn bound this run is held to, where the CLI enforces one.
    pub turns: Option<u64>,
}

/// Parses and validates one `send` payload.
///
/// # Errors
///
/// A [`RunError`] with [`ErrorKind::Unsupported`] for anything the driver
/// will not run — a `v` it does not speak, an `op` this CLI cannot serve, a
/// `task` over the bound, a `workspace` outside the root, `tools` it cannot
/// honour, a `budget` above the bound — or [`ErrorKind::Host`] when the
/// workspace does not exist. Nothing is spawned either way, and nothing is
/// billed.
pub fn decode(payload: &[u8], config: &AgentConfig, caps: Caps) -> Result<Accepted, RunError> {
    let request: Request = serde_json::from_slice(payload).map_err(|e| {
        refusal(
            ErrorKind::Unsupported,
            format!("payload is not an agent v{VERSION} request: {e}"),
        )
    })?;
    accept(&request, config, caps)
}

/// Validates an already-parsed request. See [`decode`].
///
/// # Errors
///
/// As [`decode`].
pub fn accept(request: &Request, config: &AgentConfig, caps: Caps) -> Result<Accepted, RunError> {
    if let Some(v) = request.v() {
        if v != VERSION {
            return Err(refusal(
                ErrorKind::Unsupported,
                format!("agent version {v} is not supported; this driver speaks v{VERSION}"),
            ));
        }
    }
    if request.op() == Op::Resume && !caps.resume {
        return Err(refusal(
            ErrorKind::Unsupported,
            format!("`{}` cannot resume a session", config.name),
        ));
    }
    if request.task().len() > config.task_bytes {
        return Err(refusal(
            ErrorKind::Unsupported,
            format!(
                "task is {} bytes; the bound is {}",
                request.task().len(),
                config.task_bytes
            ),
        ));
    }
    if request.task().trim().is_empty() {
        return Err(refusal(ErrorKind::Unsupported, "task is empty"));
    }
    let tools = match request.tools() {
        None => config.tools.clone(),
        Some(_) if !caps.tools => {
            let name = &config.name;
            return Err(refusal(
                ErrorKind::Unsupported,
                format!(
                    "`{name}` has no per-session tool allowlist; the cage is its configuration"
                ),
            ));
        }
        Some(asked) => {
            let allowed: BTreeSet<&str> = config.tools.iter().map(String::as_str).collect();
            if let Some(stranger) = asked.iter().find(|name| !allowed.contains(name.as_str())) {
                return Err(refusal(
                    ErrorKind::Unsupported,
                    format!("tool `{stranger}` is not in this driver's set"),
                ));
            }
            asked.to_vec()
        }
    };
    let (cost_microusd, turns) = match request.budget() {
        None => (config.task_cost_microusd, config.task_turns),
        Some(_) if !caps.budget => {
            return Err(refusal(
                ErrorKind::Unsupported,
                format!("`{}` enforces no per-run budget", config.name),
            ))
        }
        Some(asked) => (
            tighten(
                asked.cost_microusd,
                config.task_cost_microusd,
                "cost_microusd",
            )?,
            tighten(asked.turns, config.task_turns, "turns")?,
        ),
    };
    let workspace = resolve(config, request.workspace())?;
    Ok(Accepted {
        op: request.op(),
        task: request.task().to_owned(),
        session: request.session().map(ToOwned::to_owned),
        workspace,
        tools,
        cost_microusd,
        turns,
    })
}

/// A request's bound may only narrow the configured one.
fn tighten(
    asked: Option<u64>,
    configured: Option<u64>,
    what: &str,
) -> Result<Option<u64>, RunError> {
    match (asked, configured) {
        (None, configured) => Ok(configured),
        (Some(asked), Some(bound)) if asked > bound => Err(refusal(
            ErrorKind::Unsupported,
            format!("budget.{what} {asked} is above the configured bound {bound}"),
        )),
        (Some(asked), _) => Ok(Some(asked)),
    }
}

/// Resolves a request's `workspace` under the configured root, and proves
/// it stayed inside — including through a symlink, which is why the check
/// is on the canonical path and not on the components alone.
fn resolve(config: &AgentConfig, asked: Option<&str>) -> Result<PathBuf, RunError> {
    let Some(asked) = asked.filter(|path| !path.is_empty()) else {
        return Ok(config.workspace_root.clone());
    };
    let relative = Path::new(asked);
    let ordinary = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    if !ordinary {
        return Err(refusal(
            ErrorKind::Unsupported,
            format!("workspace `{asked}` must be a plain path relative to the workspace root"),
        ));
    }
    let root = config
        .workspace_root
        .canonicalize()
        .map_err(|e| refusal(ErrorKind::Host, format!("workspace root: {e}")))?;
    let full = root.join(relative).canonicalize().map_err(|e| {
        refusal(
            ErrorKind::Host,
            format!("workspace `{asked}` is not a directory: {e}"),
        )
    })?;
    if !full.starts_with(&root) {
        return Err(refusal(
            ErrorKind::Unsupported,
            format!("workspace `{asked}` resolves outside the workspace root"),
        ));
    }
    if !full.is_dir() {
        return Err(refusal(
            ErrorKind::Host,
            format!("workspace `{asked}` is not a directory"),
        ));
    }
    Ok(full)
}

/// A [`RunError`], for the one line that builds them all.
#[must_use]
pub fn refusal(kind: ErrorKind, message: impl Into<String>) -> RunError {
    RunError {
        kind,
        message: message.into(),
    }
}

// --- what an adapter reads out of a run --------------------------------------

/// What an adapter read out of the CLI's event stream.
///
/// The other half of the seam: whatever the CLI's events look like, this is
/// the shape the shared reply is built from. An adapter that cannot find a
/// field leaves it `None`; the reply says `null` and the caller sees that
/// the CLI did not state it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// How the run ended, as the CLI's own events describe it.
    pub stop: Option<Stop>,
    /// The session id the CLI reported.
    pub session: Option<String>,
    /// The model the CLI said answered, verbatim.
    pub model: Option<String>,
    /// The opaque mode string, verbatim.
    pub mode: Option<String>,
    /// What the CLI's terminal event reported.
    pub usage: Usage,
    /// The final assistant message, for the envelope parser.
    pub final_message: Option<Vec<u8>>,
}

/// Builds the reply and the bill from a finished run (ADR-0013 §2, §5, §6).
///
/// This is where the ladder meets the accounting. The rules, once:
///
/// | what happened | `stop` | billed |
/// |---|---|---|
/// | the CLI reported its terminal event | the adapter's `stop`, or `done` | what the CLI reported |
/// | the interrupt was answered | `abandoned` | what the CLI reported |
/// | a signal was needed, or nothing was reported | `abandoned` / `{limit: wall}` | **the ceiling** |
/// | `SIGKILL` was needed | `error.lost` | **the ceiling** |
/// | the final message was not an envelope | `error.envelope` | what the CLI reported |
#[must_use]
pub fn settle(
    config: &AgentConfig,
    version: &str,
    run: &Run,
    outcome: &Outcome,
) -> (Reply, Consumption) {
    let reported = run.usage_is_reported();
    let (stop, envelope) = conclude(run, outcome);
    let consumed = if reported {
        bill(config, Billing::Reported(outcome.usage))
    } else {
        bill(config, Billing::Ceiling)
    };
    let reply = Reply {
        v: VERSION,
        stop,
        envelope,
        session: outcome.session.clone(),
        cli: wire::Cli {
            name: config.name.clone(),
            version: version.to_owned(),
        },
        model: outcome.model.clone(),
        mode: outcome.mode.clone(),
        usage: if reported {
            outcome.usage
        } else {
            Usage::default()
        },
        transcript: run.transcript(),
        truncated: Truncated {
            transcript: run.truncated,
        },
    };
    (reply, consumed)
}

/// The `stop` and the envelope for a finished run.
fn conclude(run: &Run, outcome: &Outcome) -> (Stop, Option<Envelope>) {
    use process::{Ending, Rung};
    match run.ending {
        Ending::Abandoned(Rung::Kill) | Ending::WallLimit(Rung::Kill) => (
            Stop::Error(refusal(
                ErrorKind::Lost,
                "the CLI ignored every signal and was killed; billed at the ceiling",
            )),
            None,
        ),
        Ending::Abandoned(_) => (Stop::Abandoned, None),
        Ending::WallLimit(_) => (Stop::Limit(Limit::Wall), None),
        Ending::Completed if !run.terminal_seen() => (
            Stop::Error(refusal(
                ErrorKind::Lost,
                format!(
                    "the CLI ended{} without its terminal event; billed at the ceiling",
                    run.code().map(|c| format!(" with {c}")).unwrap_or_default()
                ),
            )),
            None,
        ),
        Ending::Completed => match outcome.stop.clone() {
            // A stop the adapter read out of the CLI's own events —
            // a limit it hit, or an error it reported — stands as it is.
            Some(stop) if !matches!(stop, Stop::Done) => (stop, None),
            _ => envelope_stop(outcome),
        },
    }
}

/// `done` with an envelope, or `error.envelope` with the transcript.
///
/// An `EnvelopeViolation` is never rewritten into `{"status":"failed"}`: a
/// synthesized envelope is indistinguishable from one the session wrote,
/// and the caller would not know the contract was broken.
fn envelope_stop(outcome: &Outcome) -> (Stop, Option<Envelope>) {
    let Some(message) = outcome.final_message.as_deref() else {
        return (
            Stop::Error(refusal(
                ErrorKind::Envelope,
                "the CLI reported no final message",
            )),
            None,
        );
    };
    match envelope::parse(message) {
        Ok(envelope) => (Stop::Done, Some(envelope)),
        Err(violation) => (
            Stop::Error(refusal(ErrorKind::Envelope, violation.reason)),
            None,
        ),
    }
}

/// What a reply bills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Billing {
    /// Nothing ran: `error.unsupported`, `error.host`, `error.unavailable`
    /// before a spawn, or an abandon that arrived first.
    Nothing,
    /// The CLI stated what it used.
    Reported(Usage),
    /// Something ran and the driver cannot say how much. Billed *up*, never
    /// down: the reservation was already taken, so nothing new leaves the
    /// agent, and a driver that guessed low would be the one place where a
    /// cancel is cheaper than a completion.
    Ceiling,
}

/// Turns a [`Billing`] into what the kernel settles against.
///
/// `cost_microusd` is the CLI's own figure when it stated one — `claude`
/// states a list-price cost under a subscription login too, and the driver
/// attaches no opinion about whether a bill exists — and otherwise the
/// tokens at the configured prices, if the harness configured any. A
/// harness that would rather not budget in list-price dollars leaves
/// `cost_microusd` out of its grants and meters in `tokens`.
///
/// Not reported, on purpose: `calls` (the kernel adds one at `send`),
/// `wall_ms` (the reducer charges it on every `Tick` the requester waits
/// through), and `compute_ms` (the CLI's local CPU is not what is paid for).
#[must_use]
pub fn bill(config: &AgentConfig, billing: Billing) -> Consumption {
    match billing {
        Billing::Nothing => Consumption::none(),
        Billing::Ceiling => {
            let mut dims = vec![(DimKey::Tokens, config.task_tokens)];
            if let Some(cost) = config.cost_ceiling() {
                dims.push((DimKey::CostMicroUsd, cost));
            }
            Consumption::from_dims(dims)
        }
        Billing::Reported(usage) => {
            let mut dims = vec![(DimKey::Tokens, usage.tokens())];
            if let Some(cost) = usage.cost_microusd.or_else(|| derive_cost(config, usage)) {
                dims.push((DimKey::CostMicroUsd, cost));
            }
            Consumption::from_dims(dims)
        }
    }
}

/// The cost of `usage` at the configured prices, when there are any.
fn derive_cost(config: &AgentConfig, usage: Usage) -> Option<u64> {
    let input = config.input_price_microusd;
    let output = config.output_price_microusd;
    if input.is_none() && output.is_none() {
        return None;
    }
    Some(
        usage
            .input_tokens
            .saturating_mul(input.unwrap_or(0))
            .saturating_add(usage.output_tokens.saturating_mul(output.unwrap_or(0))),
    )
}

/// A reply that ran nothing.
#[must_use]
pub fn refused(config: &AgentConfig, version: &str, error: RunError) -> Reply {
    Reply {
        v: VERSION,
        stop: Stop::Error(error),
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

/// A reply, as bytes. Serializing these types cannot fail in practice; if
/// it ever does, the reply says so instead of being empty.
#[must_use]
pub fn encode(reply: &Reply) -> Vec<u8> {
    serde_json::to_vec(reply).unwrap_or_else(|_| {
        format!(
            r#"{{"v":{VERSION},"stop":{{"error":{{"kind":"host","message":"reply could not be serialized"}}}},"envelope":null,"session":null,"cli":{{"name":"","version":""}},"model":null,"mode":null,"usage":{{"input_tokens":0,"output_tokens":0,"cost_microusd":null,"turns":null}},"transcript":[],"truncated":{{"transcript":false}}}}"#
        )
        .into_bytes()
    })
}

// --- the flight registry ------------------------------------------------------

/// The runs `Driver::abandon` can reach: `corr → cancel`, and the corrs
/// abandoned before the driver ever saw them.
///
/// A `BTreeMap`, not a `HashMap`: hash-ordered containers are denied
/// workspace-wide, because the reducer is a pure fold and iteration order
/// that differs between machines is a replay bug.
#[derive(Debug, Default)]
pub struct Flights {
    state: Mutex<FlightsState>,
}

#[derive(Debug, Default)]
struct FlightsState {
    open: BTreeMap<Corr, Arc<Cancel>>,
    abandoned_early: BTreeSet<Corr>,
}

impl Flights {
    /// Registers a run, returning its handle and whether `abandon` already
    /// arrived for it.
    ///
    /// Registered when the delivery is taken, not when its future is first
    /// polled: `abandon` may arrive in between, and it must find the entry.
    pub fn enter(self: &Arc<Self>, corr: Corr) -> (Flight, bool) {
        let mut state = lock(&self.state);
        let early = state.abandoned_early.remove(&corr);
        let cancel = Arc::new(Cancel::default());
        state.open.insert(corr, Arc::clone(&cancel));
        drop(state);
        (
            Flight {
                flights: Arc::clone(self),
                corr,
                cancel,
            },
            early,
        )
    }

    /// The requester of `corr` was cancelled.
    pub fn abandon(&self, corr: Corr) {
        let mut state = lock(&self.state);
        match state.open.get(&corr) {
            Some(cancel) => cancel.abandon(),
            None => {
                state.abandoned_early.insert(corr);
            }
        }
    }

    /// Abandons every open run: what a driver's `Drop` calls, so a harness
    /// shutting down leaves no CLI behind.
    pub fn abandon_all(&self) {
        let state = lock(&self.state);
        for cancel in state.open.values() {
            cancel.abandon();
        }
    }

    /// How many runs are open right now.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        lock(&self.state).open.len()
    }
}

/// One run's registration, removed when the run ends however it ends.
#[derive(Debug)]
pub struct Flight {
    flights: Arc<Flights>,
    corr: Corr,
    cancel: Arc<Cancel>,
}

impl Flight {
    /// The cancel handle to hand to [`process::run`].
    #[must_use]
    pub fn cancel(&self) -> &Cancel {
        &self.cancel
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        lock(&self.flights.state).open.remove(&self.corr);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Single-lines every `description` in a derived schema, and optionally
/// drops the `default` keywords: a tool schema is read by a model, and a
/// strict final-message schema has no defaults to speak of.
pub(crate) fn tidy(value: &mut Value, drop_defaults: bool) {
    match value {
        Value::Object(object) => {
            if drop_defaults {
                object.remove("default");
            }
            if let Some(Value::String(text)) = object.get_mut("description") {
                *text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            }
            for (_, child) in object.iter_mut() {
                tidy(child, drop_defaults);
            }
        }
        Value::Array(items) => {
            for item in items {
                tidy(item, drop_defaults);
            }
        }
        _ => {}
    }
}
