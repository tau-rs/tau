//! Run-scoped store of pipeline step outputs, keyed by pipeline-step id.
//!
//! Makes `${steps.<id>.output}` addressable — the substrate the
//! single-agent interpreter lacks. Stores each step's output as a JSON
//! `Value`; `template_map` projects it to the `String` map the templater
//! consumes (string values pass through; other values are compact-JSON
//! encoded).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};

use serde_json::Value;

/// Pipeline step outputs accumulated during a run.
#[derive(Debug, Default, Clone)]
pub struct OutputStore {
    map: BTreeMap<String, Value>,
}

impl OutputStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `id`'s output.
    pub fn insert(&mut self, id: impl Into<String>, value: Value) {
        self.map.insert(id.into(), value);
    }

    /// Look up a step's output value.
    pub fn get(&self, id: &str) -> Option<&Value> {
        self.map.get(id)
    }

    /// Project to the `id -> String` map the templater consumes. A
    /// `Value::String` yields its inner text; any other value is
    /// compact-JSON encoded.
    pub fn template_map(&self) -> BTreeMap<String, String> {
        self.map
            .iter()
            .map(|(k, v)| {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), s)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_map_unwraps_strings_and_encodes_others() {
        let mut s = OutputStore::new();
        s.insert("a", Value::String("hi".into()));
        s.insert("b", serde_json::json!({"n": 1}));
        let m = s.template_map();
        assert_eq!(m.get("a").unwrap(), "hi");
        assert_eq!(m.get("b").unwrap(), "{\"n\":1}");
    }
}
