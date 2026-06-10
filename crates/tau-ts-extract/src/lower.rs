//! AST literal → ProjectConfig field mapping.
//!
//! Strategy: walk the NameMap, extract factory calls into an intermediate
//! representation, then serialize to TOML and call `ProjectConfig::parse_str`.
//! This means TS-derived configs go through the SAME validation path as
//! TOML-derived ones — no duplicated logic, and `#[non_exhaustive]` structs
//! are never constructed from outside tau-pkg.

use std::collections::BTreeMap;
use std::path::Path;

use swc_common::{sync::Lrc, SourceMap, Spanned};
use swc_ecma_ast::{
    Expr, KeyValueProp, Lit, Module, ModuleDecl, ModuleItem, ObjectLit, Prop, PropName,
    PropOrSpread,
};

use tau_pkg::project::project::ProjectConfig;

use crate::error::{Position, TsExtractError};
use crate::factory::{arg_as_object, arg_as_string, recognize_factory_call, Factory};
use crate::scope::NameMap;

// ──────────────────────────────────────────────────────────────────────────────
// Intermediate representation (owned strings only — TOML serializable)
// ──────────────────────────────────────────────────────────────────────────────

struct IrAgent {
    display_name: String,
    package: String,
    llm_backend: String,
    model: Option<String>,
    prompt_system: Option<String>,
    tool_refs: Vec<String>,
}

enum IrToolBody {
    Native(String),
    Mcp(String),
}

struct IrTool {
    body: IrToolBody,
    description: String,
}

// ──────────────────────────────────────────────────────────────────────────────
// Public entry point
// ──────────────────────────────────────────────────────────────────────────────

/// Build a `ProjectConfig` from a parsed module + name map.
pub fn build_project_config(
    module: &Module,
    names: &NameMap,
    source_path: &Path,
    sm: &Lrc<SourceMap>,
) -> Result<ProjectConfig, TsExtractError> {
    let project_name = source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_owned();

    // Walk imports: reject any import not from "tau".
    for item in &module.body {
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import_decl)) = item {
            let source_str = import_decl
                .src
                .value
                .as_wtf8()
                .to_string_lossy()
                .into_owned();
            if source_str != "tau" {
                let pos = TsExtractError::position_from_span(
                    sm,
                    import_decl.span,
                    source_path.to_path_buf(),
                );
                return Err(TsExtractError::ImportNotSupported {
                    pos,
                    import_source: source_str,
                });
            }
        }
    }

    let mut agents: BTreeMap<String, IrAgent> = BTreeMap::new();
    let mut tools: BTreeMap<String, IrTool> = BTreeMap::new();

    // First pass — collect tools/mcp (so agent `tools: { ref }` can resolve them).
    for (name, expr) in names {
        let resolved = resolve_ref(expr, names);
        if let Some((factory, call)) = recognize_factory_call(resolved) {
            match factory {
                Factory::Tool => {
                    let obj = arg_as_object(call, 0).ok_or_else(|| {
                        mk_err(
                            source_path,
                            sm,
                            call.span(),
                            &format!("`tool({name})`: first argument must be an object literal"),
                        )
                    })?;
                    let tool = extract_tool(name, obj, source_path, sm)?;
                    tools.insert(name.clone(), tool);
                }
                Factory::Mcp => {
                    let url = arg_as_string(call, 0).ok_or_else(|| {
                        mk_err(
                            source_path,
                            sm,
                            call.span(),
                            &format!("`mcp({name})`: first argument must be a string URL"),
                        )
                    })?;
                    tools.insert(
                        name.clone(),
                        IrTool {
                            body: IrToolBody::Mcp(url),
                            description: String::new(),
                        },
                    );
                }
                Factory::ContextManager => {
                    let pos = TsExtractError::position_from_span(
                        sm,
                        call.span(),
                        source_path.to_path_buf(),
                    );
                    return Err(TsExtractError::Deferred {
                        pos,
                        factory: "contextManager".to_string(),
                        until: "β.4".to_string(),
                    });
                }
                Factory::Agent => {} // second pass
            }
        }
    }

    // Second pass — collect agents.
    for (name, expr) in names {
        if let Some((Factory::Agent, call)) = recognize_factory_call(expr) {
            let obj = arg_as_object(call, 0).ok_or_else(|| {
                mk_err(
                    source_path,
                    sm,
                    call.span(),
                    &format!("`agent({name})`: first argument must be an object literal"),
                )
            })?;
            let (agent, extra_tools) = extract_agent(name, obj, names, source_path, sm)?;
            agents.insert(name.clone(), agent);
            for (tool_name, tool) in extra_tools {
                tools.entry(tool_name).or_insert(tool);
            }
        }
    }

    // Serialize to TOML and parse through the standard validation path.
    let toml = build_toml(&project_name, &agents, &tools);
    ProjectConfig::parse_str(&toml).map_err(|e| map_config_err(e, source_path, sm))
}

