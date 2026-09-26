//! The generic agent driver (ADR-0013 §1): one `impl Driver` over
//! decode → flights → run → settle, and the sealed [`Cli`] trait that names
//! what a CLI adapter supplies to it.
//!
//! Before this file, `claude.rs` and `codex.rs` each carried the whole
//! driver: the same flight registry, the same run thread and one-shot
//! channel, the same `Inner`/`Shared` split, the same `abandoned()` reply.
//! What differed was two functions — the [`Invocation`] going in and the
//! [`Outcome`] coming out — plus the caps, the probe, and two small habits
//! (`codex` writes a schema file per run and reads its login mode from the
//! probe). Those are the trait's methods; everything else is here, once.
//!
//! ```text
//! payload ──decode(C::CAPS)──▶ Accepted ──C::invocation──▶ the CLI ──▶ lines
//!                                                                      │
//!                                                     C::outcome ◀─────┘
//!                                                          │
//!                                           settle ◀───────┘ ──▶ (Reply, Consumption)
//! ```
//!
//! # Why the trait is public and sealed
//!
//! ADR-0013 §1 wrote `Cli` as crate-private, and its 2026-09-20 amendment
//! deferred it for three reasons. A crate-private trait cannot bound a
//! public generic, so the trait is `pub` — and sealed, so that only this
//! crate's adapters implement it and its method set can grow without a
//! breaking change. A trait with no implementors is dead code under `-D
//! warnings`, and nothing under `tests/` can implement a sealed trait; both
//! answered by the two adapters that now exist and by the ladder tests,
//! which run the real drivers over `tau-fake-cli`. Every method returns
//! the same two seam types the adapters already built, which is what makes
//! the extraction a move and not a redesign.

use std::sync::Arc;

use tau_kernel::abi::{Budget as KernelBudget, Consumption, Corr};
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{BoxFuture, Delivery};

use super::process::{self, Cancel, Invocation, Run};
use super::wire::{self, Caps, ErrorKind, Reply, RunError, Stop, Truncated, Usage, VERSION};
use super::{
    decode, encode, probe_login, probe_version, refusal, refused, settle, Accepted, AgentConfig,
    Availability, ConfigError, Flights, Outcome, Probe, Verdict,
};

/// The seal: a supertrait only this module's descendants can name, so that
/// [`Cli`] has exactly the implementors this crate ships.
pub(super) mod sealed {
    /// Implemented by each adapter beside its `impl Cli`.
    pub trait Sealed {}
}

/// What one CLI adapter supplies to [`AgentDriver`]: which requests it can
/// honour, how to probe it, how to build a run's command line, and how to
/// read the run back. Sealed: the implementors are `claude::Claude` and
/// `codex::Codex`, and a harness registers `AgentDriver<Claude>`, never
/// something of its own.
///
/// Every method is pure over its arguments except [`scratch`](Self::scratch),
/// which may touch the host's temporary directory. Nothing here spawns a
/// process, reads a clock, or reads a login: the driver spawns, the CLI
/// owns its login, and time is the kernel's.
pub trait Cli: sealed::Sealed + Default + Send + Sync + 'static {
    /// What this CLI can honour of the wire (ADR-0013 §2): a per-session
    /// tool allowlist, a per-run budget, `resume`.
    const CAPS: Caps;

    /// What a run needs on disk before it starts and until the process
    /// ends: `codex` writes the strict envelope schema to a file per run,
    /// because `--output-schema` takes a path; `claude` needs nothing and
    /// uses `()`. Dropped the moment the process has ended, before the
    /// outcome is read.
    type Scratch;

    /// The two commands the CLI answers about itself, and how to read the
    /// login mode out of the second (ADR-0013 §6).
    fn probe() -> Probe;

    /// Prepares the per-run scratch for `corr`, or refuses the run without
    /// spawning anything. The error is the reply's `stop`, nothing billed.
    ///
    /// # Errors
    ///
    /// A [`RunError`] — `error.host` when the scratch could not be
    /// written.
    fn scratch(&self, corr: Corr) -> Result<Self::Scratch, RunError>;

    /// The command line, the first stdin message, the interrupt, and which
    /// printed line is terminal (ADR-0013 §7), for one accepted request.
    fn invocation(
        &self,
        config: &AgentConfig,
        accepted: &Accepted,
        scratch: &Self::Scratch,
    ) -> Invocation;

    /// What the run's lines said (ADR-0013 §7). `verdict` is the login
    /// verdict this run proceeded under, always `Ready`: `codex` reads the
    /// reply's `mode` from it, `claude` from its own `init` event.
    fn outcome(&self, run: &Run, verdict: &Verdict) -> Outcome;

    /// A run the CLI refused before starting a turn — exited without a
    /// terminal event for a reason its exit and stderr make plain — as the
    /// reply it should get instead of `settle`'s `error.lost` at the
    /// ceiling. `None`, the default, hands the run to `settle`. The hook for
    /// `codex`'s unknown-thread `resume` row, once it is pinned (#223).
    fn refused_before_start(&self, run: &Run) -> Option<RunError> {
        let _ = run;
        None
    }

    /// What the run said about being logged out, if anything: the message
    /// to flip the driver's verdict with, whatever the probe said earlier
    /// (ADR-0013 §6). The default reads the outcome's `stop`, which is
    /// where a CLI that fails fast puts it (`claude`, #194 run 11). A CLI
    /// that retries on `401` and never reports it as its stop (`codex`,
    /// #130 §3) overrides this and reads its `error` events.
    fn auth_failure(&self, run: &Run, outcome: &Outcome) -> Option<String> {
        let _ = run;
        match &outcome.stop {
            Some(Stop::Error(error)) if error.kind == ErrorKind::Unavailable => {
                Some(error.message.clone())
            }
            _ => None,
        }
    }
}

