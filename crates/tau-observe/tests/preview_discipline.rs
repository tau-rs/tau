//! CI guard: every call to `preview::full` / `preview::full_json` must
//! be inside a `tracing::trace!` or `trace_span!` invocation. We can't
//! enforce this with the type system (the helpers return `impl Display`
//! and tracing's macros don't know their semantics), so a grep-style
//! lint runs in CI instead.
//!
//! False positives can be silenced by adding the comment
//! `// tau_observe_preview_full_allowed` on the same line.

use std::fs;
use std::path::PathBuf;

#[test]
fn no_full_helper_at_non_trace_callsite() {
    let workspace_root = workspace_root();
    let mut violations = Vec::new();
    walk(&workspace_root.join("crates"), &mut |path, contents| {
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            return;
        }
        // Skip this discipline test itself — the string literals below
        // would otherwise trigger the lint.
        if path.file_name().and_then(|n| n.to_str()) == Some("preview_discipline.rs") {
            return;
        }
        let mut last_macro: Option<(String, usize)> = None;
        for (line_no, line) in contents.lines().enumerate() {
            if let Some(m) = find_tracing_macro(line) {
                last_macro = Some((m, line_no));
            }
            if (line.contains("preview::full(") || line.contains("preview::full_json("))
                && !line.contains("tau_observe_preview_full_allowed")
            {
                let in_trace_window = match &last_macro {
                    Some((m, ln)) if line_no.saturating_sub(*ln) < 5 => {
                        m == "trace" || m == "trace_span"
                    }
                    _ => false,
                };
                if !in_trace_window {
                    violations.push(format!(
                        "{}:{}: {}",
                        path.display(),
                        line_no + 1,
                        line.trim()
                    ));
                }
            }
        }
    });
    assert!(
        violations.is_empty(),
        "preview::full* used at non-trace call sites:\n{}",
        violations.join("\n")
    );
}

fn find_tracing_macro(line: &str) -> Option<String> {
    for macro_name in ["trace", "debug", "info", "warn", "error"] {
        let needles = [
            format!("tracing::{macro_name}!"),
            format!("{macro_name}!("),
            format!("{macro_name}_span!"),
            format!("tracing::{macro_name}_span!"),
        ];
        for needle in &needles {
            if line.contains(needle) {
                if needle.contains("span") {
                    return Some(format!("{macro_name}_span"));
                }
                return Some(macro_name.to_string());
            }
        }
    }
    None
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `crates/tau-observe`; go up two levels.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn walk(dir: &std::path::Path, cb: &mut dyn FnMut(&std::path::Path, &str)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip target/ to avoid scanning build artifacts.
            if path.file_name().and_then(|n| n.to_str()) == Some("target") {
                continue;
            }
            walk(&path, cb);
        } else if let Ok(contents) = fs::read_to_string(&path) {
            cb(&path, &contents);
        }
    }
}
