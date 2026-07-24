//! Orchestrates writing the generated SDK packages under the repo root.

use std::path::Path;

use crate::emit_python;
use crate::error::CodegenError;
use crate::schema::SchemaModel;

/// Generate all SDK packages under `repo_root` (writes `sdk/python`; `sdk/ts`
/// is added by the TS emitter task).
pub fn generate(repo_root: &Path) -> Result<(), CodegenError> {
    let schema = SchemaModel::load(repo_root)?;

    let py = emit_python::render_package(&schema);
    write_tree(&repo_root.join("sdk/python"), py)?;

    Ok(())
}

fn write_tree(
    base: &Path,
    files: std::collections::BTreeMap<std::path::PathBuf, String>,
) -> Result<(), CodegenError> {
    for (rel, contents) in files {
        let path = base.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
    }
    Ok(())
}
