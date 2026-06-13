//! Pipeline input templating: `${input}` and `${steps.<id>.output}`.
//!
//! Two surfaces share one parser:
//! - [`extract_refs`] — static reference extraction for build-time checks
//!   (no values needed).
//! - [`resolve`] — runtime substitution against an input + prior outputs.
//!
//! Escape: `$${` yields a literal `${`.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use thiserror::Error;

/// A `${...}` reference found in a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateRef {
    /// `${input}` — the run's top-level input.
    Input,
    /// `${steps.<id>.output}` — an earlier step's output, by id.
    StepOutput(String),
}

/// Template parse/resolve error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TemplateError {
    /// A `${` was never closed by `}`.
    #[error("unterminated reference: ${{{0}")]
    Unterminated(String),
    /// A reference did not match `input` or `steps.<id>.output`.
    #[error("unrecognized reference: {0}")]
    Unrecognized(String),
    /// A `${steps.<id>.output}` named an id with no available output.
    #[error("unresolved reference: {0}")]
    Unresolved(String),
}

/// Parse a `key` (the text between `${` and `}`) into a [`TemplateRef`].
fn parse_key(key: &str) -> Result<TemplateRef, TemplateError> {
    if key == "input" {
        return Ok(TemplateRef::Input);
    }
    if let Some(stripped) = key.strip_prefix("steps.") {
        if let Some(id) = stripped.strip_suffix(".output") {
            return Ok(TemplateRef::StepOutput(id.to_string()));
        }
    }
    Err(TemplateError::Unrecognized(key.to_string()))
}

/// Walk `template`, invoking `on_ref` for each recognized reference and
/// pushing literal text (with `$${`->`${` unescaping) to `out`.
fn walk(
    template: &str,
    out: &mut String,
    mut on_ref: impl FnMut(TemplateRef) -> Result<String, TemplateError>,
) -> Result<(), TemplateError> {
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            if chars.peek() == Some(&'$') {
                chars.next();
                if chars.peek() == Some(&'{') {
                    chars.next();
                    out.push_str("${");
                } else {
                    out.push_str("$$");
                }
                continue;
            }
            if chars.peek() == Some(&'{') {
                chars.next();
                let mut key = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '}' {
                        closed = true;
                        break;
                    }
                    key.push(ch);
                }
                if !closed {
                    return Err(TemplateError::Unterminated(key));
                }
                let r = parse_key(&key)?;
                out.push_str(&on_ref(r)?);
                continue;
            }
        }
        out.push(c);
    }
    Ok(())
}

/// Extract every `${...}` reference from `template`, in order. Used by the
/// lowering pass for forward/unknown-reference checks (no values needed).
pub fn extract_refs(template: &str) -> Result<Vec<TemplateRef>, TemplateError> {
    let mut refs = Vec::new();
    let mut sink = String::new();
    walk(template, &mut sink, |r| {
        refs.push(r.clone());
        Ok(String::new())
    })?;
    Ok(refs)
}

/// Resolve `${...}` references in `template` against `input` + `prior`
/// (step id -> stringified output).
pub fn resolve(
    template: &str,
    input: &str,
    prior: &BTreeMap<String, String>,
) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(template.len());
    walk(template, &mut out, |r| match r {
        TemplateRef::Input => Ok(input.to_string()),
        TemplateRef::StepOutput(id) => prior.get(&id).cloned().ok_or(TemplateError::Unresolved(id)),
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_input_and_step_output() {
        let mut prior = BTreeMap::new();
        prior.insert("gather".to_string(), "notes".to_string());
        let out = resolve("in=${input} g=${steps.gather.output}", "X", &prior).unwrap();
        assert_eq!(out, "in=X g=notes");
    }

    #[test]
    fn escapes_double_dollar() {
        let out = resolve("$${input}", "X", &BTreeMap::new()).unwrap();
        assert_eq!(out, "${input}");
    }

    #[test]
    fn unterminated_errors() {
        assert!(matches!(
            resolve("${input", "X", &BTreeMap::new()),
            Err(TemplateError::Unterminated(_))
        ));
    }

    #[test]
    fn unresolved_step_errors() {
        assert!(matches!(
            resolve("${steps.nope.output}", "X", &BTreeMap::new()),
            Err(TemplateError::Unresolved(ref s)) if s == "nope"
        ));
    }

    #[test]
    fn extract_refs_lists_in_order() {
        let refs = extract_refs("${input} ${steps.a.output} ${steps.b.output}").unwrap();
        assert_eq!(
            refs,
            alloc::vec![
                TemplateRef::Input,
                TemplateRef::StepOutput("a".into()),
                TemplateRef::StepOutput("b".into()),
            ]
        );
    }
}
