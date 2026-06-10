//! Top-level constant walker → name → Expr map.

use std::collections::BTreeMap;

use swc_ecma_ast::{Decl, Expr, Module, ModuleDecl, ModuleItem, Pat, Stmt, VarDecl, VarDeclarator};

/// Map from top-level constant name to its initializer expression.
pub type NameMap<'a> = BTreeMap<String, &'a Expr>;

/// Walk a parsed Module and collect every top-level `const NAME = EXPR;`
/// (including `export const`).
pub fn collect_top_level(module: &Module) -> NameMap<'_> {
    let mut names = BTreeMap::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                if var.kind == swc_ecma_ast::VarDeclKind::Const {
                    collect_from_var(var, &mut names);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(exp)) => {
                if let Decl::Var(var) = &exp.decl {
                    if var.kind == swc_ecma_ast::VarDeclKind::Const {
                        collect_from_var(var, &mut names);
                    }
                }
            }
            _ => {}
        }
    }
    names
}

fn collect_from_var<'a>(var: &'a VarDecl, names: &mut NameMap<'a>) {
    for decl in &var.decls {
        if let (Some(name), Some(init)) = (extract_ident_name(decl), decl.init.as_deref()) {
            names.insert(name, init);
        }
    }
}

fn extract_ident_name(decl: &VarDeclarator) -> Option<String> {
    match &decl.name {
        Pat::Ident(binding) => Some(binding.id.sym.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_module;
    use std::path::Path;

    #[test]
    fn collects_single_top_level_const() {
        let src = r#"const foo = 42;"#;
        let (module, sm) = parse_module(src, Path::new("/tmp/t.ts")).unwrap();
        let names = collect_top_level(&module);
        assert!(
            names.contains_key("foo"),
            "expected `foo`, got: {:?}",
            names.keys().collect::<Vec<_>>()
        );
        let _ = sm; // keep SourceMap alive
    }

    #[test]
    fn collects_exported_const() {
        let src = r#"export const bar = "hello";"#;
        let (module, _sm) = parse_module(src, Path::new("/tmp/t.ts")).unwrap();
        let names = collect_top_level(&module);
        assert!(names.contains_key("bar"));
    }

    #[test]
    fn collects_multiple_declarations() {
        let src = r#"
            const a = 1;
            const b = "x";
            export const c = { foo: "bar" };
        "#;
        let (module, _sm) = parse_module(src, Path::new("/tmp/t.ts")).unwrap();
        let names = collect_top_level(&module);
        assert_eq!(names.len(), 3);
        assert!(names.contains_key("a"));
        assert!(names.contains_key("b"));
        assert!(names.contains_key("c"));
    }
}
