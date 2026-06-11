//! Guided multi-option error renderer for sandbox resolution failures,
//! Layer 2 install-time cross-check errors, and Skills-2 install errors.
//!
//! Spec §6 of 2026-05-04-sandbox-activation-design.md.

use std::fmt::Write as _;

use tau_pkg::sandbox_check::CrossCheckError;
use tau_pkg::InstallError;
use tau_runtime_tokio::process_gate::{ResolutionError, ResolutionRejection};

/// Render a [`ResolutionError`] as a guided multi-option error message.
///
/// Output is plain text (no color); each line is one logical step. The
/// caller (typically tau-cli) prints this to stderr and exits 2.
pub fn render_resolution_error(err: &ResolutionError) -> String {
    match err {
        ResolutionError::NoAdapterMatches {
            tried,
            platform,
            required_tier,
        } => render_no_adapter_matches(tried, platform, *required_tier),
        ResolutionError::PluginTierMismatch {
            plugin,
            required,
            delivered,
        } => render_plugin_tier_mismatch(plugin, *required, *delivered),
        ResolutionError::ConfigError { message } => {
            format!("sandbox configuration error: {message}")
        }
        // Catch-all for #[non_exhaustive] forward-compat.
        other => format!("sandbox resolution error: {other}"),
    }
}

fn render_no_adapter_matches(
    tried: &[(String, ResolutionRejection)],
    platform: &str,
    required_tier: tau_ports::CapabilityTier,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "no sandbox adapter satisfies project requirements");
    let _ = writeln!(out);
    let _ = writeln!(out, "  required: tier={required_tier:?}");
    let _ = writeln!(out, "  detected platform: {platform}");
    let _ = writeln!(out);
    let _ = writeln!(out, "  adapter status on this machine:");
    for (name, rejection) in tried {
        let pad = if name.len() < 12 { 12 - name.len() } else { 1 };
        let padding = " ".repeat(pad);
        let line = match rejection {
            ResolutionRejection::PlatformMismatch => "not applicable on this platform".to_string(),
            ResolutionRejection::ProbeUnavailable(reason) => {
                format!("unavailable: {reason}")
            }
            ResolutionRejection::TierTooLow {
                delivered,
                required,
            } => {
                format!("available, but tier={delivered:?} below required={required:?}")
            }
            ResolutionRejection::ShapesUnsupported { missing } => {
                format!("missing shapes: {missing:?}")
            }
            ResolutionRejection::PluginTierTooLow { plugin, required } => {
                format!("plugin '{plugin}' requires tier {required:?}; this adapter cannot deliver")
            }
            other => format!("rejected: {other}"),
        };
        let _ = writeln!(out, "    {name}:{padding}{line}");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "  options to proceed:");
    let _ = writeln!(
        out,
        "    - install a container runtime (Docker Desktop or Podman)"
    );
    let _ = writeln!(
        out,
        "    - reduce required_tier in <scope>/config.toml to \"light\" or \"none\""
    );
    let _ = writeln!(out, "    - run with --no-sandbox (this invocation only)");
    out
}

/// Render a plugin-tier mismatch as a guided error.
pub fn render_plugin_tier_mismatch(
    plugin: &str,
    required: tau_domain::PluginRequiredTier,
    delivered: tau_ports::CapabilityTier,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "plugin '{plugin}' requires tier {required:?}; selected adapter delivers {delivered:?}"
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "  options to proceed:");
    let _ = writeln!(
        out,
        "    - upgrade required_tier in <scope>/config.toml to match the plugin's floor"
    );
    let _ = writeln!(
        out,
        "    - remove the plugin from agents that don't actually use it"
    );
    let _ = writeln!(
        out,
        "    - the plugin author can reduce required_tier (NOT recommended for security-sensitive plugins)"
    );
    out
}