// ──────────────────────────────────────────────────────────────────────────────
// TOML builder
// ──────────────────────────────────────────────────────────────────────────────

fn build_toml(
    project_name: &str,
    agents: &BTreeMap<String, IrAgent>,
    tools: &BTreeMap<String, IrTool>,
) -> String {
    let mut out = String::new();

    out.push_str("[project]\n");
    out.push_str(&format!("name = {}\n\n", toml_str(project_name)));

    for (name, tool) in tools {
        out.push_str(&format!("[tools.{}]\n", toml_key(name)));
        match &tool.body {
            IrToolBody::Native(fn_name) => {
                out.push_str(&format!("native = {}\n", toml_str(fn_name)));
            }
            IrToolBody::Mcp(url) => {
                out.push_str(&format!("mcp = {}\n", toml_str(url)));
            }
        }
        if !tool.description.is_empty() {
            out.push_str(&format!("description = {}\n", toml_str(&tool.description)));
        }
        out.push('\n');
    }

    for (name, agent) in agents {
        out.push_str(&format!("[agents.{}]\n", toml_key(name)));
        out.push_str(&format!(
            "display_name = {}\n",
            toml_str(&agent.display_name)
        ));
        out.push_str(&format!("package = {}\n", toml_str(&agent.package)));
        out.push_str(&format!("llm_backend = {}\n", toml_str(&agent.llm_backend)));
        if let Some(model) = &agent.model {
            out.push_str(&format!("model = {}\n", toml_str(model)));
        }
        if !agent.tool_refs.is_empty() {
            let refs: Vec<String> = agent.tool_refs.iter().map(|r| toml_str(r)).collect();
            out.push_str(&format!("tool_refs = [{}]\n", refs.join(", ")));
        }
        if let Some(sys) = &agent.prompt_system {
            out.push_str(&format!("[agents.{}.prompt]\n", toml_key(name)));
            out.push_str(&format!("system = {}\n", toml_str(sys)));
        }
        out.push('\n');
    }

    out
}

/// Escape a Rust string for use as a TOML quoted string value.
fn toml_str(s: &str) -> String {
    // Use TOML literal strings (single-quoted) for simplicity when no single-quotes.
    // Otherwise fall back to basic strings with backslash escapes.
    if !s.contains('\'') {
        format!("'{s}'")
    } else {
        // Basic TOML string: escape backslashes and double-quotes.
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    }
}

