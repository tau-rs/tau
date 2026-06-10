//! Shared `tau.toml` / `project.ts` loader used by dev / build / check / run.
//!
//! File-extension dispatch:
//! - `.ts` → TS extractor (β.8)
//! - directory → look for `tau.toml` inside
//! - everything else → treat as a `.toml` file path

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use tau_pkg::project::ProjectConfig;

/// Result of loading a project from disk.
pub struct LoadedProject {
    /// Project root (the directory containing the manifest file).
    pub project_root: PathBuf,
    /// Parsed + validated project config.
    pub project: ProjectConfig,
}

/// Load a project from a path that may be a directory, a `.ts` file,
/// or a `.toml` file.
pub fn load_project(path: &Path) -> Result<LoadedProject> {
    let ext = path.extension().and_then(|s| s.to_str());
    if path.is_file() && ext == Some("ts") {
        let src = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        let project = tau_ts_extract::extract_project(&src, path)
            .map_err(|e| anyhow!("{}", e))?;
        let project_root = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.to_path_buf());
        Ok(LoadedProject {
            project_root,
            project,
        })
    } else {
        // Default: TOML path. `path` is a directory OR a .toml file.
        let project_root = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| path.to_path_buf())
        };
        let tau_toml = project_root.join("tau.toml");
        let toml_str = std::fs::read_to_string(&tau_toml)
            .with_context(|| format!("read {}", tau_toml.display()))?;
        let project = ProjectConfig::parse_str(&toml_str)
            .map_err(|e| anyhow!("parse tau.toml: {e}"))?;
        Ok(LoadedProject {
            project_root,
            project,
        })
    }
}
