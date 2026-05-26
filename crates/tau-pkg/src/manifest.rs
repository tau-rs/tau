//! Read and structurally validate `tau.toml` manifests from disk.
//!
//! [`read_manifest`] is a thin wrapper:
//!
//! 1. `fs::read_to_string(path)` — surfaces I/O errors as
//!    [`ManifestReadError::Io`] or [`ManifestReadError::NotFound`].
//! 2. `toml::from_str::<UncheckedManifest>(&text)` — surfaces parse
//!    failures as [`ManifestReadError::Parse`].
//! 3. [`tau_domain::UncheckedManifest::validate`] — surfaces structural
//!    failures as [`ManifestReadError::Validation`] via `#[from]`.
//!
//! Used by the install lifecycle (Task 10) to read `tau.toml` from a
//! freshly-cloned package source tree.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use tau_domain::{PackageManifest, UncheckedManifest};

use crate::error::ManifestReadError;

/// Read and validate a manifest from disk.
///
/// `path` is the manifest file itself (typically
/// `<package_root>/tau.toml`), not the package root directory.
///
/// # Errors
///
/// - [`ManifestReadError::NotFound`] — file does not exist.
/// - [`ManifestReadError::Io`] — file exists but could not be read
///   (permissions, broken symlink, etc.).
/// - [`ManifestReadError::Parse`] — file is not valid TOML or doesn't
///   match the [`UncheckedManifest`] schema.
/// - [`ManifestReadError::Validation`] — TOML parsed but structural
///   validation rejected the manifest (invalid name, capability,
///   etc.).
///
/// # Example
///
/// ```
/// use tau_pkg::read_manifest;
///
/// # let tmp = tempfile::tempdir().unwrap();
/// # let path = tmp.path().join("tau.toml");
/// # std::fs::write(&path, concat!(
/// #     "name = \"acme-tool\"\n",
/// #     "version = \"1.0.0\"\n",
/// #     "description = \"A tool\"\n",
/// #     "authors = [\"Acme <acme@example.com>\"]\n",
/// #     "source = \"https://example.com/acme/tool.git\"\n",
/// #     "kind = \"tool\"\n",
/// #     "dependencies = []\n",
/// #     "capabilities = []\n",
/// # )).unwrap();
/// let manifest = read_manifest(&path).unwrap();
/// assert_eq!(manifest.name().as_str(), "acme-tool");
/// ```
pub fn read_manifest(path: &Path) -> Result<PackageManifest, ManifestReadError> {
    let text = fs::read_to_string(path).map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            ManifestReadError::NotFound {
                path: path.display().to_string(),
            }
        } else {
            ManifestReadError::Io {
                message: format!("reading manifest {}: {e}", path.display()),
            }
        }
    })?;

    let unchecked: UncheckedManifest =
        toml::from_str(&text).map_err(|e| ManifestReadError::Parse {
            reason: format!("{}: {e}", path.display()),
        })?;

    let manifest = unchecked.validate()?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    fn write_manifest(dir: &TempDir, contents: &str) -> std::path::PathBuf {
        let path = dir.path().join("tau.toml");
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// Minimal valid manifest using the natural TOML form (per ADR-0005).
    /// `PackageSource` serializes via its `Display`/`FromStr` string form,
    /// `PackageKind::Custom` as the inner `kind` string.
    fn minimal_valid_manifest() -> &'static str {
        r#"
name = "acme-tool"
version = "1.0.0"
description = "A tool for testing"
authors = ["Acme <support@acme.dev>"]
source = "https://example.com/acme/tool.git"
kind = "tool"
dependencies = []
capabilities = []
"#
    }

    #[test]
    fn read_manifest_returns_not_found_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("does-not-exist.toml");
        let err = read_manifest(&path).unwrap_err();
        assert!(matches!(err, ManifestReadError::NotFound { .. }));
    }

    #[test]
    fn read_manifest_returns_parse_for_bad_toml() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, "this is not toml = = =");
        let err = read_manifest(&path).unwrap_err();
        assert!(matches!(err, ManifestReadError::Parse { .. }));
    }

    #[test]
    fn read_manifest_returns_error_for_invalid_name() {
        let tmp = TempDir::new().unwrap();
        // PackageName rejects names with spaces/uppercase at deserialisation
        // time, so this surfaces as Parse (serde rejects the field value).
        // Either Parse or Validation is acceptable — we just want a typed
        // error, not a panic.
        let bad = r#"
name = "INVALID NAME WITH SPACES"
version = "1.0.0"
description = "bad"
authors = []
source = "https://example.com/x.git"
kind = "tool"
dependencies = []
capabilities = []
"#;
        let path = write_manifest(&tmp, bad);
        let err = read_manifest(&path).unwrap_err();
        match err {
            ManifestReadError::Parse { .. } => {}
            ManifestReadError::Validation(_) => {}
            other => panic!("expected Parse or Validation, got {other:?}"),
        }
    }

    #[test]
    fn read_manifest_succeeds_for_minimal_valid_manifest() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, minimal_valid_manifest());
        let manifest = read_manifest(&path).unwrap();
        assert_eq!(manifest.name().as_str(), "acme-tool");
        assert_eq!(manifest.version().to_string(), "1.0.0");
        assert_eq!(manifest.description(), "A tool for testing");
    }

    /// Manifest with a `[plugin]` table parses, validates, and surfaces
    /// the typed plugin manifest via `PackageManifest::plugin()`.
    #[test]
    fn read_manifest_extracts_plugin_table() {
        let tmp = TempDir::new().unwrap();
        let toml_text = r#"
name = "echo-llm"
version = "0.1.0"
description = "Toy LlmBackend plugin"
authors = ["Acme <support@acme.dev>"]
source = "https://example.com/echo-llm.git"
kind = "llm-backend"
dependencies = []
capabilities = []

[plugin]
provides = "llm_backend"
kind     = "rust-cargo"
bin      = "echo-llm"
"#;
        let path = write_manifest(&tmp, toml_text);
        let manifest = read_manifest(&path).unwrap();
        let plugin = manifest
            .plugin()
            .expect("manifest should expose [plugin] table");
        assert_eq!(plugin.provides, tau_domain::PortKind::LlmBackend);
        assert_eq!(plugin.kind, tau_domain::PluginKind::RustCargo);
        assert_eq!(plugin.bin, "echo-llm");
    }

    /// Data-only packages (no `[plugin]` table) round-trip with
    /// `plugin == None`. This is the existing behaviour, preserved
    /// for backward compatibility.
    #[test]
    fn read_manifest_without_plugin_table_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, minimal_valid_manifest());
        let manifest = read_manifest(&path).unwrap();
        assert!(
            manifest.plugin().is_none(),
            "data-only manifest should have plugin == None"
        );
    }

    /// Unknown plugin `kind` strings (e.g., `python-pip` before that
    /// variant lands) surface as a `Parse` error from the typed
    /// `PluginKind` deserializer.
    #[test]
    fn read_manifest_invalid_plugin_kind_errors() {
        let tmp = TempDir::new().unwrap();
        let bad = r#"
name = "bad-plugin"
version = "0.1.0"
description = "Plugin with unknown kind"
authors = []
source = "https://example.com/bad.git"
kind = "llm-backend"
dependencies = []
capabilities = []

[plugin]
provides = "llm_backend"
kind     = "python-pip"
bin      = "bad-plugin"
"#;
        let path = write_manifest(&tmp, bad);
        let result = read_manifest(&path);
        assert!(
            matches!(result, Err(ManifestReadError::Parse { .. })),
            "expected Parse error for unknown plugin kind, got {result:?}",
        );
    }
}
