//! The agent wire shape (ADR-0013 §2): one [`Request`] per `send`, one
//! [`Reply`] per `Reply`, both JSON, versioned by [`VERSION`].
//!
//! The JSON is the contract; these types are versioned with the crate, not
//! with the kernel's ABI, and they live here rather than in
//! `tau_kernel::bridge` because no second lane needs the shape and the
//! kernel carries no vocabulary it has no neighbour for (ADR-0013 §2).
//!
//! What the request deliberately does *not* carry: `model`, `effort`,
//! `permission_mode`, an absolute `workspace`, or `tools` outside the
//! configured set. Each is the harness's decision at registration, because
//! the requester may itself be a model and a model does not size its own
//! cage (ADR-0009). Each is one optional, defaulted field away on the day a
//! program needs it, without a `v` bump.

use schemars::{JsonSchema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::envelope::Envelope;

/// The agent wire version these types speak.
pub const VERSION: u16 = 1;

/// Which of the wire's optional surfaces a given CLI can honour (ADR-0013
/// §7, §9).
///
/// A driver whose CLI has no per-session tool allowlist refuses a present
/// `tools` rather than folding it into the prompt, and omits the field from
/// the schema it projects, so a model never writes one. Same for `budget`,
/// and for the `resume` op — which `codex exec` could not serve at 0.46.0,
/// because `exec resume` had no JSON stream to read a terminal event from;
/// 0.157.1 has one (#128 run 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    /// The CLI has a per-session tool allowlist.
    pub tools: bool,
    /// The CLI enforces a per-run cost or turn bound.
    pub budget: bool,
    /// The CLI can continue a session it already has, with a JSON stream.
    pub resume: bool,
}

impl Caps {
    /// Everything the wire offers.
    pub const ALL: Self = Self {
        tools: true,
        budget: true,
        resume: true,
    };
    /// Only what every CLI can do: one fresh session per `send`.
    pub const RUN_ONLY: Self = Self {
        tools: false,
        budget: false,
        resume: false,
    };
}

impl Default for Caps {
    fn default() -> Self {
        Self::ALL
    }
}

/// One `send`: a whole task, or an amendment to a session the CLI already
/// has. Tagged by `op`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Start a fresh CLI session in `workspace`.
    Run {
        /// The wire version. Absent means the version `describe()`
        /// projected, which is [`VERSION`]; a present value the driver does
        /// not implement is `error.unsupported`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(skip)]
        v: Option<u16>,
        /// The work order.
        task: String,
        /// A path relative to the configured workspace root; the child's
        /// `cwd`. Absent is the root itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<String>,
        /// The CLI-native tool names this session may use. Must be a subset
        /// of the configured set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools: Option<Vec<String>>,
        /// A tightening of the registered task bound.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        budget: Option<Budget>,
    },
    /// Continue the session a previous reply named.
    Resume {
        /// See [`Request::Run`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(skip)]
        v: Option<u16>,
        /// The session id a previous reply reported. Unknown to the CLI is
        /// `error.provider` with the CLI's own text.
        session: String,
        /// The amendment.
        task: String,
        /// See [`Request::Run`]; the same workspace as the original run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<String>,
    },
}

/// Which op a [`Request`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// A fresh session.
    Run,
    /// A continuation.
    Resume,
}

impl Op {
    /// The wire form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Resume => "resume",
        }
    }
}

impl Request {
    /// Which op this is.
    #[must_use]
    pub fn op(&self) -> Op {
        match self {
            Self::Run { .. } => Op::Run,
            Self::Resume { .. } => Op::Resume,
        }
    }

    /// The wire version the caller asked in, if it said.
    #[must_use]
    pub fn v(&self) -> Option<u16> {
        match self {
            Self::Run { v, .. } | Self::Resume { v, .. } => *v,
        }
    }

    /// The work order, or the amendment.
    #[must_use]
    pub fn task(&self) -> &str {
        match self {
            Self::Run { task, .. } | Self::Resume { task, .. } => task,
        }
    }

    /// The workspace, relative to the configured root.
    #[must_use]
    pub fn workspace(&self) -> Option<&str> {
        match self {
            Self::Run { workspace, .. } | Self::Resume { workspace, .. } => workspace.as_deref(),
        }
    }

