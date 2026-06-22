//! Resolved model reference: the concrete `{ backend, model_id }` an alias
//! lowered to. The IR never carries the source-level alias (D2).

// schemars 0.8 derive generates code using bare `Box`/`String`/`vec!`
// from the std prelude — import it when the feature is active.
#[cfg(feature = "schema")]
#[allow(unused_imports)]
use std::prelude::rust_2021::*;

use alloc::string::String;
use serde::{Deserialize, Serialize};

/// A concrete, build-time-resolved model selection.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ModelRef {
    /// Backend package name — the key the runtime resolves a backend by.
    pub backend: String,
    /// Vendor model id placed into `CompletionRequest.model`.
    pub model_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_json() {
        let m = ModelRef {
            backend: "anthropic".into(),
            model_id: "claude-haiku-4-5".into(),
        };
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<ModelRef>(&s).unwrap(), m);
    }
}