/// Produce a TOML bare key or quoted key for an identifier.
fn toml_key(s: &str) -> String {
    // TOML bare keys: A-Za-z0-9_-
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        s.to_owned()
    } else {
        toml_str(s)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Object literal helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Walk an `ObjectLit` and return key→Expr pairs (KeyValue only).
fn obj_props(obj: &ObjectLit) -> BTreeMap<String, &Expr> {
    let mut map = BTreeMap::new();
    for prop in &obj.props {
        if let PropOrSpread::Prop(p) = prop {
            if let Prop::KeyValue(KeyValueProp { key, value }) = p.as_ref() {
                let key_str = match key {
                    PropName::Ident(i) => i.sym.to_string(),
                    PropName::Str(s) => s.value.as_wtf8().to_string_lossy().into_owned(),
                    _ => continue,
                };
                map.insert(key_str, value.as_ref());
            }
        }
    }
    map
}

/// Walk an `ObjectLit` returning (key, Option<Expr>) — None means shorthand.
fn obj_props_with_shorthands(obj: &ObjectLit) -> Vec<(String, Option<&Expr>)> {
    let mut out = Vec::new();
    for prop in &obj.props {
        if let PropOrSpread::Prop(p) = prop {
            match p.as_ref() {
                Prop::KeyValue(KeyValueProp { key, value }) => {
                    let key_str = match key {
                        PropName::Ident(i) => i.sym.to_string(),
                        PropName::Str(s) => s.value.as_wtf8().to_string_lossy().into_owned(),
                        _ => continue,
                    };
                    out.push((key_str, Some(value.as_ref())));
                }
                Prop::Shorthand(ident) => {
                    out.push((ident.sym.to_string(), None));
                }
                _ => {}
            }
        }
    }
    out
}

/// Try to get a string literal from the prop map.
fn get_string(props: &BTreeMap<String, &Expr>, key: &str) -> Option<String> {
    let expr = props.get(key)?;
    expr_as_string(expr)
}

/// Extract a string value from an `Expr::Lit(Lit::Str(...))`.
///
/// Converts `Wtf8Atom` to `String` via `to_string_lossy` (surrogate pairs become
/// the Unicode replacement character, which won't appear in valid TS string literals).
fn expr_as_string(expr: &Expr) -> Option<String> {
    if let Expr::Lit(Lit::Str(s)) = expr {
        Some(s.value.as_wtf8().to_string_lossy().into_owned())
    } else {
        None
    }
}

/// Resolve one level of identifier indirection against the NameMap.
fn resolve_ref<'a>(expr: &'a Expr, names: &'a NameMap<'a>) -> &'a Expr {
    if let Expr::Ident(ident) = expr {
        if let Some(resolved) = names.get(ident.sym.as_ref()) {
            return resolved;
        }
    }
    expr
}

// ──────────────────────────────────────────────────────────────────────────────
// Factory extractors
// ──────────────────────────────────────────────────────────────────────────────

fn extract_tool(
    name: &str,
    obj: &ObjectLit,
    source_path: &Path,
    sm: &Lrc<SourceMap>,
) -> Result<IrTool, TsExtractError> {
    let props_kv = obj_props(obj);

    // Walk raw props to detect `run: <function>` — must happen before
    // checking `native` so we emit InlineToolBody rather than "missing native".
    for prop in &obj.props {
        if let PropOrSpread::Prop(p) = prop {
            if let Prop::KeyValue(KeyValueProp { key, value }) = p.as_ref() {
                let key_str = match key {
                    PropName::Ident(i) => i.sym.to_string(),
                    PropName::Str(s) => s.value.as_wtf8().to_string_lossy().into_owned(),
                    _ => continue,
                };
                if key_str == "run" && matches!(value.as_ref(), Expr::Arrow(_) | Expr::Fn(_)) {
                    let pos = TsExtractError::position_from_span(
                        sm,
                        value.span(),
                        source_path.to_path_buf(),
                    );
                    return Err(TsExtractError::InlineToolBody { pos });
                }
            }
        }
    }

    let body = if let Some(native_val) = props_kv.get("native") {
        let fn_name = expr_as_string(native_val).ok_or_else(|| {
            mk_err(
                source_path,
                sm,
                native_val.span(),
                &format!("tool({name}): `native` field must be a string"),
            )
        })?;
        IrToolBody::Native(fn_name)
    } else {
        return Err(mk_err(
            source_path,
            sm,
            obj.span(),
            &format!("tool({name}): must have a `native` field (use mcp() factory for MCP tools)"),
        ));
    };

    let description = get_string(&props_kv, "description").unwrap_or_default();

    Ok(IrTool { body, description })
}

