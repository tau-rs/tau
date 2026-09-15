//! The sandbox wire shape (ADR-0009 §2): one request per `send`, one reply
//! per `Reply`, both JSON, versioned by [`VERSION`].
//!
//! The JSON is the contract; these types are versioned with the crate. A
//! program that builds requests by hand writes the JSON, sets `v`, and is
//! refused loudly on a mismatch. A model never sees `v`: the schema
//! [`schema`] projects has only `code` and `stdin`, with
//! `additionalProperties: false`, so an absent `v` means "the version the
//! schema was projected at" — never a guess.

use schemars::{JsonSchema, SchemaGenerator};
use serde::{Deserialize, Serialize};

/// The sandbox wire version these types speak.
pub const VERSION: u16 = 1;

/// One run: the code, and what to feed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// The wire version. Absent means the version `describe()` projected,
    /// which is [`VERSION`]; a present value the driver does not implement
    /// is `error.unsupported`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub v: Option<u16>,
    /// The program.
    pub code: String,
    /// Standard input for the program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub stdin: Option<String>,
}

/// What one run produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    /// The wire version, [`VERSION`].
    pub v: u16,
    /// How the run ended.
    pub stop: Stop,
    /// The first `output_bytes` of standard output, as UTF-8, lossily.
    pub stdout: String,
    /// The first `output_bytes` of standard error, as UTF-8, lossily.
    pub stderr: String,
    /// Whether either stream was cut at the bound.
    pub truncated: Truncated,
    /// What the run used, in the interpreter's native units. Information
    /// for the caller, not the accounting: that is the `Consumption`.
    pub usage: Usage,
}

/// How a run ended (ADR-0009 §2, the `stop` table).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    /// The interpreter exited with this status. `0` is not special.
    Exit(i32),
    /// The interpreter died of a signal it did not ask for, by name.
    Signal(String),
    /// The host kernel killed it at `RLIMIT_CPU`.
    CpuLimit,
    /// The shim killed it at the wall bound.
    WallLimit,
    /// The shim killed it because the requester was cancelled.
    Abandoned,
    /// The driver could not run it, or lost it.
    Error(RunError),
}

/// Why the driver could not run, or lost, a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunError {
    /// Which kind.
    pub kind: ErrorKind,
    /// For a human, or a model.
    pub message: String,
}

/// The kinds of [`RunError`], and what each bills (ADR-0009 §2, §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// A `v`, a field, or a `code` size the driver cannot honour. Nothing
    /// ran; nothing billed.
    Unsupported,
    /// The driver could not start the run. Nothing ran; nothing billed.
    Host,
    /// The run started and its report never arrived. Billed at the ceiling.
    Lost,
}

/// Which streams were cut at the output bound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truncated {
    /// Standard output lost bytes past the bound.
    pub stdout: bool,
    /// Standard error lost bytes past the bound.
    pub stderr: bool,
}

/// CPU time and peak memory, from `getrusage(RUSAGE_CHILDREN)` in the shim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// User CPU time, microseconds.
    pub cpu_user_us: u64,
    /// System CPU time, microseconds.
    pub cpu_sys_us: u64,
    /// Peak resident set size, bytes. Not summable across calls, so it has
    /// no budget dimension.
    pub max_rss_bytes: u64,
}

impl Usage {
    /// `compute_ms`: user plus system time in milliseconds, rounded up.
    #[must_use]
    pub fn compute_ms(self) -> u64 {
        self.cpu_user_us
            .saturating_add(self.cpu_sys_us)
            .div_ceil(1_000)
    }
}

/// The input schema `describe()` projects: derived from [`Request`], the
/// type the driver deserializes with, so there is one source of truth
/// (ADR-0009 §7). Draft 2020-12, no `$schema` or `title`, and
/// `additionalProperties: false`.
#[must_use]
pub fn schema() -> serde_json::Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12().with(|s| {
        s.meta_schema = None;
    });
    let mut schema = SchemaGenerator::new(settings).root_schema_for::<Request>();
    if let Some(object) = schema.as_object_mut() {
        object.remove("title");
    }
    schema.to_value()
}
