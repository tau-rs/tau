//! `tau init` — scaffold a project tau.toml.

use std::path::Path;

use crate::cli::InitArgs;
use crate::output::Output;

const SCAFFOLD_TEMPLATE: &str = r#"[project]
name = "{name}"

# Declare your model aliases here, then reference one from each agent's
# `model` field. Each alias maps to a concrete backend package + vendor id:
#   [models.default]
#   backend = "anthropic"
#   model   = "claude-haiku-4-5"
[models]

[agents.example]
display_name = "Example Agent"
package      = ""
model        = ""

[agents.example.prompt]
system = """
You are an example agent. Edit this prompt to give yourself a job.
"""
"#;

/// Scaffold used by `tau init --allow`: a governed project with an `[allow]`
/// ceiling (ADR-0057 / D2). `{ceiling}` is replaced with the commented,
/// least-privilege union of installed packages' declared capabilities. Note
/// model aliases move under `[allow.models]` once an `[allow]` ceiling exists.
const SCAFFOLD_ALLOW_TEMPLATE: &str = r#"[project]
name = "{name}"

# Governance ceiling (ADR-0057). `tau build` refuses to build unless every
# capability your agents and tools use is within this [allow] ceiling. The
# entries below are the least-privilege union of your installed packages'
# declared capabilities — uncomment and narrow the ones you actually need.
[allow]
{ceiling}

# When an [allow] ceiling is declared, model aliases live under [allow.models]:
#   [allow.models.default]
#   backend = "anthropic"
#   model   = "claude-haiku-4-5"

[agents.example]
display_name = "Example Agent"
package      = ""
model        = ""

[agents.example.prompt]
system = """
You are an example agent. Edit this prompt to give yourself a job.
"""
"#;

const GITIGNORE_HINT: &str = "hint: add `.tau/` to your .gitignore (tau-pkg installs packages there as machine-local state, per ADR-0004 §6)";

/// Run `tau init`: scaffold a project tau.toml at cwd.
pub async fn run(args: &InitArgs, output: &mut Output) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let target_path = cwd.join("tau.toml");

    let project_name = derive_project_name(&cwd)?;

    let scaffold = build_scaffold(&project_name, &cwd, args.allow);

    if args.dry_run {
        output.dry_run(format!(
            "would create tau.toml at {}",
            target_path.display()
        ))?;
        output.dry_run("file contents would be:")?;
        output.dry_run("---")?;
        for line in scaffold.lines() {
            output.dry_run(line)?;
        }
        output.dry_run("---")?;
        output.dry_run(format!("would print: {GITIGNORE_HINT}"))?;
        output.dry_run("no changes written.")?;
        return Ok(());
    }

    if target_path.exists() && !args.force {
        anyhow::bail!(
            "tau.toml already exists at {}; use --force to overwrite",
            target_path.display()
        );
    }

    std::fs::write(&target_path, &scaffold)?;

    if output.is_json() {
        let payload = serde_json::json!({
            "created": "tau.toml",
            "path": target_path,
            "force": args.force,
        });
        output.json(&payload)?;
    } else {
        output.human(&format!("created {}", target_path.display()))?;
    }

    output.status(GITIGNORE_HINT)?;

    Ok(())
}

/// Build the tau.toml scaffold. `--allow` produces a governed scaffold with a
/// least-privilege `[allow]` ceiling seeded from installed packages; otherwise
/// the bare stub.
fn build_scaffold(project_name: &str, cwd: &Path, allow: bool) -> String {
    if !allow {
        return SCAFFOLD_TEMPLATE.replace("{name}", project_name);
    }
    let lines = installed_capability_lines(cwd);
    let ceiling = if lines.is_empty() {
        // No installed packages to seed from — offer commented examples the
        // user can uncomment and narrow.
        [
            "#   \"fs.read\" = { paths = [\"./**\"] }",
            "#   \"net.http\" = { hosts = [\"api.example.com\"] }",
        ]
        .join("\n")
    } else {
        lines.join("\n")
    };
    SCAFFOLD_ALLOW_TEMPLATE
        .replace("{name}", project_name)
        .replace("{ceiling}", &ceiling)
}

/// Least-privilege union of installed packages' declared capabilities, each
/// rendered as a COMMENTED `[allow]` ceiling line (sorted, deduped). Returns
/// empty when there is no scope/lockfile or no representable capabilities —
/// e.g. a fresh project with nothing installed yet.
fn installed_capability_lines(cwd: &Path) -> Vec<String> {
    let Ok(scope) = tau_pkg::Scope::resolve(cwd) else {
        return Vec::new();
    };
    let Ok(lockfile) = tau_pkg::lockfile::LockFile::load(&scope.lockfile_path()) else {
        return Vec::new();
    };
    let mut lines = std::collections::BTreeSet::new();
    for pkg in &lockfile.packages {
        let toml_path = scope
            .package_dir(&pkg.name, &pkg.active_version)
            .join("tau.toml");
        let Ok(manifest) = tau_pkg::read_manifest(&toml_path) else {
            continue;
        };
        for cap in manifest.capabilities() {
            if let Some(line) = cap_to_allow_line(cap) {
                lines.insert(line);
            }
        }
    }
    lines.into_iter().collect()
}

