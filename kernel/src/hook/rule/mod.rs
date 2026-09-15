//! The Rule language: one line of policy, parsed at `attach`, evaluated at
//! a hook point (ADR-0008 §5).
//!
//! ```text
//! when <point> [ if <predicate> ] then <verdict>
//! ```
//!
//! A rule is bounded by construction, which is the only metering a text
//! program gets before a fuel tier exists: no loop, no recursion past a
//! fixed nesting limit, no state, no allocation while a predicate is
//! evaluated. Everything that could fail is refused at parse — a fact the
//! point does not carry, a `deny` at a point that admits none, a name
//! compared by order — so [`Rule::evaluate`] is total and `FailureMode` is
//! moot for it.
//!
//! What the language cannot say is refused with an error that names the
//! home ADR-0008 §5 gives it: a JSON field of the payload belongs to the
//! driver that owns the schema, a regex or anything across events to a
//! native hook, a rewrite nowhere.
//!
//! The grammar is the ADR's, exactly. An addition is an ADR amendment
//! first, and the fuzz seeds in `fuzz/seeds/rule-parse/` grow with it.
//!
//! # Canonical text
//!
//! A parsed rule keeps one text: its canonical rendering, which is what
//! [`Rule::source`] returns, what `Display` prints, and what the `Attached`
//! entry records. `parse(display(r)) == r` is the oracle the fuzz target
//! checks; whitespace, leading zeros, and a doubled `not` are normalised
//! away by it.

mod eval;
mod lex;
mod parse;

use core::fmt;
use core::str::FromStr;

use crate::abi::{DimKey, MsgKind, Name, NameError};
use crate::hook::{HookEvent, HookPoint, Verdict};

/// Parenthesis nesting a predicate may reach. Deeper is refused at parse,
/// which bounds the parser's and the evaluator's recursion by a constant.
pub const MAX_NESTING: usize = 32;

/// A parsed rule of the ADR-0008 §5 language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    point: HookPoint,
    predicate: Option<Predicate>,
    verdict: RuleVerdict,
    /// The canonical rendering: `Display` of the three fields above.
    text: String,
}

impl Rule {
    /// Parses one line of rule text.
    ///
    /// # Errors
    ///
    /// A [`RuleError`]: a syntax error with its byte offset, a fact the
    /// point does not carry, a `deny` at a point that admits none, or one
    /// of the things the language cannot say, with its home.
    pub fn parse(source: &str) -> Result<Self, RuleError> {
        let (point, predicate, verdict) = parse::rule(source)?;
        let mut rule = Self {
            point,
            predicate,
            verdict,
            text: String::new(),
        };
        rule.text = rule.to_string();
        Ok(rule)
    }

    /// The canonical text, as recorded in the log.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.text
    }

    /// The point the rule is written for. `attach` refuses a rule at any
    /// other point.
    #[must_use]
    pub fn point(&self) -> &HookPoint {
        &self.point
    }

    /// Evaluates the rule against an event. Total: a rule cannot fail.
    ///
    /// One pass over the predicate, plus one scan of the payload per
    /// `contains` or `starts_with`. An atom about a fact the event does not
    /// carry — a `remaining.<dim>` the subject holds no grant on, or an
    /// event from a point the rule was not written for — is false.
    /// `emit to parent` from the root, which has none, is an `Allow`: the
    /// note has nowhere to go, as a note to an exited agent has.
    #[must_use]
    pub fn evaluate(&self, event: &HookEvent) -> Verdict {
        eval::evaluate(self, event)
    }
}

impl FromStr for Rule {
    type Err = RuleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "when {}", self.point)?;
        if let Some(predicate) = &self.predicate {
            write!(f, " if {predicate}")?;
        }
        write!(f, " then {}", self.verdict)
    }
}

/// The `<predicate>` of a rule: a tree of `or`, `and`, `not` over atoms,
/// at most [`MAX_NESTING`] parentheses deep.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Predicate {
    Or(Box<Predicate>, Box<Predicate>),
    And(Box<Predicate>, Box<Predicate>),
    Not(Box<Predicate>),
    Atom(Atom),
}