    /// The per-session tool allowlist, on a `run`.
    #[must_use]
    pub fn tools(&self) -> Option<&[String]> {
        match self {
            Self::Run { tools, .. } => tools.as_deref(),
            Self::Resume { .. } => None,
        }
    }

    /// The requested tightening of the task bound, on a `run`.
    #[must_use]
    pub fn budget(&self) -> Option<&Budget> {
        match self {
            Self::Run { budget, .. } => budget.as_ref(),
            Self::Resume { .. } => None,
        }
    }

    /// The session to continue, on a `resume`.
    #[must_use]
    pub fn session(&self) -> Option<&str> {
        match self {
            Self::Resume { session, .. } => Some(session),
            Self::Run { .. } => None,
        }
    }
}

/// A tightening of the registered task bound, honoured where the CLI
/// enforces it and refused where it does not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// At or under the configured cost bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_microusd: Option<u64>,
    /// At or under the configured turn bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<u64>,
}

/// What one run produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    /// The wire version, [`VERSION`].
    pub v: u16,
    /// How the run ended.
    pub stop: Stop,
    /// The worker envelope parsed from the CLI's final message, or `null`
    /// when there was none to parse — which is every `limit`, every
    /// `abandoned`, and every error.
    pub envelope: Option<Envelope>,
    /// The session id the CLI reported, or `null` if it never did. What a
    /// `resume` takes.
    pub session: Option<String>,
    /// The binary's name and the version it printed at construction.
    pub cli: Cli,
    /// The model the CLI said answered, verbatim, or `null`.
    pub model: Option<String>,
    /// **Verbatim, opaque.** The one string the CLI itself stated about how
    /// it is logged in, or `null`. The driver never interprets it and no
    /// type here enumerates it: the policy behind such an enum changed
    /// twice in 2026, and a verbatim string is stable by construction.
    ///
    /// One string, never the whole probe output: `claude auth status`
    /// carries the user's email and organisation, and a reply is a blob the
    /// log keeps forever.
    pub mode: Option<String>,
    /// What the CLI's terminal event reported. Information for the caller;
    /// the accounting is the `Consumption`.
    pub usage: Usage,
    /// The CLI's own event stream, one JSON value per stdout line, parsed
    /// and carried unread. Lines that are not JSON are carried as
    /// `{ "raw": "…" }`.
    pub transcript: Vec<Value>,
    /// Whether the transcript was cut at the bound.
    pub truncated: Truncated,
}

/// The binary a reply came from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cli {
    /// What the harness registered it as (`claude`, `codex`).
    pub name: String,
    /// What it printed at construction (`2.1.272 (Claude Code)`).
    pub version: String,
}

/// How a run ended (ADR-0013 §2, the `stop` table).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    /// The CLI emitted its terminal event with a final message and exited.
    /// `envelope.status` says whether the *task* succeeded; `done` says the
    /// *run* did.
    Done,
    /// The CLI, or the driver's wall bound, stopped the session at a bound.
    /// No envelope: the model got no turn to write one.
    Limit(Limit),
    /// The requester was cancelled and the CLI stopped on the ladder of
    /// ADR-0013 §5 before the last rung.
    Abandoned,
    /// The driver could not run it, the CLI refused, or the run was lost.
    Error(RunError),
}

/// Which bound ended a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Limit {
    /// The CLI's own turn bound.
    Turns,
    /// The CLI's own cost bound.
    Cost,
    /// The driver's wall bound, which climbs the cancel ladder.
    Wall,
}

/// Why the driver could not run, could not finish, or lost a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunError {
    /// Which kind.
    pub kind: ErrorKind,
    /// For a human, or a model.
    pub message: String,
}

/// The kinds of [`RunError`], and what each bills (ADR-0013 §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// A `v`, an `op`, a field, a `tools` name outside the configured set,
    /// a `budget` above the bound, a `workspace` outside the root, or a
    /// `task` over the bound. Nothing spawned, nothing billed.
    Unsupported,
    /// The driver could not start the child: binary missing, workspace not
    /// a directory, spawn refused. Nothing ran, nothing billed.
    Host,
    /// The CLI is not logged in. The fix is a human logging in; the driver
    /// never tries.
    Unavailable,
    /// The CLI's terminal event names a rate limit or a quota window.
    Throttled,
    /// Any other CLI-side error: a provider 4xx/5xx, a refusal the CLI
    /// surfaced, an unknown `session`.
    Provider,
    /// The run finished with a final message that was not an envelope, even
    /// after the tolerant parse. The transcript carries what was said.
    Envelope,
    /// The run started and its terminal event never arrived. Something ran
    /// and the driver cannot say how much: billed at the ceiling.
    Lost,
}

