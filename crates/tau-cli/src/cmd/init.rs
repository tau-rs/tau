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

const GITIGNORE_HINT: &str = "hint: add `.tau/` to your .gitignore (tau-pkg installs packages there as machine-local state, per ADR-0004 §6)";

/// Run `tau init`: scaffold a project tau.toml at cwd.
pub async fn run(args: &InitArgs, output: &mut Output) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let target_path = cwd.join("tau.toml");

    let project_name = derive_project_name(&cwd)?;

    let scaffold = SCAFFOLD_TEMPLATE.replace("{name}", &project_name);

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