impl Predicate {
    /// Binding strength, for printing with the fewest parentheses: an
    /// `and` inside an `or` needs none, an `or` inside an `and` does.
    const fn rank(&self) -> u8 {
        match self {
            Self::Or(..) => 0,
            Self::And(..) => 1,
            Self::Not(_) => 2,
            Self::Atom(_) => 3,
        }
    }

    fn fmt_at(&self, f: &mut fmt::Formatter<'_>, min: u8) -> fmt::Result {
        let paren = self.rank() < min;
        if paren {
            f.write_str("(")?;
        }
        match self {
            Self::Or(l, r) => {
                l.fmt_at(f, 0)?;
                f.write_str(" or ")?;
                r.fmt_at(f, 1)?;
            }
            Self::And(l, r) => {
                l.fmt_at(f, 1)?;
                f.write_str(" and ")?;
                r.fmt_at(f, 2)?;
            }
            Self::Not(p) => {
                f.write_str("not ")?;
                p.fmt_at(f, 3)?;
            }
            Self::Atom(a) => write!(f, "{a}")?,
        }
        if paren {
            f.write_str(")")?;
        }
        Ok(())
    }
}

impl fmt::Display for Predicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_at(f, 0)
    }
}

/// One test. The parser guarantees the value's type matches the fact's.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Atom {
    Cmp { fact: Fact, cmp: Cmp, value: Value },
    In { fact: Fact, values: Vec<Value> },
    Contains(String),
    StartsWith(String),
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cmp { fact, cmp, value } => write!(f, "{fact} {cmp} {value}"),
            Self::In { fact, values } => {
                write!(f, "{fact} in {{")?;
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, " {v}")?;
                }
                f.write_str(" }")
            }
            Self::Contains(s) => write!(f, "payload contains {}", Quoted(s)),
            Self::StartsWith(s) => write!(f, "payload starts_with {}", Quoted(s)),
        }
    }
}

/// A field of the [`HookEvent`], by the same name.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Fact {
    Depth,
    Driver,
    Kind,
    PayloadLen,
    Remaining(DimKey),
}

impl Fact {
    /// What the fact compares against.
    const fn ty(&self) -> Ty {
        match self {
            Self::Depth | Self::PayloadLen | Self::Remaining(_) => Ty::Int,
            Self::Driver => Ty::Driver,
            Self::Kind => Ty::Msg,
        }
    }

    /// Whether events at `point` carry this fact (ADR-0008 §2).
    const fn at(&self, point: &HookPoint) -> bool {
        matches!(
            (self, point),
            (Self::Depth, _)
                | (
                    Self::Driver | Self::PayloadLen,
                    HookPoint::PreSend | HookPoint::PreDeliver
                )
                | (Self::Kind, HookPoint::PreDeliver)
                | (
                    Self::Remaining(_),
                    HookPoint::PreSend
                        | HookPoint::PreDeliver
                        | HookPoint::OnSpawn
                        | HookPoint::OnBudget { .. },
                )
        )
    }
}

impl fmt::Display for Fact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Depth => f.write_str("depth"),
            Self::Driver => f.write_str("driver"),
            Self::Kind => f.write_str("kind"),
            Self::PayloadLen => f.write_str("payload.len"),
            Self::Remaining(dim) => write!(f, "remaining.{dim}"),
        }
    }
}

/// The type of a fact, and so of the value it may be compared against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ty {
    Int,
    Driver,
    Msg,
}

impl Ty {
    const fn describe(self) -> &'static str {
        match self {
            Self::Int => "an integer",
            Self::Driver => "a driver name, bare",
            Self::Msg => "one of request, reply, partial, notice",
        }
    }
}