fn extract_agent(
    name: &str,
    obj: &ObjectLit,
    names: &NameMap,
    source_path: &Path,
    sm: &Lrc<SourceMap>,
) -> Result<(IrAgent, BTreeMap<String, IrTool>), TsExtractError> {
    let props = obj_props(obj);

    let display_name = get_string(&props, "display_name").ok_or_else(|| {
        mk_err(
            source_path,
            sm,
            obj.span(),
            &format!("agent({name}): missing required string field `display_name`"),
        )
    })?;
    let package = get_string(&props, "package").ok_or_else(|| {
        mk_err(
            source_path,
            sm,
            obj.span(),
            &format!("agent({name}): missing required string field `package`"),
        )
    })?;
    let llm_backend = get_string(&props, "llm_backend").ok_or_else(|| {
        mk_err(
            source_path,
            sm,
            obj.span(),
            &format!("agent({name}): missing required string field `llm_backend`"),
        )
    })?;
    let model = get_string(&props, "model");

    let prompt_system = if let Some(Expr::Object(prompt_obj)) = props.get("prompt").copied() {
        let pp = obj_props(prompt_obj);
        get_string(&pp, "system")
    } else {
        None
    };

    let mut tool_refs: Vec<String> = Vec::new();
    let mut extra_tools: BTreeMap<String, IrTool> = BTreeMap::new();

    if let Some(Expr::Object(tools_obj)) = props.get("tools").copied() {
        for (tool_name, maybe_expr) in obj_props_with_shorthands(tools_obj) {
            tool_refs.push(tool_name.clone());

            let resolved: Option<&Expr> = match maybe_expr {
                Some(expr) => Some(resolve_ref(expr, names)),
                None => names.get(&tool_name).copied(),
            };

            if let Some(resolved_expr) = resolved {
                if let Some((factory, call)) = recognize_factory_call(resolved_expr) {
                    match factory {
                        Factory::Tool => {
                            if let Some(obj) = arg_as_object(call, 0) {
                                let tool = extract_tool(&tool_name, obj, source_path, sm)?;
                                extra_tools.insert(tool_name, tool);
                            }
                        }
                        Factory::Mcp => {
                            if let Some(url) = arg_as_string(call, 0) {
                                extra_tools.insert(
                                    tool_name,
                                    IrTool {
                                        body: IrToolBody::Mcp(url),
                                        description: String::new(),
                                    },
                                );
                            }
                        }
                        Factory::ContextManager => {
                            let pos = TsExtractError::position_from_span(
                                sm,
                                call.span(),
                                source_path.to_path_buf(),
                            );
                            return Err(TsExtractError::Deferred {
                                pos,
                                factory: "contextManager".to_string(),
                                until: "β.4".to_string(),
                            });
                        }
                        Factory::Agent => {}
                    }
                }
            }
        }
    }

    let agent = IrAgent {
        display_name,
        package,
        llm_backend,
        model,
        prompt_system,
        tool_refs,
    };

    Ok((agent, extra_tools))
}

// ──────────────────────────────────────────────────────────────────────────────
// Error helpers
// ──────────────────────────────────────────────────────────────────────────────

fn mk_err(
    path: &Path,
    sm: &Lrc<SourceMap>,
    span: swc_common::Span,
    message: &str,
) -> TsExtractError {
    let pos = TsExtractError::position_from_span(sm, span, path.to_path_buf());
    TsExtractError::ParseError {
        pos,
        message: message.to_owned(),
    }
}

