//! Parsing of individual definition files (`agents/**/*.md`, `*.toml`).
//! Errors are reason-only `String`s — the scanner attaches the file path.

use std::path::PathBuf;

use super::super::project::{UncheckedAgent, UncheckedPrompt, UncheckedTool};

/// Replace every `\r\n` with `\n` (spec: build-time CRLF normalization).
#[allow(dead_code)] // wired up by Task 3 (dirs/scan.rs)
pub(crate) fn normalize_crlf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Split `---`-fenced YAML frontmatter from the markdown body.
/// The first line must be exactly `---`; the closing fence is the next
/// line that is exactly `---`. Body keeps everything after the closing
/// fence's newline.
#[allow(dead_code)] // wired up by Task 3 (dirs/scan.rs)
pub(crate) fn split_frontmatter(text: &str) -> Result<(String, String), String> {
    let missing = || {
        "missing `---` frontmatter fence on line 1 (a non-definition file must be \
         prefixed with `_` to be ignored)"
            .to_string()
    };
    let rest = text.strip_prefix("---\n").ok_or_else(missing)?;
    // Closing fence: either a leading "---\n" (empty frontmatter) or "\n---\n"
    // later; also accept a file ending exactly in "\n---".
    if let Some(body) = rest.strip_prefix("---\n") {
        return Ok((String::new(), body.to_string()));
    }
    if rest == "---" {
        return Ok((String::new(), String::new()));
    }
    if let Some(idx) = rest.find("\n---\n") {
        return Ok((rest[..idx].to_string(), rest[idx + 5..].to_string()));
    }
    if let Some(yaml) = rest.strip_suffix("\n---") {
        return Ok((yaml.to_string(), String::new()));
    }
    Err("unterminated `---` frontmatter fence".to_string())
}

/// Parse an `agents/**/*.md` definition: YAML frontmatter → `UncheckedAgent`
/// (full `[agents.X]` schema), body → inline system prompt.
#[allow(dead_code)] // wired up by Task 3 (dirs/scan.rs)
pub(crate) fn parse_agent_md(raw: &str) -> Result<UncheckedAgent, String> {
    let text = normalize_crlf(raw);
    let (yaml, body) = split_frontmatter(&text)?;
    // Targeted checks for forbidden keys before the typed parse. `name` isn't
    // a field on `UncheckedAgent` at all, so without this check the typed
    // parse would reject it as a generic unknown field (a confusing error).
    // `prompt`, however, IS a real `UncheckedAgent` field — without this
    // check the typed parse would silently accept it, letting the
    // frontmatter's `prompt` fight the markdown body for the system prompt.
    // Do not delete this as redundant UX polish; it is load-bearing for
    // `prompt`.
    if !yaml.trim().is_empty() {
        let map: serde_yaml::Mapping =
            serde_yaml::from_str(&yaml).map_err(|e| format!("frontmatter YAML: {e}"))?;
        for key in ["name", "prompt"] {
            if map.contains_key(serde_yaml::Value::String(key.to_string())) {
                let why = match key {
                    "name" => "the file path defines the name",
                    _ => "the markdown body is the system prompt",
                };
                return Err(format!("frontmatter key `{key}` is not allowed: {why}"));
            }
        }
    }
    // Typed parse from the raw string so serde reports duplicate fields.
    let mut agent: UncheckedAgent = if yaml.trim().is_empty() {
        serde_yaml::from_str("{}").map_err(|e| format!("frontmatter: {e}"))?
    } else {
        serde_yaml::from_str(&yaml).map_err(|e| format!("frontmatter: {e}"))?
    };
    if !body.trim().is_empty() {
        agent.prompt = Some(UncheckedPrompt {
            system: Some(body),
            system_file: None::<PathBuf>,
        });
    }
    Ok(agent)
}

/// Parse an `agents/**/*.toml` definition (the `[agents.X]` table body).
#[allow(dead_code)] // wired up by Task 3 (dirs/scan.rs)
pub(crate) fn parse_agent_toml(raw: &str) -> Result<UncheckedAgent, String> {
    forbid_name_key(raw)?;
    toml::from_str(raw).map_err(|e| e.to_string())
}

/// Parse a `tools/**/*.toml` definition (the `[tools.X]` table body).
#[allow(dead_code)] // wired up by Task 3 (dirs/scan.rs)
pub(crate) fn parse_tool_toml(raw: &str) -> Result<UncheckedTool, String> {
    forbid_name_key(raw)?;
    toml::from_str(raw).map_err(|e| e.to_string())
}