/// The facts a point's events carry, for the error that names them.
const fn carries(point: &HookPoint) -> &'static str {
    match point {
        HookPoint::PreSend => "depth, driver, payload, remaining.<dim>",
        HookPoint::PreDeliver => "depth, driver, kind, payload, remaining.<dim>",
        HookPoint::OnSpawn | HookPoint::OnBudget { .. } => "depth, remaining.<dim>",
        HookPoint::OnExit => "depth",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    const fn is_order(self) -> bool {
        !matches!(self, Self::Eq | Self::Ne)
    }

    fn holds<T: Ord>(self, l: &T, r: &T) -> bool {
        match self {
            Self::Eq => l == r,
            Self::Ne => l != r,
            Self::Lt => l < r,
            Self::Le => l <= r,
            Self::Gt => l > r,
            Self::Ge => l >= r,
        }
    }
}

impl fmt::Display for Cmp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    Int(u64),
    Driver(Name),
    MsgKind(MsgKind),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::Driver(name) => write!(f, "{name}"),
            Self::MsgKind(kind) => f.write_str(kind_name(*kind)),
        }
    }
}

/// The wire name of a message kind, as the rule text spells it.
fn kind_name(kind: MsgKind) -> &'static str {
    match kind {
        MsgKind::Request => "request",
        MsgKind::Reply => "reply",
        MsgKind::Partial => "partial",
        MsgKind::Notice => "notice",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RuleVerdict {
    Allow,
    Deny(String),
    Emit { to: Target, note: String },
}

impl fmt::Display for RuleVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allow => f.write_str("allow"),
            Self::Deny(reason) => write!(f, "deny {}", Quoted(reason)),
            Self::Emit {
                to: Target::Subject,
                note,
            } => write!(f, "emit {}", Quoted(note)),
            Self::Emit {
                to: Target::Parent,
                note,
            } => write!(f, "emit to parent {}", Quoted(note)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Subject,
    Parent,
}

/// A string literal, escaped the way the lexer unescapes it.
struct Quoted<'a>(&'a str);

impl fmt::Display for Quoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;
        for c in self.0.chars() {
            match c {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                '\n' => f.write_str("\\n")?,
                '\r' => f.write_str("\\r")?,
                '\t' => f.write_str("\\t")?,
                c => write!(f, "{c}")?,
            }
        }
        f.write_str("\"")
    }
}

/// Something ADR-0008 §5 lists as not expressible, with its home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Wish {
    /// A JSON field of the payload: the kernel would be parsing payloads
    /// by proxy.
    JsonField,
    /// A regex over the payload: backtracking is not linear.
    Regex,
    /// Anything across events — counts, rates, "the third time": rules are
    /// stateless, and state that is not in the log is not replayable.
    AcrossEvents,
    /// Anything about another agent: the event is about one subject.
    AnotherAgent,
    /// Arithmetic: not a predicate.
    Arithmetic,
    /// Time: not a predicate.
    Time,
    /// A string transform: not a predicate.
    StringTransform,
    /// Rewriting the payload: not a verdict.
    Rewrite,
}

impl Wish {
    /// What was asked for.
    #[must_use]
    pub const fn what(self) -> &'static str {
        match self {
            Self::JsonField => "a JSON field of the payload",
            Self::Regex => "a regex over the payload",
            Self::AcrossEvents => "anything across events",
            Self::AnotherAgent => "anything about another agent",
            Self::Arithmetic => "arithmetic",
            Self::Time => "time",
            Self::StringTransform => "a string transform",
            Self::Rewrite => "rewriting the payload",
        }
    }

    /// Where it goes instead (ADR-0008 §5).
    #[must_use]
    pub const fn home(self) -> &'static str {
        match self {
            Self::JsonField => "the driver that owns that schema",
            Self::Regex | Self::Arithmetic | Self::Time | Self::StringTransform => "a native hook",
            Self::AcrossEvents => "a native hook, which may hold state",
            Self::AnotherAgent => "a native hook, or a driver",
            Self::Rewrite => "nowhere: deny with a reason, and the model corrects itself",
        }
    }
}

