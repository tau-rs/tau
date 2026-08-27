//! Recursive, deterministic scan of `[dirs]` roots (spec: strict hygiene).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::super::project::{
    validate_dirs_decl, DirsEntry, ProjectConfigError, UncheckedAgent, UncheckedDirs, UncheckedTool,
};
use super::file;

/// OS-generated files that are silently skipped rather than rejected —
/// they are not authored by a human placing a definition, so strict
/// hygiene (extension/charset checks) would just be noise.
const OS_JUNK: &[&str] = &["Thumbs.db", "desktop.ini"];

/// Definitions harvested from `[dirs]` roots. Paths are project-root-relative.
#[derive(Debug)]
pub struct ScannedDefs {
    /// Full path-name → (entry, source file).
    pub agents: BTreeMap<String, (UncheckedAgent, PathBuf)>,
    /// Full path-name → (entry, source file).
    pub tools: BTreeMap<String, (UncheckedTool, PathBuf)>,
}

/// Which `[dirs]` root is being walked; determines the accepted file
/// extensions and which parser/output map an entry feeds.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Agents,
    Tools,
}

/// Scan the `[dirs]` roots declared in `dirs` (relative to `project_root`)
/// and return every definition found, keyed by its derived path-name.
///
/// Validates root declarations (existence, containment, non-overlap) before
/// walking, then walks each declared root recursively enforcing strict
/// hygiene (no symlinks, `[a-z0-9_-]+` segments, `.md`/`.toml` only).
pub fn scan_dirs(
    project_root: &Path,
    dirs: &UncheckedDirs,
) -> Result<ScannedDefs, ProjectConfigError> {
    let agents_root = dirs
        .agents
        .as_deref()
        .map(|d| resolve_root(project_root, "agents", d))
        .transpose()?;
    let tools_root = dirs
        .tools
        .as_deref()
        .map(|d| resolve_root(project_root, "tools", d))
        .transpose()?;
    if let (Some((_, a)), Some((_, t))) = (&agents_root, &tools_root) {
        if a.starts_with(t) || t.starts_with(a) {
            return Err(ProjectConfigError::DirsRootsOverlap {
                a: dirs.agents.clone().unwrap_or_default(),
                b: dirs.tools.clone().unwrap_or_default(),
            });
        }
    }
    let mut out = ScannedDefs {
        agents: BTreeMap::new(),
        tools: BTreeMap::new(),
    };
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
        agents: dirs
            .agents
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        tools: dirs
            .tools
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
    };
    let s = scan_dirs(project_root, &unchecked)?;
    Ok(s.agents
        .values()
        .map(|(_, p)| p.clone())
        .chain(s.tools.values().map(|(_, p)| p.clone()))
        .collect())
}

/// Syntactic checks + existence + symlink-free containment. Returns
/// `(declared relative path, absolute canonical path)`.
fn resolve_root(
    project_root: &Path,
    kind: &'static str,
    decl: &str,
) -> Result<(PathBuf, PathBuf), ProjectConfigError> {
    let rel = validate_dirs_decl(kind, decl)?;
    let err = |reason: String| ProjectConfigError::DirsRoot {
        kind,
        path: decl.to_string(),
        reason,
    };
    let abs = project_root.join(&rel);
    let canon = abs
        .canonicalize()
        .map_err(|_| err("directory does not exist".to_string()))?;
    let root_canon = project_root
        .canonicalize()
        .map_err(|e| err(format!("project root not canonicalizable: {e}")))?;
    if !canon.starts_with(&root_canon) {
        return Err(err("escapes the project root".to_string()));
    }
    if !canon.is_dir() {
        return Err(err("is not a directory".to_string()));
    }
    Ok((rel, canon))
}

/// `true` if `s` is non-empty and every character is `[a-z0-9_-]`.
fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Render `root_rel/segments…/leaf` as a `/`-joined display path for error
/// messages, regardless of host OS path separator. `leaf` may be empty
/// (directory-level errors), in which case no trailing separator is added.
fn display_path(root_rel: &Path, segments: &[String], leaf: &str) -> String {
    let root_str = root_rel.to_string_lossy().replace('\\', "/");
    let mut parts: Vec<&str> = Vec::with_capacity(segments.len() + 2);
    if !root_str.is_empty() {
        parts.push(root_str.as_str());
    }
    for s in segments {
        parts.push(s.as_str());
    }
    if !leaf.is_empty() {
        parts.push(leaf);
    }
    parts.join("/")
}

