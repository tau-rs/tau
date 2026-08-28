# Directory-Based Tool & Agent Definitions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let agents and tools be authored as files under project directories (`agents/**/*.{md,toml}`, `tools/**/*.toml`), opted in via a `[dirs]` table in tau.toml, merged into `ProjectConfig` before lowering so IR, governance, and verify are unchanged.

**Architecture:** A new `dirs` module in `tau-pkg` scans declared roots (strict hygiene, path=name with `/` separator, `[a-z0-9_-]` segments), parses `.md` (YAML frontmatter + body-as-inline-prompt) and `.toml` entry files into the existing `UncheckedAgent`/`UncheckedTool` types, and merges them at the unchecked level inside a new root-aware `ProjectConfig::parse_str_at`; `from_path` delegates to it so all call sites become dirs-aware. Two determinism fixes ride along: CRLF normalization for prompt-asset hashing and a containment guard on `read_prompt_file`.

**Tech Stack:** Rust workspace; `serde_yaml 0.9` (already used by tau-domain for SKILL.md), `toml`, `notify` (dev watcher), `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-08-22-dir-based-definitions-design.md`

## Global Constraints

- Every cargo command: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate> <filter>` (per repo CARGO RULES; use `timeout 180` + `cargo check` for checks; never bare cargo, never workspace-wide).
- Commit with explicit identity (lefthook tests can corrupt worktree git identity): `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "..."`.
- Name segments: `^[a-z0-9_-]+$`, no leading `_` (leading `_`/`.` entries are ignored by the scan before naming). OS-junk ignore list: `Thumbs.db`, `desktop.ini`. Separator: `/`.
- Frontmatter fences: line `---` first line of file, closing line `---`. Forbidden frontmatter keys: `name`, `prompt`. Md files are agents-root only.
- CRLF (`\r\n`) normalizes to `\n` in md text and in prompt-file bytes before hashing.
- `ProjectConfig` is `#[non_exhaustive]`; crate versions are workspace-managed — adding fields needs no per-crate semver bump.
- Rust edits follow workspace lints (warnings are deny in CI); run `cargo fmt` before each commit: `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p <crate>`.

---

### Task 1: `[dirs]` schema, validated `DirsEntry`, error variants

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (struct fields near line 16-58; error enum at ~1129; `parse_str` at ~2915; `validate()` — locate via `rg -n "fn validate" crates/tau-pkg/src/project/project.rs`)
- Test: inline `#[cfg(test)]` tests in the same file (existing convention, see `validate_rejects_empty_project_name` ~line 3069)

**Interfaces:**
- Produces: `pub struct UncheckedDirs { pub agents: Option<String>, pub tools: Option<String> }` (serde, `deny_unknown_fields`); field `pub dirs: Option<UncheckedDirs>` on `UncheckedProjectConfig`; `pub struct DirsEntry { pub agents: Option<PathBuf>, pub tools: Option<PathBuf> }`; field `pub dirs: Option<DirsEntry>` on `ProjectConfig`; `ProjectConfigError::{DirsRequireRoot, DirsRoot{kind,path,reason}, DirsRootsOverlap{a,b}, DefFile{file,reason}, DuplicateDefinition{kind,name,file}}`.
- Consumes: nothing new.

- [ ] **Step 1: Write failing tests** (in the existing `mod tests` of project.rs):

```rust
#[test]
fn dirs_table_parses_and_validates() {
    let toml = r#"
[project]
name = "p"
[dirs]
agents = "agents"
tools  = "defs/tools"
"#;
    // parse_str must REJECT [dirs] (no root available to scan).
    let err = ProjectConfig::parse_str(toml).unwrap_err();
    assert!(matches!(err, ProjectConfigError::DirsRequireRoot), "got {err:?}");
}

#[test]
fn dirs_unknown_key_rejected() {
    let toml = "[project]\nname = \"p\"\n[dirs]\nskils = \"x\"\n";
    assert!(ProjectConfig::parse_str(toml).is_err());
}

#[test]
fn dirs_syntactic_validation_rejects_bad_roots() {
    // Validation helper is exercised directly (fs checks live in scan, Task 3).
    for bad in ["/abs/agents", "", "../up", "./agents", ".tau/agents", "_agents", "a/.hidden/b"] {
        let err = validate_dirs_decl("agents", bad).unwrap_err();
        assert!(matches!(err, ProjectConfigError::DirsRoot { .. }), "{bad}: {err:?}");
    }
    assert!(validate_dirs_decl("agents", "agents").is_ok());
    assert!(validate_dirs_decl("tools", "defs/tools").is_ok());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg dirs_`
Expected: compile FAIL (`UncheckedDirs`/`DirsRequireRoot`/`validate_dirs_decl` not defined).

- [ ] **Step 3: Implement.** In project.rs:

```rust
/// `[dirs]` table — opt-in directory-based definition roots (relative paths).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedDirs {
    /// Root scanned for `agents/**/*.{md,toml}` definitions.
    #[serde(default)]
    pub agents: Option<String>,
    /// Root scanned for `tools/**/*.toml` definitions.
    #[serde(default)]
    pub tools: Option<String>,
}

/// Validated `[dirs]` declaration (paths as declared, relative to project root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirsEntry {
    /// Agents root, if declared.
    pub agents: Option<PathBuf>,
    /// Tools root, if declared.
    pub tools: Option<PathBuf>,
}

/// Syntactic validation of one `[dirs]` declaration: relative, no `..`/`.`
/// components, no component starting with `.` or `_`. Existence, containment,
/// and overlap need the filesystem and run in `dirs::scan_dirs`.
pub(crate) fn validate_dirs_decl(
    kind: &'static str,
    decl: &str,
) -> Result<PathBuf, ProjectConfigError> {
    let err = |reason: &str| ProjectConfigError::DirsRoot {
        kind,
        path: decl.to_string(),
        reason: reason.to_string(),
    };
    if decl.is_empty() {
        return Err(err("must not be empty"));
    }
    let p = std::path::Path::new(decl);
    if p.is_absolute() {
        return Err(err("must be a relative path inside the project root"));
    }
    for comp in p.components() {
        match comp {
            std::path::Component::Normal(s) => {
                let s = s.to_str().ok_or_else(|| err("must be UTF-8"))?;
                if s.starts_with('.') || s.starts_with('_') {
                    return Err(err("components must not start with `.` or `_`"));
                }
            }
            _ => return Err(err("`..` and `.` components are not allowed")),
        }
    }
    Ok(p.to_path_buf())
}
```