/// Why rule text was refused. `at` is a byte offset into the text.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RuleError {
    /// Nothing but whitespace.
    #[error("empty rule; a rule is `when <point> [if <predicate>] then <verdict>`")]
    Empty,
    /// A character no token starts with.
    #[error("unexpected {found:?} at {at}")]
    UnexpectedChar {
        /// Where.
        at: usize,
        /// What.
        found: char,
    },
    /// A string literal with no closing quote.
    #[error("unterminated string starting at {at}")]
    UnterminatedString {
        /// Where the string opened.
        at: usize,
    },
    /// A `\` followed by something that is not an escape.
    #[error("bad escape `\\{found}` at {at}; the escapes are \\\" \\\\ \\n \\r \\t")]
    BadEscape {
        /// Where.
        at: usize,
        /// The character after the backslash.
        found: char,
    },
    /// An integer past `u64::MAX`.
    #[error("integer at {at} does not fit in 64 bits")]
    IntTooLarge {
        /// Where.
        at: usize,
    },
    /// The wrong token here.
    #[error("expected {expected}, found {found} at {at}")]
    Unexpected {
        /// Where.
        at: usize,
        /// The token, or "end of rule".
        found: String,
        /// What the grammar wanted.
        expected: &'static str,
    },
    /// Not one of the five points.
    #[error("unknown point `{found}` at {at}; the points are pre_send, pre_deliver, on_spawn, on_exit, on_budget(<dim>, <int>)")]
    UnknownPoint {
        /// Where.
        at: usize,
        /// What.
        found: String,
    },
    /// Not one of the facts.
    #[error("unknown fact `{found}` at {at}; the facts are depth, driver, kind, payload.len, remaining.<dim>, and payload contains / starts_with")]
    UnknownFact {
        /// Where.
        at: usize,
        /// What.
        found: String,
    },
    /// Not one of the verdicts.
    #[error("unknown verdict `{found}` at {at}; the verdicts are allow, deny <string>, emit [to subject | to parent] <string>")]
    UnknownVerdict {
        /// Where.
        at: usize,
        /// What.
        found: String,
    },
    /// A name — a driver, a custom dimension — the ABI grammar refuses.
    #[error("bad name at {at}: {error}")]
    BadName {
        /// Where.
        at: usize,
        /// Why.
        error: NameError,
    },
    /// The rule names a fact its point's events do not carry, so it could
    /// only fail at runtime (ADR-0008 §5).
    #[error("`{fact}` is not a fact at {point}, which carries {carries}")]
    FactNotAtPoint {
        /// The fact.
        fact: String,
        /// The point.
        point: HookPoint,
        /// What the point does carry.
        carries: &'static str,
    },
    /// A value of the wrong type for the fact.
    #[error("`{fact}` compares against {expected}, at {at}")]
    TypeMismatch {
        /// Where the value is.
        at: usize,
        /// The fact.
        fact: String,
        /// What it takes.
        expected: &'static str,
    },
    /// An ordering comparison on a fact that has no order.
    #[error("`{fact}` admits == and != only, at {at}")]
    Unordered {
        /// Where the operator is.
        at: usize,
        /// The fact.
        fact: String,
    },
    /// A `deny` at an on point, where nothing can be stopped (ADR-0008 §1).
    #[error("{point} does not admit deny; it has already happened (ADR-0008 §1)")]
    DenyNotAdmitted {
        /// The point.
        point: HookPoint,
    },
    /// Parentheses nested past [`MAX_NESTING`].
    #[error("predicate nests deeper than {limit} parentheses")]
    TooDeep {
        /// The limit.
        limit: usize,
    },
    /// Something the language does not say, on purpose.
    #[error("a rule cannot say {}; its home is {} (ADR-0008 §5), at {at}", wish.what(), wish.home())]
    NotExpressible {
        /// Where.
        at: usize,
        /// What, and where it goes.
        wish: Wish,
    },
}
