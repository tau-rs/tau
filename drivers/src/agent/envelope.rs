//! The worker envelope (ADR-0013 §4): the one JSON object a CLI session is
//! contracted to end with, its strict JSON Schema, and the tolerant parser
//! that finds it in whatever the session actually printed.
//!
//! Two shapes of the same type, on purpose:
//!
//! - [`schema`] is **strict** — every field required, no extras, `error`
//!   nullable. It is what a CLI's structured-output mode is handed
//!   (`codex --output-schema`, `claude --json-schema`), where a strict
//!   shape is what makes the mode worth using.
//! - [`parse`] is **tolerant** — fences stripped, the last balanced object
//!   taken, the three list fields defaulted. A session that ignored its
//!   contract still ran a task somebody paid for, and the reply should
//!   carry what it produced rather than throw it away.
//!
//! What tolerance is *not*: an envelope the driver wrote itself. When the
//! parse fails the driver reports [`EnvelopeViolation`] as `error.envelope`
//! and carries the transcript, because a synthesized `{"status":"failed"}`
//! would be indistinguishable from one the session wrote.
//!
//! [`parse`] is a pure function over bytes — no I/O, no clock, no
//! allocation beyond its input's size — so a fuzz target can be pointed at
//! it (ADR-0013 §4).

use schemars::{JsonSchema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The report a worker session ends with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Envelope {
    /// The session's own verdict on the task.
    pub status: Status,
    /// Up to three sentences.
    pub summary: String,
    /// What the task produced.
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    /// What the session decided alone, because it could not ask.
    #[serde(default)]
    pub assumptions: Vec<String>,
    /// Decisions worth a line.
    #[serde(default)]
    pub events: Vec<String>,
    /// Why, when `status` is `failed`.
    #[serde(default)]
    pub error: Option<String>,
}

/// A session's verdict on its own task (ADR-0013 §4).
///
/// Neither a CLI bound nor an interrupt produces one of these: the session
/// never gets a turn to write an envelope at all, and the reply's `stop`
/// says so instead (#130 runs 5 and 6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The task was done.
    #[default]
    Ok,
    /// The session stopped short on its own and says what is left.
    Partial,
    /// The task could not be done; `error` says why.
    Failed,
    /// A later instruction told the session to wind down.
    Cancelled,
}

/// One thing the session produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    /// Where it is. The contract asks for a path relative to the workspace;
    /// #130's session wrote absolute ones, so both are accepted and the
    /// driver rewrites neither.
    pub path: String,
    /// What it is.
    pub kind: Kind,
}

/// What an [`Artifact`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A file in the workspace.
    File,
    /// A patch.
    Patch,
    /// A report, meant to be read.
    Report,
}

/// Why a final message was not an envelope, even after the tolerant parse.
///
/// Reported as `error.envelope`, with the transcript intact.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{reason}")]
pub struct EnvelopeViolation {
    /// What went wrong, for a human or a model reading the reply.
    pub reason: String,
}

impl EnvelopeViolation {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// The tolerant parse of ADR-0013 §4: fence strip, then the last balanced
/// `{…}`, then serde.
///
/// Pure over bytes: invalid UTF-8 is read lossily rather than refused,
/// because a session that emitted one bad byte still wrote a report.
///
/// # Errors
///
/// [`EnvelopeViolation`] when there is no balanced object in the message, or
/// when the object is not an envelope: `status` and `summary` are required,
/// and `status` must be one of the four the contract names.
pub fn parse(bytes: &[u8]) -> Result<Envelope, EnvelopeViolation> {
    let text = String::from_utf8_lossy(bytes);
    let stripped = strip_fence(text.trim());
    let object = last_balanced_object(stripped)
        .ok_or_else(|| EnvelopeViolation::new(describe_absence(stripped)))?;
    serde_json::from_str(object)
        .map_err(|e| EnvelopeViolation::new(format!("not an envelope: {e}")))
}

/// What to say when there was no object at all to parse.
fn describe_absence(text: &str) -> String {
    if text.is_empty() {
        "the final message was empty".to_owned()
    } else {
        format!(
            "no balanced JSON object in the final message ({} bytes)",
            text.len()
        )
    }
}

/// Removes one surrounding code fence, with or without a language tag.
///
/// Only a fence that wraps the *whole* message: a fence around one block in
/// the middle of prose is left where it is, and the brace scan below finds
/// the object inside it anyway.
fn strip_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    // The tag runs to the first newline: "```json\n{…}\n```".
    let body = match rest.split_once('\n') {
        Some((tag, body)) if !tag.contains('`') => body,
        _ => rest,
    };
    body.strip_suffix("```")
        .map_or(text, |inner| inner.trim_end().trim_end_matches('\n'))
}

/// The last balanced `{…}` in `text`, scanning for braces outside string
/// literals.
///
/// "Last", not "first": a session that narrates before it reports leaves the
/// envelope at the end, and a session that quotes the schema at the top
/// leaves a decoy there.
fn last_balanced_object(text: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut start = None;
    let mut last = None;
    let mut in_string = false;
    let mut escaped = false;
    for (at, byte) in text.bytes().enumerate() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = Some(at);
                }
                depth = depth.saturating_add(1);
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(from) = start.take() {
                        last = text.get(from..=at);
                    }
                }
            }
            _ => {}
        }
    }
    last
}

/// The strict schema handed to a CLI that can enforce a final-message shape
/// (`codex --output-schema`, `claude --json-schema`), derived from the same
/// type [`parse`] deserializes with — one source of truth (ADR-0006 §5).
///
/// Strict where the parser is tolerant: every field is required, including
/// the three the parser defaults and the nullable `error`, and
/// `additionalProperties` is `false`. That is the shape structured-output
/// modes accept, and asking for all six fields is how the session is told
/// to write all six.
#[must_use]
pub fn schema() -> Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12().with(|s| {
        s.meta_schema = None;
        s.inline_subschemas = true;
    });
    let mut schema = SchemaGenerator::new(settings)
        .root_schema_for::<Envelope>()
        .to_value();
    if let Some(object) = schema.as_object_mut() {
        object.remove("title");
        object.remove("description");
        let names: Vec<Value> = object
            .get("properties")
            .and_then(Value::as_object)
            .map(|p| p.keys().map(|k| Value::String(k.clone())).collect())
            .unwrap_or_default();
        object.insert("required".to_owned(), Value::Array(names));
        object.insert("additionalProperties".to_owned(), Value::Bool(false));
    }
    super::tidy(&mut schema, true);
    schema
}