/// Render a `CrossCheckError` from `tau install`'s Layer 2 step 8.7
/// into multi-line guided diagnostic output.
///
/// The output format mirrors `render_resolution_error`: a leading "✗"
/// marker, the discrepancy laid out, and a numbered "Resolution"
/// section telling the user how to recover.
///
/// # Sub-project B Task 10
pub fn render_cross_check_error(err: &CrossCheckError) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "✗ install aborted: plugin capability cross-check failed"
    );
    let _ = writeln!(out);

    match err {
        CrossCheckError::SpawnFailed(msg) => {
            let _ = writeln!(out, "  Could not spawn plugin binary: {msg}");
            let _ = writeln!(out);
            let _ = writeln!(out, "  Resolution:");
            let _ = writeln!(
                out,
                "    1. Verify the binary builds standalone (cargo build)."
            );
            let _ = writeln!(
                out,
                "    2. Verify the binary runs (./target/debug/<plugin>)."
            );
            let _ = writeln!(
                out,
                "    3. Once it runs, retry: tau install --force <plugin>"
            );
        }
        CrossCheckError::HandshakeFailed(msg) => {
            let _ = writeln!(out, "  Plugin handshake failed: {msg}");
            let _ = writeln!(out);
            let _ = writeln!(out, "  Resolution:");
            let _ = writeln!(
                out,
                "    1. Check that the plugin's binary speaks the expected protocol."
            );
            let _ = writeln!(
                out,
                "    2. Inspect plugin stderr (run the binary directly)."
            );
            let _ = writeln!(
                out,
                "    3. After fixing, retry: tau install --force <plugin>"
            );
        }
        CrossCheckError::BinaryClaimsExtra { plugin, claimed } => {
            let _ = writeln!(
                out,
                "  Plugin '{plugin}' calls tool.describe_capabilities and asks for:"
            );
            let _ = writeln!(out, "    - {claimed:?}");
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "  But the manifest's [[capabilities]] block does not include this capability."
            );
            let _ = writeln!(out);
            let _ = writeln!(out, "  Resolution:");
            let _ = writeln!(
                out,
                "    1. Add the missing capability to the plugin manifest's [[capabilities]] block."
            );
            let _ = writeln!(
                out,
                "    2. Or remove the capability from the binary's tool.describe_capabilities surface."
            );
            let _ = writeln!(out, "    3. Then retry: tau install --force <plugin>");
        }
        CrossCheckError::ManifestDeclaresUnused { plugin, declared } => {
            let _ = writeln!(out, "  Manifest of '{plugin}' declares this capability:");
            let _ = writeln!(out, "    - {declared:?}");
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "  But the binary's tool.describe_capabilities surface does not request it."
            );
            let _ = writeln!(out);
            let _ = writeln!(out, "  Resolution:");
            let _ = writeln!(
                out,
                "    1. Remove the unused capability from the plugin manifest."
            );
            let _ = writeln!(
                out,
                "    2. Or extend the binary to actually use this capability via tool.describe_capabilities."
            );
            let _ = writeln!(out, "    3. Then retry: tau install --force <plugin>");
        }
        // CrossCheckError is #[non_exhaustive]; future variants render as
        // a generic line.
        _ => {
            let _ = writeln!(out, "  {err}");
            let _ = writeln!(out);
            let _ = writeln!(out, "  Resolution:");
            let _ = writeln!(out, "    1. Inspect the plugin manifest and binary.");
            let _ = writeln!(out, "    2. Then retry: tau install --force <plugin>");
        }
    }

    out
}

