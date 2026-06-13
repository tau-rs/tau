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
    ArrayLit, Expr, ExprOrSpread, KeyValueProp, Lit, Module, ModuleDecl, ModuleItem, ObjectLit,
    Prop, PropName, PropOrSpread,
};

use tau_pkg::project::project::ProjectConfig;

use crate::error::{Position, TsExtractError};
use crate::factory::{arg_as_array, arg_as_object, arg_as_string, recognize_factory_call, Factory};
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
    produces: Vec<String>,
}

enum IrToolBody {
    Native(String),
    Mcp(String),
}

/// A single capability declaration extracted from a tool object.
struct IrCapability {
    kind: String,
    paths: Vec<String>,
}

struct IrTool {
    body: IrToolBody,
    description: String,
    capabilities: Vec<IrCapability>,
}

struct IrPipelineStep {
    id: String,
    run: String,
    input: Option<String>,
}

/// A single goal extracted from `goals([...])`.
struct IrGoal {
    id: String,
    evaluates: String,
    /// Menu predicate name (e.g. `"matches"`, `"exists"`). None if `fn` is set.
    check: Option<String>,
    pattern: Option<String>,
    equals: Option<String>,
    min_count: Option<u64>,
    /// Native-fn escape hatch. None if `check` is set.
    fn_name: Option<String>,
}

/// A single deliverable extracted from `deliverables([...])`.
struct IrDeliverable {
    id: String,
    path: Option<String>,
    output: Option<String>,
    must_satisfy: String,
    on_fail: Option<String>,
    max_attempts: Option<u64>,
    retry_from: Option<String>,
    judge_model: Option<String>,
    judge: Option<String>,
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
    let mut pipeline_steps: Vec<IrPipelineStep> = Vec::new();
    let mut goals: Vec<IrGoal> = Vec::new();
    let mut deliverables: Vec<IrDeliverable> = Vec::new();