/// What the CLI's terminal event reported (ADR-0013 §2, §6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Every input class the CLI reported, summed: uncached, cache
    /// creation, cache read. The transcript keeps the breakdown.
    ///
    /// Cache reads count. They are the bulk of an agent task's input
    /// (#130 run 1: 4 uncached, 23,851 created, 20,270 read), they are what
    /// the provider meters, and dropping them would make a long session
    /// look cheaper than a short one.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// The cost the CLI stated, in microdollars, rounded up — or `null`.
    pub cost_microusd: Option<u64>,
    /// The turns the CLI stated, or `null`.
    pub turns: Option<u64>,
}

impl Usage {
    /// Input plus output: the `tokens` dimension.
    #[must_use]
    pub fn tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// Which parts of a reply were cut at a bound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truncated {
    /// The transcript lost lines past `transcript_bytes`. The first events
    /// up to the bound are kept, and always the terminal one.
    pub transcript: bool,
}

/// The input schema `describe()` projects (ADR-0013 §9): derived from
/// [`Request`], the type the driver deserializes with, so there is one
/// source of truth (ADR-0006 §5).
///
/// `v` is skipped — a model never sets it, and an absent `v` is the version
/// the schema was projected at, never a guess. `caps` removes what the CLI
/// would refuse: a `oneOf` of `run` and `resume` when it can resume, and a
/// single flat object when it cannot.
#[must_use]
pub fn schema(caps: Caps) -> Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12().with(|s| {
        s.meta_schema = None;
        s.inline_subschemas = true;
    });
    let derived = SchemaGenerator::new(settings)
        .root_schema_for::<Request>()
        .to_value();
    let mut branches: Vec<Value> = derived
        .get("oneOf")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !caps.resume {
        branches.retain(|branch| branch_op(branch) != Some("resume"));
    }
    for branch in &mut branches {
        if branch_op(branch) == Some("run") {
            if !caps.tools {
                remove_property(branch, "tools");
            }
            if !caps.budget {
                remove_property(branch, "budget");
            }
        }
        // The branch carries the shape and closes itself; the root carries
        // the object-ness. `additionalProperties: false` must sit beside
        // the `properties` it closes: on the root, next to a `oneOf`, it
        // sees no `properties` of its own and refuses every key (draft
        // 2020-12 does not look into applicators) — which is what #193's
        // loop test found, and what `additionalProperties`'s own semantics
        // say.
        if let Some(object) = branch.as_object_mut() {
            object.remove("type");
            object.remove("title");
            object.remove("description");
            object.insert("additionalProperties".to_owned(), Value::Bool(false));
        }
    }
    let mut root = serde_json::Map::new();
    root.insert("type".to_owned(), Value::String("object".to_owned()));
    match <[Value; 1]>::try_from(branches) {
        // One op left: a flat object reads better than a one-armed `oneOf`,
        // and its `additionalProperties` lands on the root with its
        // `properties`.
        Ok([only]) => {
            if let Some(object) = only.as_object() {
                for (key, value) in object {
                    root.insert(key.clone(), value.clone());
                }
            }
        }
        Err(branches) => {
            root.insert("oneOf".to_owned(), Value::Array(branches));
        }
    }
    let mut schema = Value::Object(root);
    super::tidy(&mut schema, false);
    schema
}

/// The `op` a derived `oneOf` branch is for.
fn branch_op(branch: &Value) -> Option<&str> {
    branch
        .get("properties")
        .and_then(|p| p.get("op"))
        .and_then(|op| op.get("const"))
        .and_then(Value::as_str)
}

/// Drops one property, and its `required` entry, from a derived branch.
fn remove_property(branch: &mut Value, name: &str) {
    if let Some(properties) = branch.get_mut("properties").and_then(Value::as_object_mut) {
        properties.remove(name);
    }
    if let Some(required) = branch.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|entry| entry.as_str() != Some(name));
    }
}
