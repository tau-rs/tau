//! Orchestrates rendering + writing the generated SDK packages.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::emit_python;
use crate::emit_ts;
use crate::error::CodegenError;
use crate::schema::SchemaModel;

/// Render every generated file, keyed by repo-relative path.
pub fn render_all(repo_root: &Path) -> Result<BTreeMap<PathBuf, String>, CodegenError> {
    let schema = SchemaModel::load(repo_root)?;
    let mut all = BTreeMap::new();
    for (rel, contents) in emit_python::render_package(&schema) {
        all.insert(PathBuf::from("sdk/python").join(rel), contents);
    }
    for (rel, contents) in emit_ts::render_package(&schema) {
        all.insert(PathBuf::from("sdk/ts").join(rel), contents);
    }
    Ok(all)
}

/// Generate all SDK packages under `repo_root`.
pub fn generate(repo_root: &Path) -> Result<(), CodegenError> {
    for (rel, contents) in render_all(repo_root)? {
        let path = repo_root.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
    }
    Ok(())
}
