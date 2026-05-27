//! Workflow step input templating.
//!
//! Recognizes two reference forms:
//! - `${input}` → the workflow's user-supplied input string.
//! - `${steps.<id>.output}` → the prior step's output, by id.
//!
//! Both are resolved at step-dispatch time, after that step's preceding
//! steps have completed. Forward references (a step referencing a later
//! step) are detected at workflow-parse time in the runner before any
//! step runs — see runner.rs.

use std::collections::BTreeMap;

use crate::error::WorkflowError;

/// Resolve `${...}` references in `template` against `input` + prior step
/// outputs. Unknown references produce `WorkflowError::TemplateUnresolved`
/// with `workflow` and `step_id` populated by the caller (we don't know
/// them here).
///
/// Escape sequence: `$${` resolves to a literal `${`.
///
/// # Examples
///
/// Substitute `${input}`:
///
/// ```
/// use tau_workflow::resolve_template;
/// use std::collections::BTreeMap;
///
/// let result = resolve_template(
///     "Hello, ${input}!",
///     "world",
///     &BTreeMap::new(),
///     "my-workflow",
///     "greeting-step",
/// ).expect("template should resolve");
///
/// assert_eq!(result, "Hello, world!");
/// ```
///
/// Chain prior step outputs using `${steps.<id>.output}`:
///
/// ```
/// use tau_workflow::resolve_template;
/// use std::collections::BTreeMap;
///
/// let mut prior = BTreeMap::new();
/// prior.insert("gather".to_string(), "market analysis".to_string());
///
/// let result = resolve_template(
///     "Summarise: ${steps.gather.output}",
///     "",
///     &prior,
///     "pipeline",
///     "summarise-step",
/// ).expect("template should resolve");
///
/// assert_eq!(result, "Summarise: market analysis");
/// ```
///
/// An unknown reference returns `WorkflowError::TemplateUnresolved`:
///
/// ```
/// use tau_workflow::{resolve_template, WorkflowError};
/// use std::collections::BTreeMap;
///
/// let err = resolve_template(
///     "${steps.missing.output}",
///     "x",
///     &BTreeMap::new(),
///     "pipeline",
///     "step-b",
/// ).expect_err("unknown reference should fail");
///
/// assert!(matches!(err, WorkflowError::TemplateUnresolved { .. }));
/// ```
pub fn resolve(
    template: &str,
    input: &str,
    prior_outputs: &BTreeMap<String, String>,
    workflow: &str,
    step_id: &str,
) -> Result<String, WorkflowError> {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.char_indices().peekable();

    while let Some((_, c)) = chars.next() {
        if c == '$' {
            // Lookahead for `$${` (escape) or `${...}` (reference).
            if let Some(&(_, '$')) = chars.peek() {
                chars.next(); // consume the second $
                if let Some(&(_, '{')) = chars.peek() {
                    chars.next(); // consume {
                    out.push_str("${");
                    continue;
                } else {
                    out.push_str("$$");
                    continue;
                }
            }
            if let Some(&(_, '{')) = chars.peek() {
                chars.next(); // consume {
                let mut key = String::new();
                let mut closed = false;
                for (_, ch) in chars.by_ref() {
                    if ch == '}' {
                        closed = true;
                        break;
                    }
                    key.push(ch);
                }
                if !closed {
                    return Err(WorkflowError::TemplateUnresolved {
                        workflow: workflow.into(),
                        step_id: step_id.into(),
                        missing: format!("unterminated ${{{key}"),
                    });
                }
                let value = resolve_key(&key, input, prior_outputs).ok_or_else(|| {
                    WorkflowError::TemplateUnresolved {
                        workflow: workflow.into(),
                        step_id: step_id.into(),
                        missing: key.clone(),
                    }
                })?;
                out.push_str(value);
                continue;
            }
        }
        out.push(c);
    }
    Ok(out)
}

fn resolve_key<'a>(
    key: &str,
    input: &'a str,
    prior_outputs: &'a BTreeMap<String, String>,
) -> Option<&'a str> {
    if key == "input" {
        return Some(input);
    }
    // steps.<id>.output
    let stripped = key.strip_prefix("steps.")?;
    let id = stripped.strip_suffix(".output")?;
    prior_outputs.get(id).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_outputs() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn one_output(id: &str, val: &str) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert(id.to_string(), val.to_string());
        m
    }

    #[test]
    fn resolves_input_reference() {
        let out = resolve("hello ${input}!", "world", &empty_outputs(), "wf", "s").unwrap();
        assert_eq!(out, "hello world!");
    }

    #[test]
    fn resolves_step_output_reference() {
        let outputs = one_output("a", "alpha");
        let out = resolve("got ${steps.a.output}", "in", &outputs, "wf", "s").unwrap();
        assert_eq!(out, "got alpha");
    }

    #[test]
    fn unresolved_step_reference_errors() {
        let err = resolve("${steps.nope.output}", "x", &empty_outputs(), "wf", "s").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("nope"), "got {msg}");
    }

    #[test]
    fn passes_through_plain_text() {
        let out = resolve("no templates here", "x", &empty_outputs(), "wf", "s").unwrap();
        assert_eq!(out, "no templates here");
    }

    #[test]
    fn escapes_double_dollar() {
        let out = resolve("price: $${input}", "10", &empty_outputs(), "wf", "s").unwrap();
        assert_eq!(out, "price: ${input}");
    }

    #[test]
    fn unterminated_reference_errors() {
        let err = resolve("${input", "x", &empty_outputs(), "wf", "s").unwrap_err();
        assert!(format!("{err}").contains("unterminated"), "got {err}");
    }
}