/// Render an [`InstallError`] as a guided, human-readable error message.
///
/// Covers the four Skills-2 variants introduced in T2. Other variants
/// fall back to the `Display` impl.
///
/// # Skills-2 Task 8
pub fn render_install_error(err: &InstallError) -> String {
    match err {
        InstallError::SkillContentMissing {
            name,
            expected_path,
        } => {
            format!(
                "error: skill {name:?} failed install validation\n\n  \
                 SKILL.md not found at:\n    {}\n\n  \
                 The package declares kind = \"skill\" but no SKILL.md \
                 was found at the path specified by [skill] content.\n  \
                 Verify the package source contains SKILL.md, or update \
                 the [skill] content field in tau.toml.\n",
                expected_path.display()
            )
        }
        InstallError::SkillNameMismatch { tau_toml, skill_md } => {
            format!(
                "error: skill name mismatch\n\n  \
                 tau.toml declares name = {tau_toml:?}\n  \
                 SKILL.md frontmatter declares name = {skill_md:?}\n\n  \
                 Both must match. Fix the name field in one of:\n    \
                 tau.toml (top-level `name`)\n    \
                 SKILL.md (YAML frontmatter `name`)\n"
            )
        }
        InstallError::SkillFrontmatterInvalid { detail } => {
            format!(
                "error: skill SKILL.md frontmatter is invalid\n\n  \
                 {detail}\n\n  \
                 SKILL.md must begin with a YAML frontmatter block:\n    \
                 ---\n    name: <skill-name>\n    description: <short description>\n    \
                 ---\n    <markdown body>\n"
            )
        }
        InstallError::SkillReferenceWithoutCapability {
            reference,
            declared_paths,
        } => {
            let declared_block = if declared_paths.is_empty() {
                "    (no fs.read capabilities declared)".to_string()
            } else {
                declared_paths
                    .iter()
                    .map(|p| format!("    {p}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!(
                "error: SKILL.md references a file the skill cannot read\n\n  \
                 Reference: {reference}\n\n  \
                 Declared fs.read paths:\n{declared_block}\n\n  \
                 Add an `fs.read` capability whose paths glob covers the \
                 reference, or remove the reference from SKILL.md.\n"
            )
        }
        // Catch-all for other InstallError variants.
        other => format!("install error: {other}\n"),
    }
}

/// Render a [`tau_pkg::bundle::VerifyError`] as a guided, structured
/// error message — the shared path `tau run --bundle` uses instead of a
/// bare `eprintln!("error: {e}")` (D9).
///
/// The `VerifyError` Display strings already carry remediation hints, so
/// the renderer frames them with the standard "✗" marker used by the
/// other `render_*` helpers and lets `run_main` map the exit code.
pub fn render_verify_error(err: &tau_pkg::bundle::VerifyError) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "✗ bundle verification failed");
    let _ = writeln!(out);
    let _ = writeln!(out, "  {err}");
    out
}

/// Render an `ImportError` as a guided, human-readable error message.
///
/// Covers Skills-5 `tau skill import` error variants. Each variant
/// surfaces a remediation hint so users can fix the issue immediately.
///
/// # Skills-5 Task 5
pub fn render_import_error(err: &crate::cmd::skill::import::ImportError) -> String {
    use crate::cmd::skill::import::ImportError;
    match err {
        ImportError::SourceAlreadyTauSkill { path } => {
            format!(
                "error: source already contains tau.toml\n\n  \
                 Path: {}\n\n  \
                 This is already a tau-native skill package. Use:\n    \
                 tau install {}\n  \
                 instead of `tau skill import`.\n",
                path.display(),
                path.display()
            )
        }
        ImportError::NotASkillPackage { path } => {
            format!(
                "error: not a skill package\n\n  \
                 Path: {}\n\n  \
                 The directory has neither tau.toml nor SKILL.md.\n  \
                 A valid Anthropic-format skill must contain a SKILL.md\n  \
                 file with YAML frontmatter (name + description fields).\n",
                path.display()
            )
        }
        ImportError::OutputDirectoryExists { path } => {
            format!(
                "error: output directory already exists\n\n  \
                 Path: {}\n\n  \
                 Pass --force to overwrite:\n    \
                 tau skill import <source> --output {} --force\n",
                path.display(),
                path.display()
            )
        }
        ImportError::CloneFailed { detail } => {
            format!(
                "error: git clone failed\n\n  \
                 {detail}\n\n  \
                 Verify the source URL is correct and the repository is accessible.\n"
            )
        }
        ImportError::Synthesize(e) => {
            format!(
                "error: manifest synthesis failed\n\n  \
                 {e}\n\n  \
                 Check that SKILL.md has valid YAML frontmatter with\n  \
                 'name' and 'description' fields, and that the name contains\n  \
                 only alphanumeric characters, hyphens, and underscores.\n"
            )
        }
        other => format!("import error: {other}\n"),
    }
}