Add `#[serde(default)] pub dirs: Option<UncheckedDirs>` to `UncheckedProjectConfig`, `pub dirs: Option<DirsEntry>` to `ProjectConfig`. In `validate()` (where `ProjectConfig` is constructed), build the field:

```rust
let dirs = match &self.dirs {
    None => None,
    Some(d) => Some(DirsEntry {
        agents: d.agents.as_deref().map(|s| validate_dirs_decl("agents", s)).transpose()?,
        tools: d.tools.as_deref().map(|s| validate_dirs_decl("tools", s)).transpose()?,
    }),
};
```

In `parse_str` (line ~2915), after deserializing `unchecked`, add:

```rust
if unchecked.dirs.is_some() {
    return Err(ProjectConfigError::DirsRequireRoot);
}
```

Error variants (thiserror, same style as neighbors at ~1129):

```rust
/// `[dirs]` present but parsing had no project root to scan from.
#[error("[dirs] requires a project root; load via `ProjectConfig::from_path` or `parse_str_at` (`parse_str` cannot scan directories)")]
DirsRequireRoot,

/// A `[dirs]` root declaration is invalid.
#[error("[dirs] {kind} = {path:?}: {reason}")]
DirsRoot { /** which key */ kind: &'static str, /** declared value */ path: String, /** why */ reason: String },

/// Two `[dirs]` roots overlap (equal or nested).
#[error("[dirs] roots overlap: {a:?} and {b:?} — roots must be disjoint")]
DirsRootsOverlap { /** first root */ a: String, /** second root */ b: String },

/// A scanned definition file is invalid (hygiene, parse, or naming).
#[error("definition file {file}: {reason}")]
DefFile { /** project-root-relative path */ file: String, /** why */ reason: String },

/// The same definition name arrived from two sources.
#[error("duplicate {kind} definition {name:?} (from {file}); already defined in tau.toml or another definition file")]
DuplicateDefinition { /** \"agent\" | \"tool\" */ kind: &'static str, /** full path-name */ name: String, /** offending file */ file: String },
```

- [ ] **Step 4: Run tests, expect PASS.** Same nextest command. Also run the full crate to catch `deny_unknown_fields`/exhaustiveness fallout: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`.

- [ ] **Step 5: fmt + commit**

```bash
git add crates/tau-pkg/src/project/project.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(pkg): [dirs] table schema, DirsEntry, error taxonomy"
```

---

### Task 2: definition-file parsers (`dirs/file.rs`)

**Files:**
- Create: `crates/tau-pkg/src/project/dirs/mod.rs` (just `pub(crate) mod file;` for now; `scan` added in Task 3)
- Create: `crates/tau-pkg/src/project/dirs/file.rs`
- Modify: `crates/tau-pkg/src/project/mod.rs` (add `pub mod dirs;` — inspect existing module list first)
- Modify: `crates/tau-pkg/Cargo.toml` + root `Cargo.toml` (`serde_yaml = "0.9"` under `[workspace.dependencies]`, `serde_yaml = { workspace = true }` in tau-pkg `[dependencies]`)
- Test: inline `#[cfg(test)]` in `file.rs`

**Interfaces:**
- Consumes: `UncheckedAgent`, `UncheckedTool`, `UncheckedPrompt` from `super::super::project` (fields verified: `UncheckedPrompt { system: Option<String>, system_file: Option<PathBuf> }`; `UncheckedAgent` requires `display_name`, `package`).
- Produces (all `pub(crate)`, error type `String` = reason only; the scanner wraps it with the file path into `ProjectConfigError::DefFile`):
  - `fn normalize_crlf(s: &str) -> String`
  - `fn split_frontmatter(text: &str) -> Result<(String, String), String>` — input must be CRLF-normalized; returns (yaml, body); body = text after closing fence with one leading `\n` stripped
  - `fn parse_agent_md(raw: &str) -> Result<UncheckedAgent, String>`
  - `fn parse_agent_toml(raw: &str) -> Result<UncheckedAgent, String>`
  - `fn parse_tool_toml(raw: &str) -> Result<UncheckedTool, String>`

- [ ] **Step 1: Write failing tests** in `file.rs`:

```rust
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
        assert_eq!(a.prompt.unwrap().system.as_deref(), Some("You review code.\n"));
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
        assert!(e.contains("_"), "error must mention the `_` ignore escape: {e}");
    }

    #[test]
    fn md_forbidden_keys() {
        let e = parse_agent_md("---\nname: x\ndisplay_name: X\npackage: p@^1\n---\n").unwrap_err();
        assert!(e.contains("`name`"), "{e}");
        let e = parse_agent_md("---\nprompt:\n  system: hi\ndisplay_name: X\npackage: p@^1\n---\nbody").unwrap_err();
        assert!(e.contains("`prompt`"), "{e}");
    }

    #[test]
    fn md_unknown_and_duplicate_fields_error() {
        assert!(parse_agent_md("---\ndisplay_name: X\npackage: p@^1\nbogus: 1\n---\n").is_err());
        assert!(parse_agent_md("---\ndisplay_name: X\ndisplay_name: Y\npackage: p@^1\n---\n").is_err());
    }

    #[test]
    fn toml_agent_allows_prompt_forbids_name() {
        let a = parse_agent_toml(
            "display_name = \"X\"\npackage = \"p@^1\"\n[prompt]\nsystem = \"hi\"\n",
        ).unwrap();
        assert_eq!(a.prompt.unwrap().system.as_deref(), Some("hi"));
        let e = parse_agent_toml("name = \"x\"\ndisplay_name = \"X\"\npackage = \"p@^1\"\n").unwrap_err();
        assert!(e.contains("`name`"), "{e}");
    }

    #[test]
    fn toml_tool_parses() {
        let t = parse_tool_toml("native = \"ReadTemp\"\ndescription = \"d\"\n").unwrap();
        assert_eq!(t.description, "d");
    }
}
```

