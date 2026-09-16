//! The calculator every tool cassette was recorded against: the tool
//! definition the provider saw, and the answer the recorder gave back.
//! The tool loop's calculator driver and the scenario tables' second turn
//! both answer from here, so a replayed second request cannot disagree
//! with the recording on the `tool_result` text.

use serde_json::{json, Value};
use tau_kernel::abi::Name;
use tau_kernel::bridge::ToolDef;
use tau_kernel::driver::ToolSchema;

pub(crate) const NAME: &str = "calculator";
pub(crate) const DESCRIPTION: &str = "Evaluates an arithmetic expression.";

pub(crate) fn input_schema() -> Value {
    json!({"type":"object","properties":{"expression":{"type":"string"}},"required":["expression"],"additionalProperties":false})
}

/// The bridge definition, as the scenario tables put it on a request.
pub(crate) fn tool_def() -> ToolDef {
    ToolDef {
        name: Name::new(NAME).unwrap(),
        description: DESCRIPTION.into(),
        input_schema: input_schema(),
    }
}

/// The driver description, as a registered calculator driver answers
/// `describe`; `Toolbox::project` turns it back into [`tool_def`].
pub(crate) fn tool_schema() -> ToolSchema {
    ToolSchema {
        description: DESCRIPTION.into(),
        input_schema: serde_json::to_vec(&input_schema()).unwrap(),
    }
}

/// Sums of products over non-negative integers: `17*23`, `2+2`,
/// `2+3+5+7+11+13+17+19+23+29+31+37`. Everything a recorded model asked.
pub(crate) fn eval(expression: &str) -> Option<u64> {
    let mut sum: u64 = 0;
    for term in expression.split('+') {
        let mut product: u64 = 1;
        for factor in term.split('*') {
            product = product.checked_mul(factor.trim().parse().ok()?)?;
        }
        sum = sum.checked_add(product)?;
    }
    Some(sum)
}

/// The `tool_result` text for a `tool_call` input.
pub(crate) fn answer(input: &Value) -> String {
    let expression = input
        .get("expression")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("calculator input has no expression: {input}"));
    eval(expression)
        .unwrap_or_else(|| panic!("calculator cannot evaluate {expression:?}"))
        .to_string()
}