/// Render an `ExportError` as a guided, human-readable error message.
///
/// Skills-5 Task 6 stub — export is not yet implemented; this render
/// covers the type variants declared in the stub module.
///
/// # Skills-5 Task 5
pub fn render_export_error(err: &crate::cmd::skill::export::ExportError) -> String {
    use crate::cmd::skill::export::ExportError;
    match err {
        ExportError::SkillNotInstalled { name, suggestion } => {
            let hint = match suggestion {
                Some(s) => format!("\n\n  Did you mean: {s}?"),
                None => String::new(),
            };
            format!(
                "error: skill not installed: {name:?}{hint}\n\n  \
                 Run `tau skill list` to see installed skills.\n"
            )
        }
        ExportError::WouldDropMetadata { name, dropped } => {
            let items = dropped
                .iter()
                .map(|d| format!("    - {d}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "error: export would drop tau-specific metadata (skill {name:?})\n\n\
                 {items}\n\n  \
                 Remove --strict to proceed with a warning, or remove the\n  \
                 tau-specific fields from the skill manifest first.\n"
            )
        }
        ExportError::OutputDirectoryExists { path } => {
            format!(
                "error: output directory already exists\n\n  \
                 Path: {}\n\n  \
                 Pass --force to overwrite:\n    \
                 tau skill export <name> --output {} --force\n",
                path.display(),
                path.display()
            )
        }
        other => format!("export error: {other}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_adapter_matches_renders_all_sections() {
        let err = ResolutionError::NoAdapterMatches {
            tried: vec![
                ("native".into(), ResolutionRejection::PlatformMismatch),
                (
                    "container".into(),
                    ResolutionRejection::ProbeUnavailable(
                        "neither docker nor podman on PATH".into(),
                    ),
                ),
                (
                    "passthrough".into(),
                    ResolutionRejection::TierTooLow {
                        delivered: tau_ports::CapabilityTier::None,
                        required: tau_ports::CapabilityTier::Strict,
                    },
                ),
            ],
            platform: "macos".into(),
            required_tier: tau_ports::CapabilityTier::Strict,
        };
        let rendered = render_resolution_error(&err);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn plugin_tier_mismatch_renders_with_options() {
        let err = ResolutionError::PluginTierMismatch {
            plugin: "credentials-store".into(),
            required: tau_domain::PluginRequiredTier::Strict,
            delivered: tau_ports::CapabilityTier::None,
        };
        let rendered = render_resolution_error(&err);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn config_error_renders_message() {
        let err = ResolutionError::ConfigError {
            message: "unknown adapter kind: weird".into(),
        };
        let rendered = render_resolution_error(&err);
        assert!(rendered.contains("unknown adapter kind: weird"));
        assert!(rendered.contains("configuration error"));
    }

    #[test]
    fn verify_error_renders_structured_block() {
        use tau_pkg::bundle::VerifyError;
        let err = VerifyError::SelfHashMismatch {
            claimed: "aaaa".into(),
            computed: "bbbb".into(),
        };
        let rendered = render_verify_error(&err);
        assert!(
            rendered.contains("bundle verification failed"),
            "got: {rendered}"
        );
        // The guided Display detail is preserved in the structured output.
        assert!(rendered.contains("self-hash mismatch"), "got: {rendered}");
    }

    #[test]
    fn render_plugin_tier_mismatch_helper_works_directly() {
        let rendered = render_plugin_tier_mismatch(
            "auth-store",
            tau_domain::PluginRequiredTier::Strict,
            tau_ports::CapabilityTier::None,
        );
        insta::assert_snapshot!(rendered);
    }
}