/// Recursively walk `dir_abs` (the directory at `segments` under
/// `root_rel`), collecting definitions into `out`. See the nine-step spec
/// in the task brief for the exact hygiene rules enforced per entry.
fn walk(
    kind: Kind,
    root_rel: &Path,
    dir_abs: &Path,
    segments: &mut Vec<String>,
    out: &mut ScannedDefs,
) -> Result<(), ProjectConfigError> {
    // Step 1: read_dir, collect, sort by file_name for determinism.
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir_abs)
        .map_err(|e| ProjectConfigError::DefFile {
            file: display_path(root_rel, segments, ""),
            reason: format!("cannot read directory: {e}"),
        })?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|e| ProjectConfigError::DefFile {
            file: display_path(root_rel, segments, ""),
            reason: format!("cannot read directory entry: {e}"),
        })?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        // Step 2: file name must be UTF-8.
        let os_name = entry.file_name();
        let Some(fname) = os_name.to_str() else {
            return Err(ProjectConfigError::DefFile {
                file: display_path(root_rel, segments, &entry.file_name().to_string_lossy()),
                reason: "non-UTF-8 file name".to_string(),
            });
        };

        // Step 3: skip hidden/underscore-prefixed/OS-junk entries.
        if fname.starts_with('.') || fname.starts_with('_') || OS_JUNK.contains(&fname) {
            continue;
        }

        // Step 4: reject symlinks (must use symlink_metadata, not metadata,
        // so we inspect the link itself rather than following it).
        let meta =
            std::fs::symlink_metadata(entry.path()).map_err(|e| ProjectConfigError::DefFile {
                file: display_path(root_rel, segments, fname),
                reason: format!("cannot stat: {e}"),
            })?;
        if meta.file_type().is_symlink() {
            return Err(ProjectConfigError::DefFile {
                file: display_path(root_rel, segments, fname),
                reason: "symlinks are not allowed in definition dirs".to_string(),
            });
        }

        if meta.is_dir() {
            // Step 5: directory — validate segment charset, recurse.
            if !valid_segment(fname) {
                return Err(ProjectConfigError::DefFile {
                    file: display_path(root_rel, segments, fname),
                    reason: "directory name must match [a-z0-9_-]+".to_string(),
                });
            }
            segments.push(fname.to_string());
            walk(kind, root_rel, &entry.path(), segments, out)?;
            segments.pop();
            continue;
        }

        // Step 6: file — dispatch on (kind, extension), validate stem charset.
        let file_disp = display_path(root_rel, segments, fname);
        let stem = if let (Kind::Agents, Some(stem)) = (kind, fname.strip_suffix(".md")) {
            if !valid_segment(stem) {
                return Err(ProjectConfigError::DefFile {
                    file: file_disp,
                    reason: "file stem must match [a-z0-9_-]+ (no dots, lowercase ASCII)"
                        .to_string(),
                });
            }
            stem
        } else if let Some(stem) = fname.strip_suffix(".toml") {
            if !valid_segment(stem) {
                return Err(ProjectConfigError::DefFile {
                    file: file_disp,
                    reason: "file stem must match [a-z0-9_-]+ (no dots, lowercase ASCII)"
                        .to_string(),
                });
            }
            stem
        } else {
            return Err(ProjectConfigError::DefFile {
                file: file_disp,
                reason: "not a definition (.md/.toml); prefix with `_` to ignore".to_string(),
            });
        };

        // Step 7: derive the path-name from segments (never from an OS
        // path string), so it is `/`-joined on every platform.
        let name = segments
            .iter()
            .cloned()
            .chain(std::iter::once(stem.to_string()))
            .collect::<Vec<_>>()
            .join("/");
        let file_rel = {
            let mut p = root_rel.to_path_buf();
            for s in segments.iter() {
                p.push(s);
            }
            p.push(fname);
            p
        };

        let raw =
            std::fs::read_to_string(entry.path()).map_err(|e| ProjectConfigError::DefFile {
                file: file_disp.clone(),
                reason: format!("cannot read file: {e}"),
            })?;

        match kind {
            Kind::Agents => {
                let agent = if fname.ends_with(".md") {
                    file::parse_agent_md(&raw)
                } else {
                    file::parse_agent_toml(&raw)
                }
                // Step 9: parser errors wrap as DefFile.
                .map_err(|reason| ProjectConfigError::DefFile {
                    file: file_disp.clone(),
                    reason,
                })?;
                // Step 8: insert, duplicate-name check.
                if out.agents.contains_key(&name) {
                    return Err(ProjectConfigError::DuplicateDefinition {
                        kind: "agent",
                        name,
                        file: file_disp,
                    });
                }
                out.agents.insert(name, (agent, file_rel));
            }
            Kind::Tools => {
                let tool =
                    file::parse_tool_toml(&raw).map_err(|reason| ProjectConfigError::DefFile {
                        file: file_disp.clone(),
                        reason,
                    })?;
                if out.tools.contains_key(&name) {
                    return Err(ProjectConfigError::DuplicateDefinition {
                        kind: "tool",
                        name,
                        file: file_disp,
                    });
                }
                out.tools.insert(name, (tool, file_rel));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT_MD: &str = "---\ndisplay_name: A\npackage: p@^1\n---\nprompt body\n";
    const TOOL_TOML: &str = "native = \"ReadTemp\"\n";

    fn dirs(agents: Option<&str>, tools: Option<&str>) -> UncheckedDirs {
        UncheckedDirs {
            agents: agents.map(String::from),
            tools: tools.map(String::from),
        }
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
        for bad in [
            "agents/Bad.md",
            "agents/sp ace.md",
            "agents/v1.2.md",
            "agents/Sub/ok.md",
        ] {
            let t = tempfile::TempDir::new().unwrap();
            write(t.path(), bad, AGENT_MD);
            assert!(
                scan_dirs(t.path(), &dirs(Some("agents"), None)).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn md_toml_same_stem_collides() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "agents/x.md", AGENT_MD);
        write(
            t.path(),
            "agents/x.toml",
            "display_name = \"A\"\npackage = \"p@^1\"\n",
        );
        let e = scan_dirs(t.path(), &dirs(Some("agents"), None)).unwrap_err();
        assert!(
            matches!(e, ProjectConfigError::DuplicateDefinition { .. }),
            "{e:?}"
        );
    }

    #[test]
    fn symlinks_rejected() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "agents/real.md", AGENT_MD);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                t.path().join("agents/real.md"),
                t.path().join("agents/link.md"),
            )
            .unwrap();
            assert!(scan_dirs(t.path(), &dirs(Some("agents"), None)).is_err());
        }
    }

    /// `definition_files` is the only entry point `tau check dirs` uses
    /// (Task 10); it had no direct coverage before this test — it was only
    /// exercised transitively via `scan_dirs`. Its only non-trivial logic is
    /// the `DirsEntry: Option<PathBuf>` → `String` round-trip via
    /// `to_string_lossy` that re-enters `validate_dirs_decl` through
    /// `UncheckedDirs`; this pins that both roots resolve to the expected
    /// project-root-relative paths.
    #[test]
    fn definition_files_returns_relative_paths_for_both_roots() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "agents/triage.md", AGENT_MD);
        write(t.path(), "agents/review/strict.md", AGENT_MD);
        write(t.path(), "tools/github/search.toml", TOOL_TOML);
        let dirs = DirsEntry {
            agents: Some(PathBuf::from("agents")),
            tools: Some(PathBuf::from("tools")),
        };
        let mut files = definition_files(t.path(), &dirs).unwrap();
        files.sort();
        assert_eq!(
            files,
            vec![
                PathBuf::from("agents/review/strict.md"),
                PathBuf::from("agents/triage.md"),
                PathBuf::from("tools/github/search.toml"),
            ]
        );
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
