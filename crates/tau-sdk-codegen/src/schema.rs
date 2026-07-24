//! Parse the frozen IR JSON schema into a lookup model.
//!
//! Only the pieces the SDK emitters need are lifted out: the set of `$defs`
//! (so emitters can assert a mirrored leaf type is real) and per-`$def` enum
//! variants (so vocabulary enums are sourced from the schema, not hardcoded).

use std::path::Path;

use crate::error::CodegenError;

/// Repo-relative path to the frozen schema this codegen consumes.
pub const SCHEMA_PATH: &str = "schemas/ir/tau-ir.v2.5.0.schema.json";

/// A parsed view of the frozen IR schema.
pub struct SchemaModel {
    root: serde_json::Value,
}

impl SchemaModel {
    /// Load and parse `schemas/ir/tau-ir.v2.5.0.schema.json` under `repo_root`.
    pub fn load(repo_root: &Path) -> Result<SchemaModel, CodegenError> {
        let path = repo_root.join(SCHEMA_PATH);
        let bytes = std::fs::read(&path)?;
        let root: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| CodegenError::Schema(format!("parse {}: {e}", path.display())))?;
        Ok(SchemaModel { root })
    }

    fn defs(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.root.get("$defs").and_then(|d| d.as_object())
    }

    /// True if `name` is a `$def` in the schema.
    pub fn has_def(&self, name: &str) -> bool {
        self.defs().map(|d| d.contains_key(name)).unwrap_or(false)
    }

    /// String enum variants declared on `$defs.<def>.enum`, if any.
    pub fn enum_variants(&self, def: &str) -> Option<Vec<String>> {
        let arr = self.defs()?.get(def)?.get("enum")?.as_array()?;
        Some(
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
        )
    }

    /// The schema's top-level `$id`.
    pub fn schema_id(&self) -> Option<&str> {
        self.root.get("$id").and_then(|v| v.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn repo_root() -> std::path::PathBuf {
        // crates/tau-sdk-codegen -> repo root is two levels up.
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn loads_frozen_schema_and_finds_known_defs() {
        let model = SchemaModel::load(&repo_root()).expect("load schema");
        // IrModule is the schema root; Capability is a shared leaf type the
        // authoring surface reuses verbatim.
        assert!(model.has_def("Capability"), "Capability must be a $def");
        assert!(model.schema_id().unwrap().contains("tau-ir"));
    }

    #[test]
    fn missing_def_is_reported() {
        let model = SchemaModel::load(&repo_root()).expect("load schema");
        assert!(!model.has_def("NotARealType"));
    }
}