/// An agent driver over one CLI: one registration, one tool, one CLI
/// binary under one login the driver never sees. Cheap to clone; every
/// clone shares one flight registry, and dropping the last clone abandons
/// every open run.
///
/// `claude::ClaudeDriver` and `codex::CodexDriver` are this type over their
/// adapters.
pub struct AgentDriver<C: Cli> {
    inner: Arc<Inner<C>>,
}

impl<C: Cli> Clone for AgentDriver<C> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct Inner<C: Cli> {
    shared: Arc<Shared<C>>,
    flights: Arc<Flights>,
    ceiling: KernelBudget,
    schema: ToolSchema,
}

/// What a run thread needs, and nothing that would keep [`Inner`] alive: a
/// run holds this, not `Inner`, so that dropping the last driver handle
/// runs `Inner`'s `Drop` while runs are still open.
struct Shared<C: Cli> {
    cli: C,
    config: AgentConfig,
    version: String,
    probe: Probe,
    availability: Availability,
}

impl<C: Cli> Drop for Inner<C> {
    fn drop(&mut self) {
        // A harness shutting down leaves no CLI behind (ADR-0013 §5).
        self.flights.abandon_all();
    }
}

impl<C: Cli> AgentDriver<C> {
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
        let probe = C::probe();
        let version = probe_version(&config, &probe)?;
        let availability = Availability::new(probe_login(&config, &probe));
        let ceiling = config.ceiling();
        let schema = ToolSchema {
            description: config.describe(C::CAPS),
            input_schema: serde_json::to_vec(&wire::schema(C::CAPS)).unwrap_or_default(),
        };
        Ok(Self {
            inner: Arc::new(Inner {
                shared: Arc::new(Shared {
                    cli: C::default(),
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

impl<C: Cli> std::fmt::Debug for AgentDriver<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentDriver")
            .field("config", &self.inner.shared.config)
            .field("version", &self.inner.shared.version)
            .field("in_flight", &self.in_flight())
            .finish_non_exhaustive()
    }
}

impl<C: Cli> Driver for AgentDriver<C> {
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
            .name(format!("tau-agent-{}", shared.config.name))
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

impl<C: Cli> Shared<C> {
    /// One `send`, on its own thread: decode, the availability check, the
    /// scratch, the run, the read-back, the settlement.
    fn run(&self, corr: Corr, payload: &[u8], cancel: &Cancel) -> (Reply, Consumption) {
        let config = &self.config;
        let refuse = |error: RunError| (refused(config, &self.version, error), Consumption::none());
        let accepted = match decode(payload, config, C::CAPS) {
            Ok(accepted) => accepted,
            Err(error) => return refuse(error),
        };
        // Refused while logged out, and re-probed once per refused `send`
        // (ADR-0013 §6): never a retry loop, never a login attempt. For
        // `codex` this is the gate that works: an unsigned run retries on
        // 401 and never reports "not authenticated" on its own (#130 §3).
        let verdict = self.availability.check(config, &self.probe);
        if let Verdict::Unavailable { message } = &verdict {
            return refuse(refusal(ErrorKind::Unavailable, message.clone()));
        }
        let scratch = match self.cli.scratch(corr) {
            Ok(scratch) => scratch,
            Err(error) => return refuse(error),
        };
        let invocation = self.cli.invocation(config, &accepted, &scratch);
        let run = match process::run(&invocation, config.bounds(), cancel) {
            Ok(run) => run,
            Err(e) => {
                return refuse(refusal(
                    ErrorKind::Host,
                    format!("cannot start `{}`: {e}", config.binary.display()),
                ))
            }
        };
        drop(scratch);
        if let Some(error) = self.cli.refused_before_start(&run) {
            return refuse(error);
        }
        let outcome = self.cli.outcome(&run, &verdict);
        if let Some(message) = self.cli.auth_failure(&run, &outcome) {
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