/// Render one `tau_domain::Capability` as a commented `[allow]` ceiling line,
/// or `None` if the capability kind is not one of the root-cap kinds valid in
/// `[allow]` (see `tau_pkg::project::allow::ALLOWED_CAP_KINDS`).
fn cap_to_allow_line(cap: &tau_domain::Capability) -> Option<String> {
    let v = serde_json::to_value(cap).ok()?;
    let obj = v.as_object()?;
    let kind = obj.get("kind")?.as_str()?;
    let field = match kind {
        "fs.read" | "fs.write" | "fs.exec" => "paths",
        "net.http" => "hosts",
        "process.spawn" => "commands",
        _ => return None,
    };
    let items = obj.get(field)?.as_array()?;
    let rendered: Vec<String> = items
        .iter()
        .filter_map(|x| x.as_str())
        .map(|s| format!("{s:?}"))
        .collect();
    Some(format!(
        "#   \"{kind}\" = {{ {field} = [{}] }}",
        rendered.join(", ")
    ))
}

fn derive_project_name(cwd: &Path) -> anyhow::Result<String> {
    let name = cwd
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_default();
    if name.is_empty() {
        anyhow::bail!(
            "could not derive project name from cwd {:?}; use a directory with a non-empty name",
            cwd
        );
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_project_name_from_named_dir() {
        let path = Path::new("/Users/me/projects/foo-bar");
        let name = derive_project_name(path).unwrap();
        assert_eq!(name, "foo-bar");
    }

    #[test]
    fn derive_project_name_rejects_root() {
        let path = Path::new("/");
        let result = derive_project_name(path);
        assert!(result.is_err());
    }

    #[test]
    fn scaffold_template_is_valid_toml() {
        let scaffold = SCAFFOLD_TEMPLATE.replace("{name}", "test-project");
        let parsed: toml::Table = toml::from_str(&scaffold).expect("scaffold should be valid TOML");
        let project = parsed.get("project").unwrap().as_table().unwrap();
        assert_eq!(
            project.get("name").unwrap().as_str().unwrap(),
            "test-project"
        );
    }

    #[test]
    fn allow_scaffold_on_empty_dir_has_allow_section_and_examples() {
        let dir = tempfile::tempdir().unwrap();
        let s = build_scaffold("demo", dir.path(), true);
        assert!(s.contains("[allow]"), "got: {s}");
        // No packages installed → commented example ceiling lines.
        assert!(s.contains("#   \"fs.read\""), "got: {s}");
        assert!(s.contains("[allow.models]"), "got: {s}");
        // Must remain parseable TOML even though it's a stub to fill in.
        toml::from_str::<toml::Table>(&s).expect("allow scaffold is valid TOML");
    }

    #[test]
    fn bare_scaffold_has_no_allow_section() {
        let dir = tempfile::tempdir().unwrap();
        let s = build_scaffold("demo", dir.path(), false);
        assert!(!s.contains("[allow]"), "got: {s}");
        assert!(s.contains("[models]"), "got: {s}");
    }

    #[test]
    fn cap_to_allow_line_renders_whitelisted_kinds() {
        let cap: tau_domain::Capability =
            serde_json::from_value(serde_json::json!({"kind": "fs.read", "paths": ["/proj/**"]}))
                .unwrap();
        assert_eq!(
            cap_to_allow_line(&cap).unwrap(),
            "#   \"fs.read\" = { paths = [\"/proj/**\"] }"
        );
    }

    #[test]
    fn cap_to_allow_line_skips_non_root_kinds() {
        // agent.spawn is intentionally not a root [allow] ceiling kind.
        let cap: tau_domain::Capability = serde_json::from_value(
            serde_json::json!({"kind": "agent.spawn", "allowed_kinds": ["critic"]}),
        )
        .unwrap();
        assert!(cap_to_allow_line(&cap).is_none());
    }

    #[test]
    fn allow_scaffold_seeds_ceiling_from_installed_package_caps() {
        // A project scope with a lockfile + one installed package whose
        // manifest declares fs.read → the --allow scaffold lists it (commented).
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".tau")).unwrap();
        std::fs::write(
            dir.path().join("tau-lock.toml"),
            r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "sensors"
active_version = "0.1.0"
source = "https://example.com/sensors.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"
"#,
        )
        .unwrap();
        let pkg_dir = dir.path().join(".tau/packages/sensors/0.1.0");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("tau.toml"),
            r#"name = "sensors"
version = "0.1.0"
description = "sensor tools"
authors = ["a <a@example.com>"]
source = "https://example.com/sensors.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["/proj/sensors/**"]
"#,
        )
        .unwrap();

        let s = build_scaffold("demo", dir.path(), true);
        assert!(
            s.contains("#   \"fs.read\" = { paths = [\"/proj/sensors/**\"] }"),
            "expected seeded ceiling line, got:\n{s}"
        );
    }

    #[test]
    fn scaffold_template_validates_via_project_config() {
        use crate::config::project::UncheckedProjectConfig;
        let scaffold = SCAFFOLD_TEMPLATE.replace("{name}", "test-project");
        let unchecked: UncheckedProjectConfig = toml::from_str(&scaffold).unwrap();
        // The agent has empty package + empty model alias; this fails validation.
        let result = unchecked.validate();
        assert!(
            result.is_err(),
            "stub agent fields are intentionally empty for the user to fill in"
        );
    }
}