fn map_config_err(
    e: tau_pkg::project::project::ProjectConfigError,
    path: &Path,
    sm: &Lrc<SourceMap>,
) -> TsExtractError {
    // No useful span available — use dummy span at byte 0.
    let pos = Position {
        file: path.to_path_buf(),
        line: 0,
        col: 0,
    };
    let _ = sm; // kept for API symmetry; no span to resolve
    TsExtractError::ParseError {
        pos,
        message: format!("project validation failed: {e}"),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::extract_project;
    use std::path::Path;

    #[test]
    fn parses_minimal_agent_export() {
        let src = r#"
            export const fanMonitor = agent({
                display_name: "Fan Monitor",
                package: "fan-monitor@^0.1",
                llm_backend: "anthropic",
                model: "claude-haiku-4-5",
                prompt: { system: "Watch the temperature." }
            });
        "#;
        let config = extract_project(src, Path::new("/tmp/p.ts")).expect("parse");
        assert!(
            config.agents.contains_key("fanMonitor"),
            "expected fanMonitor in: {:?}",
            config.agents.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn resolves_top_level_constant_reference() {
        let src = r#"
            const readTemp = tool({
                native: "ReadTemp",
                description: "Read temperature"
            });
            export const a = agent({
                display_name: "A",
                package: "a@^0.1",
                llm_backend: "anthropic",
                model: "claude-haiku-4-5",
                prompt: { system: "x" },
                tools: { readTemp }
            });
        "#;
        let config = extract_project(src, Path::new("/tmp/p.ts")).expect("parse");
        assert!(
            config.tools.contains_key("readTemp"),
            "got tools: {:?}",
            config.tools.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn recognizes_mcp_factory() {
        let src = r#"
            const weather = mcp("https://mcp.weather.com");
            export const a = agent({
                display_name: "A",
                package: "a@^0.1",
                llm_backend: "anthropic",
                model: "claude-haiku-4-5",
                prompt: { system: "x" },
                tools: { weather }
            });
        "#;
        let config = extract_project(src, Path::new("/tmp/p.ts")).expect("parse");
        let weather = config.tools.get("weather").expect("weather tool");
        match &weather.body {
            tau_pkg::project::project::ToolBody::Mcp(url) => {
                assert_eq!(url, "https://mcp.weather.com");
            }
            other => panic!("expected ToolBody::Mcp, got {other:?}"),
        }
    }

    #[test]
    fn agent_with_no_tools_field_works() {
        let src = r#"
            export const solo = agent({
                display_name: "Solo",
                package: "solo@^0.1",
                llm_backend: "anthropic",
                model: "claude-haiku-4-5",
                prompt: { system: "alone" }
            });
        "#;
        let config = extract_project(src, Path::new("/tmp/p.ts")).expect("parse");
        assert!(config.agents.contains_key("solo"));
    }

    // ── Phase 4 rejection tests ──────────────────────────────────────────────

    #[test]
    fn rejects_async_function_body() {
        let src = r#"
            const t = tool({
                native: "X",
                run: async () => 42
            });
            export const a = agent({
                display_name: "A",
                package: "a@^0.1",
                llm_backend: "anthropic",
                model: "x",
                prompt: { system: "x" },
                tools: { t }
            });
        "#;
        let err = crate::extract_project(src, std::path::Path::new("/tmp/t.ts"))
            .expect_err("should fail");
        assert!(
            matches!(
                err,
                crate::error::TsExtractError::InlineToolBody { .. }
                    | crate::error::TsExtractError::UnsupportedExpression { .. }
            ),
            "expected InlineToolBody or UnsupportedExpression, got: {err:?}"
        );
    }

    #[test]
    fn rejects_context_manager_factory() {
        let src = r#"
            export const ctx = contextManager({
                budget: { tokens: 16000 }
            });
        "#;
        let err = crate::extract_project(src, std::path::Path::new("/tmp/t.ts"))
            .expect_err("should fail");
        assert!(
            matches!(err, crate::error::TsExtractError::Deferred { .. }),
            "expected Deferred, got: {err:?}"
        );
    }

    #[test]
    fn rejects_non_tau_import() {
        let src = r#"
            import { x } from "./helpers";
            export const a = agent({
                display_name: "A",
                package: "a@^0.1",
                llm_backend: "anthropic",
                model: "x",
                prompt: { system: "x" }
            });
        "#;
        let err = crate::extract_project(src, std::path::Path::new("/tmp/t.ts"))
            .expect_err("should fail");
        assert!(
            matches!(err, crate::error::TsExtractError::ImportNotSupported { .. }),
            "expected ImportNotSupported, got: {err:?}"
        );
    }

    #[test]
    fn error_position_carries_line_col() {
        let src = "const broken = ();"; // syntax error
        let err = crate::extract_project(src, std::path::Path::new("/tmp/t.ts"))
            .expect_err("should fail");
        match err {
            crate::error::TsExtractError::ParseError { pos, .. } => {
                assert_eq!(pos.line, 1, "expected line 1, got {}", pos.line);
                assert!(pos.col > 0, "expected non-zero col, got {}", pos.col);
            }
            other => panic!("expected ParseError, got: {other:?}"),
        }
    }
}
