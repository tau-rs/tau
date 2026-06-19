//! Tau factory call recognizer.

use swc_ecma_ast::{ArrayLit, CallExpr, Callee, Expr, Lit, ObjectLit};

/// Which tau factory a call expression invokes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Factory {
    Agent,
    Tool,
    Mcp,
    ContextManager,
    Pipeline,
    Goals,
    Deliverables,
    Models,
}

/// If `expr` is a tau factory call like `agent({...})`, return the
/// factory kind and the [`CallExpr`].
pub fn recognize_factory_call(expr: &Expr) -> Option<(Factory, &CallExpr)> {
    if let Expr::Call(call) = expr {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Ident(ident) = callee.as_ref() {
                let factory = match ident.sym.as_ref() {
                    "agent" => Factory::Agent,
                    "tool" => Factory::Tool,
                    "mcp" => Factory::Mcp,
                    "contextManager" => Factory::ContextManager,
                    "pipeline" => Factory::Pipeline,
                    "goals" => Factory::Goals,
                    "deliverables" => Factory::Deliverables,
                    "models" => Factory::Models,
                    _ => return None,
                };
                return Some((factory, call));
            }
        }
    }
    None
}

/// Extract a string literal value from a `CallExpr`'s positional arg at `idx`.
pub fn arg_as_string(call: &CallExpr, idx: usize) -> Option<String> {
    let arg = call.args.get(idx)?;
    if arg.spread.is_some() {
        return None;
    }
    if let Expr::Lit(Lit::Str(s)) = &*arg.expr {
        return Some(s.value.as_wtf8().to_string_lossy().into_owned());
    }
    None
}

/// Return the first argument as an object literal, if present and not spread.
pub fn arg_as_object(call: &CallExpr, idx: usize) -> Option<&ObjectLit> {
    let arg = call.args.get(idx)?;
    if arg.spread.is_some() {
        return None;
    }
    if let Expr::Object(obj) = &*arg.expr {
        return Some(obj);
    }
    None
}

/// Return the argument at `idx` as an array literal, if present and not spread.
pub fn arg_as_array(call: &CallExpr, idx: usize) -> Option<&ArrayLit> {
    let arg = call.args.get(idx)?;
    if arg.spread.is_some() {
        return None;
    }
    if let Expr::Array(arr) = &*arg.expr {
        return Some(arr);
    }
    None
}