    // First pass — collect tools/mcp/goals/deliverables (so agent `tools: { ref }` can resolve them).
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
                            capabilities: Vec::new(),
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
                Factory::Pipeline => {
                    let arr = arg_as_array(call, 0).ok_or_else(|| {
                        mk_err(
                            source_path,
                            sm,
                            call.span(),
                            "`pipeline(...)`: first argument must be an array literal",
                        )
                    })?;
                    pipeline_steps = extract_pipeline_steps(arr, source_path, sm)?;
                }
                Factory::Goals => {
                    let arr = arg_as_array(call, 0).ok_or_else(|| {
                        mk_err(
                            source_path,
                            sm,
                            call.span(),
                            "`goals(...)`: first argument must be an array literal",
                        )
                    })?;
                    goals = extract_goals(arr, source_path, sm)?;
                }
                Factory::Deliverables => {
                    let arr = arg_as_array(call, 0).ok_or_else(|| {
                        mk_err(
                            source_path,
                            sm,
                            call.span(),
                            "`deliverables(...)`: first argument must be an array literal",
                        )
                    })?;
                    deliverables = extract_deliverables(arr, source_path, sm)?;
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
    let toml = build_toml(
        &project_name,
        &agents,
        &tools,
        &pipeline_steps,
        &goals,
        &deliverables,
    );
    ProjectConfig::parse_str(&toml).map_err(|e| map_config_err(e, source_path, sm))
}

// ──────────────────────────────────────────────────────────────────────────────
// TOML builder
// ──────────────────────────────────────────────────────────────────────────────

fn build_toml(
    project_name: &str,
    agents: &BTreeMap<String, IrAgent>,
    tools: &BTreeMap<String, IrTool>,
    pipeline_steps: &[IrPipelineStep],
    goals: &[IrGoal],
    deliverables: &[IrDeliverable],
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
        if !tool.capabilities.is_empty() {
            let cap_entries: Vec<String> = tool
                .capabilities
                .iter()
                .map(|cap| {
                    let paths: Vec<String> = cap.paths.iter().map(|p| toml_str(p)).collect();
                    format!(
                        "{{ kind = {}, paths = [{}] }}",
                        toml_str(&cap.kind),
                        paths.join(", ")
                    )
                })
                .collect();
            out.push_str(&format!("capabilities = [{}]\n", cap_entries.join(", ")));
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
        if !agent.produces.is_empty() {
            let prods: Vec<String> = agent.produces.iter().map(|p| toml_str(p)).collect();
            out.push_str(&format!("produces = [{}]\n", prods.join(", ")));
        }
        if let Some(sys) = &agent.prompt_system {
            out.push_str(&format!("[agents.{}.prompt]\n", toml_key(name)));
            out.push_str(&format!("system = {}\n", toml_str(sys)));
        }
        out.push('\n');
    }

    for step in pipeline_steps {
        out.push_str("[[pipeline.steps]]\n");
        out.push_str(&format!("id = {}\n", toml_str(&step.id)));
        out.push_str(&format!("run = {}\n", toml_str(&step.run)));
        if let Some(input) = &step.input {
            out.push_str(&format!("input = {}\n", toml_str(input)));
        }
        out.push('\n');
    }

    for goal in goals {
        out.push_str(&format!("[goals.{}]\n", toml_key(&goal.id)));
        out.push_str(&format!("evaluates = {}\n", toml_str(&goal.evaluates)));
        if let Some(check) = &goal.check {
            out.push_str(&format!("check = {}\n", toml_str(check)));
        }
        if let Some(pattern) = &goal.pattern {
            out.push_str(&format!("pattern = {}\n", toml_str(pattern)));
        }
        if let Some(equals) = &goal.equals {
            out.push_str(&format!("equals = {}\n", toml_str(equals)));
        }
        if let Some(min_count) = goal.min_count {
            out.push_str(&format!("min_count = {min_count}\n"));
        }
        if let Some(fn_name) = &goal.fn_name {
            out.push_str(&format!("fn = {}\n", toml_str(fn_name)));
        }
        out.push('\n');
    }

    for deliverable in deliverables {
        out.push_str(&format!("[deliverables.{}]\n", toml_key(&deliverable.id)));
        if let Some(path) = &deliverable.path {
            out.push_str(&format!("path = {}\n", toml_str(path)));
        }
        if let Some(output) = &deliverable.output {
            out.push_str(&format!("output = {}\n", toml_str(output)));
        }
        out.push_str(&format!(
            "must_satisfy = {}\n",
            toml_str(&deliverable.must_satisfy)
        ));
        if let Some(on_fail) = &deliverable.on_fail {
            out.push_str(&format!("on_fail = {}\n", toml_str(on_fail)));
        }
        if let Some(max_attempts) = deliverable.max_attempts {
            out.push_str(&format!("max_attempts = {max_attempts}\n"));
        }
        if let Some(retry_from) = &deliverable.retry_from {
            out.push_str(&format!("retry_from = {}\n", toml_str(retry_from)));
        }
        if let Some(judge_model) = &deliverable.judge_model {
            out.push_str(&format!("judge_model = {}\n", toml_str(judge_model)));
        }
        if let Some(judge) = &deliverable.judge {
            out.push_str(&format!("judge = {}\n", toml_str(judge)));
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

fn extract_pipeline_steps(
    arr: &ArrayLit,
    source_path: &Path,
    sm: &Lrc<SourceMap>,
) -> Result<Vec<IrPipelineStep>, TsExtractError> {
    let mut steps = Vec::new();
    for (elem_idx, elem) in arr.elems.iter().enumerate() {
        let elem: &ExprOrSpread = match elem {
            Some(e) => e,
            None => {
                return Err(mk_err(
                    source_path,
                    sm,
                    arr.span,
                    &format!("pipeline step {elem_idx}: array element must not be a hole"),
                ));
            }
        };
        if elem.spread.is_some() {
            return Err(mk_err(
                source_path,
                sm,
                elem.expr.span(),
                &format!("pipeline step {elem_idx}: spread elements are not supported"),
            ));
        }
        let obj = match &*elem.expr {
            Expr::Object(obj) => obj,
            _ => {
                return Err(mk_err(
                    source_path,
                    sm,
                    elem.expr.span(),
                    &format!("pipeline step {elem_idx}: each element must be an object literal"),
                ));
            }
        };
        let props = obj_props(obj);
        let id = get_string(&props, "id").ok_or_else(|| {
            mk_err(
                source_path,
                sm,
                obj.span(),
                &format!("pipeline step {elem_idx}: missing required string field `id`"),
            )
        })?;
        let run = get_string(&props, "run").ok_or_else(|| {
            mk_err(
                source_path,
                sm,
                obj.span(),
                &format!("pipeline step {elem_idx}: missing required string field `run`"),
            )
        })?;
        let input = get_string(&props, "input");
        steps.push(IrPipelineStep { id, run, input });
    }
    Ok(steps)
}

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
    let capabilities = if let Some(cap_expr) = props_kv.get("capabilities") {
        if let Expr::Array(arr) = cap_expr {
            extract_tool_capabilities(arr, name, source_path, sm)?
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    Ok(IrTool {
        body,
        description,
        capabilities,
    })
}

/// Extract an array of capability objects from a tool's `capabilities` field.
fn extract_tool_capabilities(
    arr: &ArrayLit,
    tool_name: &str,
    source_path: &Path,
    sm: &Lrc<SourceMap>,
) -> Result<Vec<IrCapability>, TsExtractError> {
    let mut caps = Vec::new();
    for (idx, elem) in arr.elems.iter().enumerate() {
        let elem = match elem {
            Some(e) => e,
            None => continue,
        };
        if elem.spread.is_some() {
            return Err(mk_err(
                source_path,
                sm,
                elem.expr.span(),
                &format!("tool({tool_name}) capabilities[{idx}]: spread not supported"),
            ));
        }
        let obj = match &*elem.expr {
            Expr::Object(obj) => obj,
            _ => {
                return Err(mk_err(
                    source_path,
                    sm,
                    elem.expr.span(),
                    &format!(
                        "tool({tool_name}) capabilities[{idx}]: each element must be an object"
                    ),
                ));
            }
        };
        let props = obj_props(obj);
        let kind = get_string(&props, "kind").ok_or_else(|| {
            mk_err(
                source_path,
                sm,
                obj.span(),
                &format!(
                    "tool({tool_name}) capabilities[{idx}]: missing required string field `kind`"
                ),
            )
        })?;
        let paths = if let Some(paths_expr) = props.get("paths") {
            if let Expr::Array(paths_arr) = paths_expr {
                let mut path_strs = Vec::new();
                for path_elem in paths_arr.elems.iter().flatten() {
                    if let Some(s) = expr_as_string(&path_elem.expr) {
                        path_strs.push(s);
                    }
                }
                path_strs
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        caps.push(IrCapability { kind, paths });
    }
    Ok(caps)
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
                                        capabilities: Vec::new(),
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
                        Factory::Agent
                        | Factory::Pipeline
                        | Factory::Goals
                        | Factory::Deliverables => {}
                    }
                }
            }
        }
    }

    // Extract `produces: [...]` string array.
    let produces = if let Some(prod_expr) = props.get("produces") {
        if let Expr::Array(arr) = prod_expr {
            arr.elems
                .iter()
                .flatten()
                .filter_map(|e| expr_as_string(&e.expr))
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let agent = IrAgent {
        display_name,
        package,
        llm_backend,
        model,
        prompt_system,
        tool_refs,
        produces,
    };

    Ok((agent, extra_tools))
}

/// Extract a non-negative integer from a numeric literal expression.
fn expr_as_u64(expr: &Expr) -> Option<u64> {
    if let Expr::Lit(Lit::Num(n)) = expr {
        if n.value >= 0.0 && n.value.fract() == 0.0 {
            return Some(n.value as u64);
        }
    }
    None
}

/// Try to get a u64 value from the prop map.
fn get_u64(props: &BTreeMap<String, &Expr>, key: &str) -> Option<u64> {
    let expr = props.get(key)?;
    expr_as_u64(expr)
}

fn extract_goals(
    arr: &ArrayLit,
    source_path: &Path,
    sm: &Lrc<SourceMap>,
) -> Result<Vec<IrGoal>, TsExtractError> {
    let mut goals = Vec::new();
    for (idx, elem) in arr.elems.iter().enumerate() {
        let elem = match elem {
            Some(e) => e,
            None => {
                return Err(mk_err(
                    source_path,
                    sm,
                    arr.span,
                    &format!("goals: element {idx} must not be a hole"),
                ));
            }
        };
        if elem.spread.is_some() {
            return Err(mk_err(
                source_path,
                sm,
                elem.expr.span(),
                &format!("goals: element {idx}: spread elements are not supported"),
            ));
        }
        let obj = match &*elem.expr {
            Expr::Object(obj) => obj,
            _ => {
                return Err(mk_err(
                    source_path,
                    sm,
                    elem.expr.span(),
                    &format!("goals: element {idx}: each element must be an object literal"),
                ));
            }
        };
        let props = obj_props(obj);
        let id = get_string(&props, "id").ok_or_else(|| {
            mk_err(
                source_path,
                sm,
                obj.span(),
                &format!("goals[{idx}]: missing required string field `id`"),
            )
        })?;
        let evaluates = get_string(&props, "evaluates").ok_or_else(|| {
            mk_err(
                source_path,
                sm,
                obj.span(),
                &format!("goals[{idx}] ({id}): missing required string field `evaluates`"),
            )
        })?;
        let check = get_string(&props, "check");
        let pattern = get_string(&props, "pattern");
        let equals = get_string(&props, "equals");
        let min_count = get_u64(&props, "minCount");
        let fn_name = get_string(&props, "fn");
        goals.push(IrGoal {
            id,
            evaluates,
            check,
            pattern,
            equals,
            min_count,
            fn_name,
        });
    }
    Ok(goals)
}

fn extract_deliverables(
    arr: &ArrayLit,
    source_path: &Path,
    sm: &Lrc<SourceMap>,
) -> Result<Vec<IrDeliverable>, TsExtractError> {
    let mut deliverables = Vec::new();
    for (idx, elem) in arr.elems.iter().enumerate() {
        let elem = match elem {
            Some(e) => e,
            None => {
                return Err(mk_err(
                    source_path,
                    sm,
                    arr.span,
                    &format!("deliverables: element {idx} must not be a hole"),
                ));
            }
        };
        if elem.spread.is_some() {
            return Err(mk_err(
                source_path,
                sm,
                elem.expr.span(),
                &format!("deliverables: element {idx}: spread elements are not supported"),
            ));
        }
        let obj = match &*elem.expr {
            Expr::Object(obj) => obj,
            _ => {
                return Err(mk_err(
                    source_path,
                    sm,
                    elem.expr.span(),
                    &format!("deliverables: element {idx}: each element must be an object literal"),
                ));
            }
        };
        let props = obj_props(obj);
        let id = get_string(&props, "id").ok_or_else(|| {
            mk_err(
                source_path,
                sm,
                obj.span(),
                &format!("deliverables[{idx}]: missing required string field `id`"),
            )
        })?;
        let must_satisfy = get_string(&props, "mustSatisfy").ok_or_else(|| {
            mk_err(
                source_path,
                sm,
                obj.span(),
                &format!("deliverables[{idx}] ({id}): missing required string field `mustSatisfy`"),
            )
        })?;
        let path = get_string(&props, "path");
        let output = get_string(&props, "output");
        // camelCase → snake_case mappings
        let on_fail = get_string(&props, "onFail");
        let max_attempts = get_u64(&props, "maxAttempts");
        let retry_from = get_string(&props, "retryFrom");
        let judge_model = get_string(&props, "judgeModel");
        let judge = get_string(&props, "judge");
        deliverables.push(IrDeliverable {
            id,
            path,
            output,
            must_satisfy,
            on_fail,
            max_attempts,
            retry_from,
            judge_model,
            judge,
        });
    }
    Ok(deliverables)
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