(Adapt the `t.description` assertion to `UncheckedTool`'s actual field type — check `crates/tau-pkg/src/project/project.rs:564`; if `description` is `Option<String>` compare with `Some("d")`.)

- [ ] **Step 2: Run to verify failure** — `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg dirs::file`. Expected: compile FAIL.

- [ ] **Step 3: Implement** `file.rs`:

```rust
//! Parsing of individual definition files (`agents/**/*.md`, `*.toml`).
//! Errors are reason-only `String`s — the scanner attaches the file path.

use std::path::PathBuf;

use super::super::project::{UncheckedAgent, UncheckedPrompt, UncheckedTool};

/// Replace every `\r\n` with `\n` (spec: build-time CRLF normalization).
pub(crate) fn normalize_crlf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Split `---`-fenced YAML frontmatter from the markdown body.
/// The first line must be exactly `---`; the closing fence is the next
/// line that is exactly `---`. Body keeps everything after the closing
/// fence's newline.
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
pub(crate) fn parse_agent_md(raw: &str) -> Result<UncheckedAgent, String> {
    let text = normalize_crlf(raw);
    let (yaml, body) = split_frontmatter(&text)?;
    // Targeted checks for forbidden keys before the typed parse (the typed
    // parse would report them as generic unknown fields).
    if !yaml.trim().is_empty() {
        let map: serde_yaml::Mapping = serde_yaml::from_str(&yaml)
            .map_err(|e| format!("frontmatter YAML: {e}"))?;
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
pub(crate) fn parse_agent_toml(raw: &str) -> Result<UncheckedAgent, String> {
    forbid_name_key(raw)?;
    toml::from_str(raw).map_err(|e| e.to_string())
}

/// Parse a `tools/**/*.toml` definition (the `[tools.X]` table body).
pub(crate) fn parse_tool_toml(raw: &str) -> Result<UncheckedTool, String> {
    forbid_name_key(raw)?;
    toml::from_str(raw).map_err(|e| e.to_string())
}

fn forbid_name_key(raw: &str) -> Result<(), String> {
    let table: toml::Table = toml::from_str(raw).map_err(|e| e.to_string())?;
    if table.contains_key("name") {
        return Err("key `name` is not allowed: the file path defines the name".to_string());
    }
    Ok(())
}
```

Note on the empty-frontmatter branch: `serde_yaml::from_str::<UncheckedAgent>("{}")` fails because `display_name`/`package` are required — that IS the correct error ("missing field `display_name`"), keep it (`md_empty_body_means_no_prompt` test uses non-empty frontmatter). If `serde_yaml::from_str("{}")` produces an unhelpful message, map it: `.map_err(|_| "frontmatter is empty but `display_name` and `package` are required".to_string())`.

- [ ] **Step 4: Run tests, expect PASS** (same filter, then `-p tau-pkg` full).

- [ ] **Step 5: fmt + commit** — `feat(pkg): definition-file parsers (md frontmatter + toml entries)`.

---

### Task 3: recursive scanner (`dirs/scan.rs`)

**Files:**
- Create: `crates/tau-pkg/src/project/dirs/scan.rs`
- Modify: `crates/tau-pkg/src/project/dirs/mod.rs` (`pub(crate) mod file; mod scan; pub use scan::{scan_dirs, ScannedDefs, definition_files};`)
- Test: inline in `scan.rs` using `tempfile::TempDir` (already a tau-pkg dependency)

**Interfaces:**
- Consumes: Task 2 parsers; Task 1 `UncheckedDirs`, `validate_dirs_decl`, error variants.
- Produces:
  - `pub struct ScannedDefs { pub agents: BTreeMap<String, (UncheckedAgent, PathBuf)>, pub tools: BTreeMap<String, (UncheckedTool, PathBuf)> }` (PathBuf = project-root-relative file path)
  - `pub fn scan_dirs(project_root: &Path, dirs: &UncheckedDirs) -> Result<ScannedDefs, ProjectConfigError>`
  - `pub fn definition_files(project_root: &Path, dirs: &DirsEntry) -> Result<Vec<PathBuf>, ProjectConfigError>` (root-relative paths of every definition file; for the `tau check` lint, Task 10)

- [ ] **Step 1: Write failing tests** (each builds a TempDir layout, calls `scan_dirs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const AGENT_MD: &str = "---\ndisplay_name: A\npackage: p@^1\n---\nprompt body\n";
    const TOOL_TOML: &str = "native = \"ReadTemp\"\n";

    fn dirs(agents: Option<&str>, tools: Option<&str>) -> UncheckedDirs {
        UncheckedDirs { agents: agents.map(String::from), tools: tools.map(String::from) }
    }

    fn write(root: &std::path::Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn scans_nested_and_names_by_path() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "agents/triage.md", AGENT_MD);
        write(t.path(), "agents/review/strict.md", AGENT_MD);
        write(t.path(), "agents/perf/strict.md", AGENT_MD);
        write(t.path(), "tools/github/search.toml", TOOL_TOML);
        let s = scan_dirs(t.path(), &dirs(Some("agents"), Some("tools"))).unwrap();
        let names: Vec<_> = s.agents.keys().cloned().collect();
        assert_eq!(names, ["perf/strict", "review/strict", "triage"]);
        assert!(s.tools.contains_key("github/search"));
    }

    #[test]
    fn ignores_escaped_and_junk_entries() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "agents/a.md", AGENT_MD);
        write(t.path(), "agents/_README.md", "not a definition");
        write(t.path(), "agents/.hidden.md", "no");
        write(t.path(), "agents/_drafts/wip.md", "no");
        write(t.path(), "agents/Thumbs.db", "junk");
        let s = scan_dirs(t.path(), &dirs(Some("agents"), None)).unwrap();
        assert_eq!(s.agents.len(), 1);
    }

    #[test]
    fn strict_hygiene_errors() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "agents/README.txt", "x"); // wrong extension
        let e = scan_dirs(t.path(), &dirs(Some("agents"), None)).unwrap_err();
        assert!(matches!(e, ProjectConfigError::DefFile { .. }), "{e:?}");

        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "tools/a.md", "x"); // md under tools root
        assert!(scan_dirs(t.path(), &dirs(None, Some("tools"))).is_err());
    }

    #[test]
    fn charset_enforced_on_segments() {
        for bad in ["agents/Bad.md", "agents/sp ace.md", "agents/v1.2.md", "agents/Sub/ok.md"] {
            let t = tempfile::TempDir::new().unwrap();
            write(t.path(), bad, AGENT_MD);
            assert!(scan_dirs(t.path(), &dirs(Some("agents"), None)).is_err(), "{bad}");
        }
    }

    #[test]
    fn md_toml_same_stem_collides() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "agents/x.md", AGENT_MD);
        write(t.path(), "agents/x.toml", "display_name = \"A\"\npackage = \"p@^1\"\n");
        let e = scan_dirs(t.path(), &dirs(Some("agents"), None)).unwrap_err();
        assert!(matches!(e, ProjectConfigError::DuplicateDefinition { .. }), "{e:?}");
    }

    #[test]
    fn symlinks_rejected() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "agents/real.md", AGENT_MD);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(t.path().join("agents/real.md"), t.path().join("agents/link.md")).unwrap();
            assert!(scan_dirs(t.path(), &dirs(Some("agents"), None)).is_err());
        }
    }

    #[test]
    fn root_checks() {
        let t = tempfile::TempDir::new().unwrap();
        // missing root
        assert!(matches!(
            scan_dirs(t.path(), &dirs(Some("agents"), None)).unwrap_err(),
            ProjectConfigError::DirsRoot { .. }
        ));
        // overlap (equal + nested)
        std::fs::create_dir_all(t.path().join("defs/tools")).unwrap();
        assert!(matches!(
            scan_dirs(t.path(), &dirs(Some("defs"), Some("defs/tools"))).unwrap_err(),
            ProjectConfigError::DirsRootsOverlap { .. }
        ));
        // escape via symlinked root
        #[cfg(unix)]
        {
            let outside = tempfile::TempDir::new().unwrap();
            std::os::unix::fs::symlink(outside.path(), t.path().join("linkroot")).unwrap();
            assert!(scan_dirs(t.path(), &dirs(Some("linkroot"), None)).is_err());
        }
        // empty existing dir is fine
        std::fs::create_dir_all(t.path().join("agents")).unwrap();
        let s = scan_dirs(t.path(), &dirs(Some("agents"), None)).unwrap();
        assert!(s.agents.is_empty());
    }
}
```

- [ ] **Step 2: Run to verify failure** — `... cargo nextest run -p tau-pkg dirs::scan`. Expected: compile FAIL.

- [ ] **Step 3: Implement** `scan.rs`:

```rust
//! Recursive, deterministic scan of `[dirs]` roots (spec: strict hygiene).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::super::project::{
    validate_dirs_decl, DirsEntry, ProjectConfigError, UncheckedAgent, UncheckedDirs,
    UncheckedTool,
};
use super::file;

const OS_JUNK: &[&str] = &["Thumbs.db", "desktop.ini"];

/// Definitions harvested from `[dirs]` roots. Paths are project-root-relative.
pub struct ScannedDefs {
    /// Full path-name → (entry, source file).
    pub agents: BTreeMap<String, (UncheckedAgent, PathBuf)>,
    /// Full path-name → (entry, source file).
    pub tools: BTreeMap<String, (UncheckedTool, PathBuf)>,
}

enum Kind { Agents, Tools }

pub fn scan_dirs(
    project_root: &Path,
    dirs: &UncheckedDirs,
) -> Result<ScannedDefs, ProjectConfigError> {
    let agents_root = dirs.agents.as_deref()
        .map(|d| resolve_root(project_root, "agents", d)).transpose()?;
    let tools_root = dirs.tools.as_deref()
        .map(|d| resolve_root(project_root, "tools", d)).transpose()?;
    if let (Some((_, a)), Some((_, t))) = (&agents_root, &tools_root) {
        if a.starts_with(t) || t.starts_with(a) {
            return Err(ProjectConfigError::DirsRootsOverlap {
                a: dirs.agents.clone().unwrap_or_default(),
                b: dirs.tools.clone().unwrap_or_default(),
            });
        }
    }
    let mut out = ScannedDefs { agents: BTreeMap::new(), tools: BTreeMap::new() };
    if let Some((rel, abs)) = agents_root {
        walk(Kind::Agents, &rel, &abs, &mut Vec::new(), &mut out)?;
    }
    if let Some((rel, abs)) = tools_root {
        walk(Kind::Tools, &rel, &abs, &mut Vec::new(), &mut out)?;
    }
    Ok(out)
}

/// Root-relative paths of all definition files (for `tau check`).
pub fn definition_files(
    project_root: &Path,
    dirs: &DirsEntry,
) -> Result<Vec<PathBuf>, ProjectConfigError> {
    let unchecked = UncheckedDirs {
        agents: dirs.agents.as_ref().map(|p| p.to_string_lossy().into_owned()),
        tools: dirs.tools.as_ref().map(|p| p.to_string_lossy().into_owned()),
    };
    let s = scan_dirs(project_root, &unchecked)?;
    Ok(s.agents.values().map(|(_, p)| p.clone())
        .chain(s.tools.values().map(|(_, p)| p.clone()))
        .collect())
}

/// Syntactic checks + existence + symlink-free containment. Returns
/// (declared relative path, absolute canonical path).
fn resolve_root(
    project_root: &Path,
    kind: &'static str,
    decl: &str,
) -> Result<(PathBuf, PathBuf), ProjectConfigError> {
    let rel = validate_dirs_decl(kind, decl)?;
    let err = |reason: String| ProjectConfigError::DirsRoot {
        kind, path: decl.to_string(), reason,
    };
    let abs = project_root.join(&rel);
    let canon = abs.canonicalize()
        .map_err(|_| err("directory does not exist".to_string()))?;
    let root_canon = project_root.canonicalize()
        .map_err(|e| err(format!("project root not canonicalizable: {e}")))?;
    if !canon.starts_with(&root_canon) {
        return Err(err("escapes the project root".to_string()));
    }
    if !canon.is_dir() {
        return Err(err("is not a directory".to_string()));
    }
    Ok((rel, canon))
}

fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn walk(
    kind: Kind, // reborrowed as &Kind if the compiler asks; keep by-value + Copy derive otherwise
    root_rel: &Path,
    dir_abs: &Path,
    segments: &mut Vec<String>,
    out: &mut ScannedDefs,
) -> Result<(), ProjectConfigError> { /* … */ }
```

`walk` body (derive `Clone, Copy` on `Kind`):

1. `read_dir(dir_abs)`, collect `DirEntry`s, sort by `file_name()`.
2. For each entry: `let fname = entry.file_name(); let Some(fname) = fname.to_str() else { return Err(DefFile { file: rel_of(entry), reason: "non-UTF-8 file name".into() }) };`
3. Skip if `fname.starts_with('.') || fname.starts_with('_') || OS_JUNK.contains(&fname)`.
4. `let meta = std::fs::symlink_metadata(entry.path())…`; if `meta.file_type().is_symlink()` → `DefFile { reason: "symlinks are not allowed in definition dirs" }`.
5. If dir: `valid_segment(fname)` else `DefFile { reason: "directory name must match [a-z0-9_-]+" }`; push segment, recurse, pop.
6. If file: match `(kind, extension)`: `(Agents, "md")` → `file::parse_agent_md`; `(_, "toml")` → kind-specific toml parser; anything else → `DefFile { reason: "not a definition (.md/.toml); prefix with `_` to ignore" }`. Stem via `fname.strip_suffix(".md")` / `".toml"`; `valid_segment(stem)` else `DefFile { reason: "file stem must match [a-z0-9_-]+ (no dots, lowercase ASCII)" }`.
7. `let name = segments.iter().chain([&stem.to_string()]).cloned().collect::<Vec<_>>().join("/")`; file path recorded as `root_rel.join(segments…).join(fname)` (forward-slash rendering happens in error copy via `.display()`; on Windows normalize with `path.to_string_lossy().replace('\\', "/")` when formatting the name — the NAME always uses `/` because it is built from segments, never from the OS path).
8. Insert into `out.agents` / `out.tools`; on `contains_key` → `DuplicateDefinition { kind: "agent"|"tool", name, file }`.
9. Parser `Err(reason)` wraps as `DefFile { file, reason }`.

- [ ] **Step 4: Run tests, expect PASS** (`dirs::scan`, then full `-p tau-pkg`).

- [ ] **Step 5: fmt + commit** — `feat(pkg): recursive [dirs] scanner with strict hygiene and path=name`.

---

### Task 4: merge + `parse_str_at` + call-site switch

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (`parse_str` ~2915, `from_path` ~2923)
- Modify: `crates/tau-cli/src/cmd/project_load.rs` (TOML branch, lines ~38-53)
- Test: create `crates/tau-pkg/tests/dirs_project.rs`

**Interfaces:**
- Consumes: `scan_dirs` (Task 3).
- Produces: `pub fn parse_str_at(toml: &str, project_root: &Path) -> Result<ProjectConfig, ProjectConfigError>`; `from_path` becomes dirs-aware (root = manifest parent). Every existing `from_path` call site (chat, resolve, check, serve, ir_dispatcher, workflow) inherits dirs support with no edits.

- [ ] **Step 1: Write failing integration test** `crates/tau-pkg/tests/dirs_project.rs`:

```rust
use tau_pkg::project::{ProjectConfig, ProjectConfigError};

const ROOT_TOML: &str = r#"
[project]
name = "p"
[dirs]
agents = "agents"
tools  = "tools"
[models]
default = { backend = "mock-llm", model = "m" }
"#;
const AGENT_MD: &str = "---\ndisplay_name: A\npackage: p@^1\nmodel: default\n---\nbody\n";

fn write(root: &std::path::Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

#[test]
fn from_path_merges_dir_definitions() {
    let t = tempfile::TempDir::new().unwrap();
    write(t.path(), "tau.toml", ROOT_TOML);
    write(t.path(), "agents/review/strict.md", AGENT_MD);
    write(t.path(), "tools/github/search.toml", "native = \"ReadTemp\"\n");
    let cfg = ProjectConfig::from_path(t.path().join("tau.toml")).unwrap();
    let agent = &cfg.agents["review/strict"];
    assert!(matches!(
        &agent.prompt,
        tau_pkg::project::project::PromptEntry::Inline(s) if s == "body\n"
    ));
    assert!(cfg.tools.contains_key("github/search"));
    assert!(cfg.dirs.is_some());
}

#[test]
fn inline_collision_is_hard_error() {
    let t = tempfile::TempDir::new().unwrap();
    let toml = format!("{ROOT_TOML}\n[agents.\"review/strict\"]\ndisplay_name = \"B\"\npackage = \"p@^1\"\n");
    write(t.path(), "tau.toml", &toml);
    write(t.path(), "agents/review/strict.md", AGENT_MD);
    std::fs::create_dir_all(t.path().join("tools")).unwrap();
    let err = ProjectConfig::from_path(t.path().join("tau.toml")).unwrap_err();
    assert!(matches!(err, ProjectConfigError::DuplicateDefinition { .. }), "{err:?}");
}

#[test]
fn project_without_dirs_is_unaffected() {
    let t = tempfile::TempDir::new().unwrap();
    write(t.path(), "tau.toml", "[project]\nname = \"p\"\n");
    assert!(ProjectConfig::from_path(t.path().join("tau.toml")).is_ok());
}
```

(Adjust the `PromptEntry` import path / matching to the real definition at project.rs:1096 — if fields differ, assert via the entry's accessor instead. The point under test: the md body arrives as the INLINE prompt.)

- [ ] **Step 2: Run to verify failure** — `... cargo nextest run -p tau-pkg --test dirs_project`. Expected: FAIL (`parse_str_at` missing / `from_path` rejects dirs via `DirsRequireRoot`).

- [ ] **Step 3: Implement.** In project.rs:

```rust
/// Parse + validate with a project root, enabling `[dirs]` scanning.
pub fn parse_str_at(
    toml_str: &str,
    project_root: &std::path::Path,
) -> Result<Self, ProjectConfigError> {
    let mut unchecked: UncheckedProjectConfig =
        toml::from_str(toml_str).map_err(|source| ProjectConfigError::ParseStr { source })?;
    if let Some(dirs) = unchecked.dirs.clone() {
        let scanned = crate::project::dirs::scan_dirs(project_root, &dirs)?;
        for (name, (agent, file)) in scanned.agents {
            if unchecked.agents.contains_key(&name) {
                return Err(ProjectConfigError::DuplicateDefinition {
                    kind: "agent", name, file: file.display().to_string(),
                });
            }
            unchecked.agents.insert(name, agent);
        }
        for (name, (tool, file)) in scanned.tools {
            if unchecked.tools.contains_key(&name) {
                return Err(ProjectConfigError::DuplicateDefinition {
                    kind: "tool", name, file: file.display().to_string(),
                });
            }
            unchecked.tools.insert(name, tool);
        }
    }
    unchecked.validate()
}
```

`from_path`: after the existing read, replace the deserialize+validate tail with `Self::parse_str_at(&bytes, path.parent().unwrap_or_else(|| std::path::Path::new(".")))`. Keep the `NotFound`/`Read` mapping untouched.

`project_load.rs` TOML branch: replace the `read_to_string` + `parse_str` pair with:

```rust
let project = ProjectConfig::from_path(&tau_toml)
    .map_err(|e| anyhow!("load {}: {e}", tau_toml.display()))?;
```

- [ ] **Step 4: Run** — `--test dirs_project`, then full `-p tau-pkg`, then `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli`. Expected: all green.

- [ ] **Step 5: fmt + commit** — `feat(pkg): parse_str_at merges [dirs] definitions; from_path dirs-aware`.

---

### Task 5: byte-equal IR equivalence test

**Files:**
- Create: `crates/tau-sdk-codegen/tests/dirs_byte_equal.rs` (crate already depends on tau-pkg, tau-ir-lower, tempfile; helpers in `tests/common`)

**Interfaces:**
- Consumes: `ProjectConfig::from_path` (Task 4), `common::{lower_toml_bytes, lower_config_bytes}` (existing, see `tests/byte_equal.rs:21-27`).

- [ ] **Step 1: Write the test** (fails until Tasks 1-4 land; if executed in order it passes immediately — it is the acceptance gate for the invariant):

```rust
mod common;

/// Spec invariant: a dir-authored project lowers to byte-identical IR vs its
/// inline-authored twin (`agents/review/strict.md` ≡ [agents."review/strict"]
/// with prompt.system = body).
#[test]
fn dir_authored_equals_inline_authored() {
    let t = tempfile::TempDir::new().unwrap();
    let root = "\
[project]\nname = \"p\"\n\n[dirs]\nagents = \"agents\"\ntools = \"tools\"\n\n\
[models]\ndefault = { backend = \"mock-llm\", model = \"m\" }\n";
    std::fs::write(t.path().join("tau.toml"), root).unwrap();
    std::fs::create_dir_all(t.path().join("agents/review")).unwrap();
    std::fs::create_dir_all(t.path().join("tools")).unwrap();
    std::fs::write(
        t.path().join("agents/review/strict.md"),
        "---\ndisplay_name: A\npackage: p@^1\nmodel: default\n---\nYou review.\n",
    ).unwrap();

    let dir_cfg = tau_pkg::project::ProjectConfig::from_path(t.path().join("tau.toml")).unwrap();
    let dir_bytes = common::lower_config_bytes(&dir_cfg);

    let inline = "\
[project]\nname = \"p\"\n\n\
[models]\ndefault = { backend = \"mock-llm\", model = \"m\" }\n\n\
[agents.\"review/strict\"]\ndisplay_name = \"A\"\npackage = \"p@^1\"\nmodel = \"default\"\n\
[agents.\"review/strict\".prompt]\nsystem = \"You review.\\n\"\n";
    let inline_bytes = common::lower_toml_bytes(inline);

    assert_eq!(dir_bytes, inline_bytes, "dir-authored and inline-authored IR must be byte-identical");
}

/// CRLF-checkout simulation: the same md with \r\n endings lowers identically.
#[test]
fn dir_authored_crlf_invariant() {
    // Same layout as above but write the .md with \r\n line endings; assert
    // equal bytes to the LF variant.
}
```

Fill the second test by extracting the layout builder into a local `fn build(t: &std::path::Path, md: &str) -> Vec<u8>` and asserting `build(lf) == build(crlf)`. Check `common/mod.rs` for the exact helper signatures before writing (`lower_config_bytes(&ProjectConfig) -> Vec<u8>` expected; if the model/`packages` fields are required by lowering — see `parse.rs:757` fixture using `packages = ["mock-llm"]` — add `packages = [\"mock-llm\"]` to BOTH tau.toml variants identically).

- [ ] **Step 2: Run** — `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-sdk-codegen --test dirs_byte_equal`. Expected: PASS. If bytes differ, diff the two `ProjectConfig`s first (prompt body trailing-newline handling in `split_frontmatter` is the likely culprit — fix THERE, never by loosening the assert).

- [ ] **Step 3: Commit** — `test(sdk-codegen): dir-authored ≡ inline-authored byte-equal IR`.

---

### Task 6: CRLF normalization for prompt-asset bytes

**Files:**
- Modify: `crates/tau-ir-lower/src/lower/parse.rs` (prompt lowering, lines ~114-134)
- Test: inline test beside the existing parse-stage tests in the same file

**Interfaces:**
- Produces: `fn normalize_crlf_bytes(bytes: Vec<u8>) -> Vec<u8>` (private to parse.rs). Applied to `prompt_file` output before `asset_hash`.

- [ ] **Step 1: Write failing test** (this crate injects file reads via the `prompt_file` closure — no fs needed; mirror the fixture style at parse.rs:755 with an agent whose prompt uses `system_file`):

```rust
#[test]
fn prompt_asset_hash_is_crlf_invariant() {
    let toml = r#"
packages = ["mock-llm"]
[project]
name = "p"
[models]
default = { backend = "mock-llm", model = "m" }
[agents.solo]
display_name = "Solo"
package      = "p@^0.1"
model        = "default"
[agents.solo.prompt]
system_file  = "prompt.md"
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse");
    let lf: crate::lower::PromptFileReader = &|_p| Ok(b"line one\nline two\n".to_vec());
    let crlf: crate::lower::PromptFileReader = &|_p| Ok(b"line one\r\nline two\r\n".to_vec());
    let out_lf = parse(&config, &lf).expect("lf");
    let out_crlf = parse(&config, &crlf).expect("crlf");
    assert_eq!(out_lf.assets.keys().collect::<Vec<_>>(), out_crlf.assets.keys().collect::<Vec<_>>());
}
```

(Adapt the closure type + `parse()` arity to the file's real signatures — copy whatever `parse_registers_tool_node_for_each_step` at parse.rs:755 does for its `&no_prompt_files` argument, and whatever field of the parse output exposes collected assets. The assertion is: identical asset hashes for CRLF vs LF bytes.)

- [ ] **Step 2: Run to verify failure** — `... cargo nextest run -p tau-ir-lower prompt_asset_hash`. Expected: FAIL (hashes differ).

- [ ] **Step 3: Implement** in parse.rs — insert after the `prompt_file(p)` read (line ~123):

```rust
let bytes = normalize_crlf_bytes(bytes);
```

```rust
/// Build-time CRLF normalization (spec: dir-based definitions, determinism).
/// Without this, an autocrlf Windows checkout changes prompt asset hashes and
/// breaks `tau verify` cross-platform (same class as the #553 *.wit incident).
fn normalize_crlf_bytes(input: alloc::vec::Vec<u8>) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'\r' && input.get(i + 1) == Some(&b'\n') {
            i += 1; // skip the \r, keep the \n
            continue;
        }
        out.push(input[i]);
        i += 1;
    }
    out
}
```

(Use plain `Vec` if the file is std; match its existing `alloc::`/std usage.)

- [ ] **Step 4: Run tests, expect PASS**, then full `-p tau-ir-lower`.

- [ ] **Step 5: fmt + commit** — `fix(ir-lower): normalize CRLF in prompt bytes before asset hashing`.

---

### Task 7: `read_prompt_file` containment guard

**Files:**
- Modify: `crates/tau-pkg/src/bundle/build.rs` (`read_prompt_file`, locate with `rg -n "read_prompt_file" crates/tau-pkg/src/bundle/build.rs`)
- Test: inline tests in build.rs (or the crate's existing bundle test module — follow local convention)

**Interfaces:**
- Consumes/Produces: same signature `pub fn read_prompt_file(rel: &Path, project_root: &Path) -> Result<Vec<u8>, std::io::Error>`; behavior tightens: absolute paths and root-escapes now fail with `ErrorKind::InvalidInput`.

- [ ] **Step 1: Write failing tests**:

```rust
#[test]
fn read_prompt_file_rejects_absolute_and_escape() {
    let t = tempfile::TempDir::new().unwrap();
    std::fs::write(t.path().join("p.md"), "ok").unwrap();
    let outside = tempfile::TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret.md"), "no").unwrap();

    assert_eq!(read_prompt_file(std::path::Path::new("p.md"), t.path()).unwrap(), b"ok");
    // absolute
    assert!(read_prompt_file(&outside.path().join("secret.md"), t.path()).is_err());
    // ../ escape
    let escape = std::path::Path::new("..").join(outside.path().file_name().unwrap()).join("secret.md");
    assert!(read_prompt_file(&escape, t.path()).is_err());
}
```

(The `../` case may need the two tempdirs to share a parent — construct with `tempfile::TempDir::new_in` if the default layout doesn't guarantee it.)

- [ ] **Step 2: Run to verify failure** — `... cargo nextest run -p tau-pkg read_prompt_file`. Expected: FAIL (absolute + escape currently succeed).

- [ ] **Step 3: Implement** — replace the body:

```rust
pub fn read_prompt_file(
    rel: &std::path::Path,
    project_root: &std::path::Path,
) -> Result<Vec<u8>, std::io::Error> {
    let deny = |msg: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, msg.to_string());
    if rel.is_absolute() {
        return Err(deny("prompt file path must be relative to the project root"));
    }
    let abs = project_root.join(rel);
    let canon = abs.canonicalize()?;
    let root = project_root.canonicalize()?;
    if !canon.starts_with(&root) {
        return Err(deny("prompt file path escapes the project root"));
    }
    std::fs::read(&canon)
}
```

- [ ] **Step 4: Run tests, expect PASS**; then full `-p tau-pkg` AND `... cargo nextest run -p tau-cli` (build/run paths use this fn — existing fixtures with legit relative prompts must stay green).

- [ ] **Step 5: fmt + commit** — `fix(pkg): contain read_prompt_file to the project root`.

---

### Task 8: MCP contract pins nest for `/`-names

**Files:**
- Modify: `crates/tau-cli/src/cmd/build.rs` (pin write ~line 322-341; extract a helper)
- Test: inline test in build.rs

**Interfaces:**
- Produces: `fn contract_pin_path(pin_base: &Path, entry: &str) -> PathBuf` used by both the read (~line 288-301) and write (~line 325) sites.

- [ ] **Step 1: Write failing test**:

```rust
#[test]
fn contract_pin_path_nests_slash_names() {
    let base = std::path::Path::new(".tau/mcp");
    assert_eq!(
        contract_pin_path(base, "github/search"),
        std::path::Path::new(".tau/mcp/github/search.contract.json")
    );
    assert_eq!(
        contract_pin_path(base, "plain"),
        std::path::Path::new(".tau/mcp/plain.contract.json")
    );
}
```

- [ ] **Step 2: Run to verify failure** — `... cargo nextest run -p tau-cli contract_pin_path`. Expected: compile FAIL.

- [ ] **Step 3: Implement**:

```rust
/// Pin path for an MCP tool entry. Path-named tools (`github/search`) nest —
/// safe against a sibling `github.contract.json` because the file name always
/// carries the `.contract.json` suffix.
fn contract_pin_path(pin_base: &std::path::Path, entry: &str) -> std::path::PathBuf {
    pin_base.join(format!("{entry}.contract.json"))
}
```

Swap both `format!(".tau/mcp/{entry}.contract.json")`-style sites to use it, and before the write add `if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }` (replacing/augmenting the existing flat `create_dir_all(pin_base)` at ~line 322). Keep the user-facing pin-path strings in messages (lines ~301/341) rendered from the helper's output via `.display()` so copy stays consistent.

- [ ] **Step 4: Run tests, expect PASS**; then full `-p tau-cli` check: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli`.

- [ ] **Step 5: fmt + commit** — `fix(cli): nest MCP contract pins for path-named tools`.

---

### Task 9: `tau dev` watches `[dirs]` roots

**Files:**
- Modify: `crates/tau-cli/src/cmd/dev/watcher.rs` (`spawn` + `resolve_watch_paths` + the canonicalized-set filter, lines ~33-60; read the whole file first)
- Test: extend the file's existing tests of `resolve_watch_paths` (locate with `rg -n "resolve_watch_paths" crates/tau-cli/src/cmd/dev/watcher.rs`)

**Interfaces:**
- Consumes: `ProjectConfig.dirs: Option<DirsEntry>` (Task 1).
- Produces: watcher behavior only — dir roots watched with `RecursiveMode::Recursive`; the event filter accepts any path under a watched dir root.

- [ ] **Step 1: Write failing test** (mirror the existing `resolve_watch_paths` test style; if none exists, add one):

```rust
#[test]
fn watch_paths_include_dirs_roots() {
    // Build a minimal ProjectConfig via ProjectConfig::from_path on a tempdir
    // project with `[dirs] agents = "agents"` (empty agents/ dir suffices).
    // Assert the resolved dir-watch list contains `<root>/agents`.
}
```

Concretely: split `resolve_watch_paths` into `(files: Vec<PathBuf>, dirs: Vec<PathBuf>)` (or add a sibling `resolve_watch_dirs(project_root, project) -> Vec<PathBuf>`), and assert `resolve_watch_dirs(root, &project) == vec![root.join("agents")]`.

- [ ] **Step 2: Run to verify failure** — `... cargo nextest run -p tau-cli watcher`. Expected: compile FAIL.

- [ ] **Step 3: Implement**:

```rust
/// `[dirs]` roots to watch recursively (empty when the project has none).
fn resolve_watch_dirs(project_root: &Path, project: &ProjectConfig) -> Vec<PathBuf> {
    let Some(dirs) = &project.dirs else { return Vec::new() };
    [dirs.agents.as_ref(), dirs.tools.as_ref()]
        .into_iter()
        .flatten()
        .map(|rel| project_root.join(rel))
        .collect()
}
```

In `spawn`: watch each with `RecursiveMode::Recursive`; extend the filter — alongside the existing canonicalized `watched: HashSet<PathBuf>` file-set, keep `watched_dirs: Vec<PathBuf>` (canonicalized) and treat an event path `p` as relevant when `watched.contains(p) || watched_dirs.iter().any(|d| p.starts_with(d))`. Follow the existing canonicalize-with-fallback pattern (watcher.rs:48-50).

- [ ] **Step 4: Run tests, expect PASS**; `cargo check -p tau-cli` per the timeout template.

- [ ] **Step 5: fmt + commit** — `feat(cli): tau dev watches [dirs] roots recursively`.

---

### Task 10: `tau check` dirs category (gitignored-definition lint)

**Files:**
- Create: `crates/tau-cli/src/cmd/check/categories/dirs.rs`
- Modify: `crates/tau-cli/src/cmd/check/categories/mod.rs` (add `pub mod dirs;`)
- Modify: `crates/tau-cli/src/cmd/check/result.rs` (add `CheckCategory::Dirs` — locate the enum with `rg -n "enum CheckCategory" crates/tau-cli/src/cmd/check/result.rs`; update its Display/serialize arms and any exhaustive matches the compiler flags)
- Modify: the runner that dispatches categories (find with `rg -n "run_config" crates/tau-cli/src/cmd/check/` — add the `run_dirs` call beside it)
- Test: inline tests in `dirs.rs`

**Interfaces:**
- Consumes: `tau_pkg::project::dirs::definition_files` (Task 3), `ProjectConfig::from_path` (Task 4). Model the whole file on `categories/config.rs` (`run_config(ctx: &CheckCtx) -> CheckResult`, `CheckFinding`/`Severity`/`FindingLocation` construction).
- Produces: `pub fn run_dirs(ctx: &CheckCtx) -> CheckResult` — `Ok` with no findings when the project has no `[dirs]`, git is absent, or nothing is ignored; one `Severity::Warning` finding per gitignored definition file.

- [ ] **Step 1: Write failing test** for the pure helper:

```rust
#[test]
fn gitignored_definitions_flagged() {
    let t = tempfile::TempDir::new().unwrap();
    let ok = std::process::Command::new("git").arg("-C").arg(t.path()).arg("init").arg("-q").status();
    if !ok.map(|s| s.success()).unwrap_or(false) {
        eprintln!("SKIP: git unavailable");
        return;
    }
    std::fs::write(t.path().join(".gitignore"), "agents/scratch.md\n").unwrap();
    std::fs::create_dir_all(t.path().join("agents")).unwrap();
    let files = [
        std::path::PathBuf::from("agents/scratch.md"),
        std::path::PathBuf::from("agents/kept.md"),
    ];
    let ignored = gitignored_files(t.path(), &files);
    assert_eq!(ignored, vec![std::path::PathBuf::from("agents/scratch.md")]);
}
```

- [ ] **Step 2: Run to verify failure** — `... cargo nextest run -p tau-cli gitignored_definitions`. Expected: compile FAIL.

- [ ] **Step 3: Implement** `dirs.rs`:

```rust
/// Which of `files` (project-root-relative) are gitignored. Empty when git
/// is unavailable or the root is not a repository (lint silently skips).
fn gitignored_files(project_root: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    use std::io::Write;
    let mut child = match std::process::Command::new("git")
        .arg("-C").arg(project_root)
        .args(["check-ignore", "--stdin", "-z"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let input: Vec<u8> = files.iter()
        .flat_map(|f| f.to_string_lossy().into_owned().into_bytes().into_iter().chain([0u8]))
        .collect();
    if child.stdin.take().and_then(|mut s| s.write_all(&input).ok()).is_none() {
        return Vec::new();
    }
    let out = match child.wait_with_output() { Ok(o) => o, Err(_) => return Vec::new() };
    // exit 0 = some ignored, 1 = none, 128 = not a repo / error → skip
    out.stdout.split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(String::from_utf8_lossy(s).into_owned()))
        .collect()
}
```

`run_dirs`: load via `ProjectConfig::from_path(ctx.project_root.join("tau.toml"))`; on parse error return `Ok` with no findings (the `config` category owns parse errors — do not double-report); if `cfg.dirs` is `None` → `Ok`. Otherwise `definition_files(...)` → `gitignored_files(...)` → one Warning finding per hit ("definition file is gitignored: builds locally but is absent from clones/CI"), status `Ok` unless the runner convention says warnings flip status — mirror what other warning-emitting categories do (check `packages.rs`/`skills.rs` for precedent). Wire `CheckCategory::Dirs` + runner registration; the compiler's exhaustive-match errors are the checklist.

- [ ] **Step 4: Run tests, expect PASS**; full `-p tau-cli` nextest for snapshot fallout (check snapshots under `crates/tau-cli/src/cmd/snapshots/` — if `tau check` output snapshots enumerate categories, update them via the crate's snapshot workflow).

- [ ] **Step 5: fmt + commit** — `feat(cli): tau check dirs category — gitignored definition lint`.

---

### Task 11: docs — how-to, ADR-0067, SUMMARY

**Files:**
- Create: `docs/how-to/define-agents-and-tools-in-directories.md`
- Create: `docs/decisions/0067-directory-based-definitions.md`
- Modify: `docs/SUMMARY.md` (one line in "How-to" list; one line in the decisions section — read SUMMARY to find where 0066 is listed and append 0067 after it)

**Interfaces:** none (docs only).

- [ ] **Step 1: Write the how-to.** Sections: opt-in (`[dirs]` snippet), naming (`path = name`, the 4-file example tree from the spec with resulting names), md file anatomy (frontmatter = `[agents.X]` fields with `display_name`/`package` required; body = system prompt; `name`/`prompt` forbidden), toml entry files, hygiene rules (`_`/`.` escape, strict errors, charset `[a-z0-9_-]`), collisions, and a "gotchas" list (moving = renaming; gitignored files lint; symlinks rejected). Reuse the spec's examples verbatim so docs and spec cannot drift apart.

- [ ] **Step 2: Write ADR-0067.** Follow the structure of `docs/decisions/0066-guest-fs-effect-descriptor-resolution.md` (read it first). Context: dir-based DX (Claude Code/Cursor precedent), tau's byte-equal-IR + governance invariants. Decision: explicit `[dirs]`, path=name with `/`, YAML frontmatter, unchecked-level merge in `parse_str_at`, strict hygiene, CRLF normalization, containment guard. Consequences: names may contain `/` (quoted TOML keys), moving files renames definitions, TS `dirs()` factory deferred. Link the spec.

- [ ] **Step 3: Build the book** (per DOCS RULES; from `docs/`):

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build
```

Expected: only `[INFO]` lines. Then `rm -rf docs/book`.

- [ ] **Step 4: Commit** — `docs: dir-based definitions how-to + ADR-0067`.

---

## Final verification (after all tasks)

- [ ] `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
- [ ] `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-lower`
- [ ] `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli`
- [ ] `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-sdk-codegen`
- [ ] `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-pkg -p tau-cli -p tau-ir-lower -p tau-sdk-codegen`
- [ ] `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-pkg -p tau-cli -p tau-ir-lower -p tau-sdk-codegen` (CI treats fmt as a separate gate)