/// Reject a `name` key: the definition's name is derived from the file
/// path, not declared inline (keeps directory-based and inline `[tools.X]`
/// definitions from disagreeing about identity).
#[allow(dead_code)] // wired up by Task 3 (dirs/scan.rs)
fn forbid_name_key(raw: &str) -> Result<(), String> {
    let table: toml::Table = toml::from_str(raw).map_err(|e| e.to_string())?;
    if table.contains_key("name") {
        return Err("key `name` is not allowed: the file path defines the name".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "---\ndisplay_name: Strict\npackage: anthropic@^1\nmodel: fast\ntool_refs: [\"github/search\"]\n---\nYou review code.\n";

    #[test]
    fn md_parses_frontmatter_and_body_prompt() {
        let a = parse_agent_md(MD).unwrap();
        assert_eq!(a.display_name, "Strict");
        assert_eq!(a.tool_refs, vec!["github/search"]);
        let p = a.prompt.unwrap();
        assert_eq!(p.system.as_deref(), Some("You review code.\n"));
        assert!(p.system_file.is_none());
    }

    #[test]
    fn md_crlf_normalized() {
        let crlf = MD.replace('\n', "\r\n");
        let a = parse_agent_md(&crlf).unwrap();
        assert_eq!(
            a.prompt.unwrap().system.as_deref(),
            Some("You review code.\n")
        );
    }

    #[test]
    fn md_empty_body_means_no_prompt() {
        let a = parse_agent_md("---\ndisplay_name: X\npackage: p@^1\n---\n").unwrap();
        assert!(a.prompt.is_none());
    }

    #[test]
    fn md_without_fence_is_error_mentioning_escape() {
        let e = parse_agent_md("# just a doc\n").unwrap_err();
        assert!(e.contains("frontmatter"), "{e}");
        assert!(
            e.contains("_"),
            "error must mention the `_` ignore escape: {e}"
        );
    }

    #[test]
    fn md_forbidden_keys() {
        let e = parse_agent_md("---\nname: x\ndisplay_name: X\npackage: p@^1\n---\n").unwrap_err();
        assert!(e.contains("`name`"), "{e}");
        let e =
            parse_agent_md("---\nprompt:\n  system: hi\ndisplay_name: X\npackage: p@^1\n---\nbody")
                .unwrap_err();
        assert!(e.contains("`prompt`"), "{e}");
    }

    #[test]
    fn md_unknown_and_duplicate_fields_error() {
        assert!(parse_agent_md("---\ndisplay_name: X\npackage: p@^1\nbogus: 1\n---\n").is_err());
        assert!(
            parse_agent_md("---\ndisplay_name: X\ndisplay_name: Y\npackage: p@^1\n---\n").is_err()
        );
    }

    #[test]
    fn toml_agent_allows_prompt_forbids_name() {
        let a = parse_agent_toml(
            "display_name = \"X\"\npackage = \"p@^1\"\n[prompt]\nsystem = \"hi\"\n",
        )
        .unwrap();
        assert_eq!(a.prompt.unwrap().system.as_deref(), Some("hi"));
        let e = parse_agent_toml("name = \"x\"\ndisplay_name = \"X\"\npackage = \"p@^1\"\n")
            .unwrap_err();
        assert!(e.contains("`name`"), "{e}");
    }

    #[test]
    fn toml_tool_parses() {
        let t = parse_tool_toml("native = \"ReadTemp\"\ndescription = \"d\"\n").unwrap();
        assert_eq!(t.description, "d");
    }

    // -- split_frontmatter: direct branch coverage --
    //
    // These exercise split_frontmatter directly rather than via
    // parse_agent_md: the empty-frontmatter cases would fail a typed
    // UncheckedAgent parse for an unrelated reason (missing required
    // `display_name`/`package`), which would obscure which branch of
    // split_frontmatter actually ran.

    #[test]
    fn split_frontmatter_empty_with_trailing_newline_no_body() {
        // "---\n---\n": closing fence immediately follows the opening one,
        // with a newline and nothing after — file.rs:28-29.
        let (yaml, body) = split_frontmatter("---\n---\n").unwrap();
        assert_eq!(yaml, "");
        assert_eq!(body, "");
    }

    #[test]
    fn split_frontmatter_empty_with_trailing_body() {
        // Same branch (file.rs:28-29) but with a non-empty body after the
        // closing fence.
        let (yaml, body) = split_frontmatter("---\n---\nBODY\n").unwrap();
        assert_eq!(yaml, "");
        assert_eq!(body, "BODY\n");
    }

    #[test]
    fn split_frontmatter_bare_empty_no_trailing_newline() {
        // "---\n---" with no trailing newline at all — file.rs:31-32.
        let (yaml, body) = split_frontmatter("---\n---").unwrap();
        assert_eq!(yaml, "");
        assert_eq!(body, "");
    }

    #[test]
    fn split_frontmatter_nonempty_yaml_no_trailing_newline_after_close() {
        // Non-empty frontmatter where the file ends exactly at the closing
        // `---` with no trailing newline — file.rs:37-39.
        let (yaml, body) = split_frontmatter("---\nfoo: bar\n---").unwrap();
        assert_eq!(yaml, "foo: bar");
        assert_eq!(body, "");
    }

    #[test]
    fn split_frontmatter_unterminated_fence_is_error() {
        // Opening fence present but no closing `---` anywhere — file.rs:40.
        let e = split_frontmatter("---\nfoo: bar\n").unwrap_err();
        assert!(e.contains("unterminated"), "{e}");
    }
}
