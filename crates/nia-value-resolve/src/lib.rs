// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{Expr, ExprKind, Module, Visibility};
use nia_ast_walk::{Visitor, walk_expr, walk_module};
use nia_defs::{DefCollection, DefKind, ModuleUsingScope, PublicNamespace, PublicSurfaces};
use nia_diagnostic::Diagnostic;
pub use nia_ids::DefId;
use nia_ids::GlobalDefId;
use nia_imports::ImportAliasMap;
use nia_span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct ValueResolution {
    pub names: HashMap<Span, ValueNameResolution>,
    pub qualified_values: HashMap<Span, GlobalDefId>,
    /// For spans whose value resolves to an enum variant (brought in via
    /// `using` or accessed as `mod::Enum::Variant`), the parent enum's
    /// GlobalDefId so consumers can type the bare ident as that enum.
    pub variant_enums: HashMap<Span, GlobalDefId>,
    /// For `Qualified` spans like `mod::TypeName` appearing in expression
    /// position (e.g., as a type prefix in `mod::Enum::Variant` or
    /// `mod::Type::associated_fn(...)`), the resolved type's GlobalDefId.
    /// Populated by value-resolve so downstream phases can recognise these
    /// as type prefixes without re-resolving the import alias.
    pub qualified_type_prefixes: HashMap<Span, GlobalDefId>,
    pub builtins: HashMap<Span, BuiltinResolution>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueNameResolution {
    Def(DefId),
    External(GlobalDefId),
    ImportAlias,
    LocalDeferred,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinResolution {
    SizeOf,
    AlignOf,
    Len,
    Ptr,
    Asm,
    Reserved,
}

pub fn resolve_module_values(module: &Module, defs: &DefCollection) -> ValueResolution {
    resolve_module_values_inner(module, defs, None, &[], None, None)
}

pub fn resolve_module_values_with_imports(
    module: &Module,
    defs: &DefCollection,
    imports: &ImportAliasMap,
    all_defs: &[DefCollection],
) -> ValueResolution {
    resolve_module_values_inner(module, defs, Some(imports), all_defs, None, None)
}

pub fn resolve_module_values_with_context(
    module: &Module,
    defs: &DefCollection,
    imports: &ImportAliasMap,
    all_defs: &[DefCollection],
    public_surfaces: &PublicSurfaces,
    using_scope: &ModuleUsingScope,
) -> ValueResolution {
    resolve_module_values_inner(
        module,
        defs,
        Some(imports),
        all_defs,
        Some(public_surfaces),
        Some(using_scope),
    )
}

fn resolve_module_values_inner(
    module: &Module,
    defs: &DefCollection,
    imports: Option<&ImportAliasMap>,
    all_defs: &[DefCollection],
    public_surfaces: Option<&PublicSurfaces>,
    using_scope: Option<&ModuleUsingScope>,
) -> ValueResolution {
    let mut resolver = ValueResolver {
        defs,
        imports,
        all_defs,
        public_surfaces,
        using_scope,
        names: HashMap::new(),
        qualified_values: HashMap::new(),
        variant_enums: HashMap::new(),
        qualified_type_prefixes: HashMap::new(),
        builtins: HashMap::new(),
        diagnostics: Vec::new(),
    };
    walk_module(&mut resolver, module);
    ValueResolution {
        names: resolver.names,
        qualified_values: resolver.qualified_values,
        variant_enums: resolver.variant_enums,
        qualified_type_prefixes: resolver.qualified_type_prefixes,
        builtins: resolver.builtins,
        diagnostics: resolver.diagnostics,
    }
}

struct ValueResolver<'a> {
    defs: &'a DefCollection,
    imports: Option<&'a ImportAliasMap>,
    all_defs: &'a [DefCollection],
    public_surfaces: Option<&'a PublicSurfaces>,
    using_scope: Option<&'a ModuleUsingScope>,
    names: HashMap<Span, ValueNameResolution>,
    qualified_values: HashMap<Span, GlobalDefId>,
    variant_enums: HashMap<Span, GlobalDefId>,
    qualified_type_prefixes: HashMap<Span, GlobalDefId>,
    builtins: HashMap<Span, BuiltinResolution>,
    diagnostics: Vec<Diagnostic>,
}

impl<'ast> Visitor<'ast> for ValueResolver<'_> {
    fn visit_expr(&mut self, expr: &'ast Expr) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                let resolution = self.resolve_ident(name, expr.span);
                if let ValueNameResolution::External(global_id) = resolution {
                    self.qualified_values.insert(expr.span, global_id);
                }
                self.names.insert(expr.span, resolution);
            }
            ExprKind::Builtin { name, .. } => {
                let resolution = self.resolve_builtin(name, expr.span);
                self.builtins.insert(expr.span, resolution);
                walk_expr(self, expr);
            }
            ExprKind::Call { callee, args } => {
                if let ExprKind::Builtin { name, .. } = &callee.kind
                    && name == "asm"
                {
                    self.visit_expr(callee);
                    for arg in args {
                        self.visit_asm_config(arg);
                    }
                } else {
                    walk_expr(self, expr);
                }
            }
            ExprKind::BracketSuffix { callee, args } => {
                self.visit_expr(callee);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        self.visit_expr(expr);
                    }
                }
            }
            ExprKind::Qualified { lhs, .. } => {
                self.visit_expr(lhs);
                self.resolve_qualified_value(expr);
            }
            _ => walk_expr(self, expr),
        }
    }
}

impl<'a> ValueResolver<'a> {
    fn visit_asm_config(&mut self, expr: &Expr) {
        let ExprKind::StructLiteral { fields } = &expr.kind else {
            self.visit_expr(expr);
            return;
        };
        for field in fields {
            match field.name.as_str() {
                "inputs" | "outputs" => self.visit_expr(&field.value),
                "code" | "clobbers" => {}
                _ => self.visit_expr(&field.value),
            }
        }
    }

    fn resolve_qualified_value(&mut self, expr: &Expr) {
        let ExprKind::Qualified { lhs, name } = &expr.kind else {
            return;
        };
        let ExprKind::Ident(module_name) = &lhs.kind else {
            return;
        };
        let Some(imports) = self.imports else {
            return;
        };
        let Some(import) = imports.get(self.defs.module_id, module_name) else {
            return;
        };
        self.names
            .insert(lhs.span, ValueNameResolution::ImportAlias);
        if let Some(surfaces) = self.public_surfaces
            && let Some(surface) = surfaces.get(import.target)
            && let Some(item) = surface.lookup_value(name)
        {
            self.qualified_values.insert(
                expr.span,
                GlobalDefId {
                    module_id: item.target_module,
                    def_id: item.target_def_id,
                },
            );
            if let Some(enum_id) = item.parent_enum {
                self.variant_enums.insert(expr.span, enum_id);
            }
            return;
        }
        // Look for a type in the target module's public surface — `mod::Type`
        // appearing in expression position is a valid type prefix (e.g.
        // `mod::Enum::Variant` or `mod::Type::associated_fn(...)`).
        if let Some(surfaces) = self.public_surfaces
            && let Some(surface) = surfaces.get(import.target)
            && let Some(item) = surface.lookup_type(name)
        {
            self.qualified_type_prefixes.insert(
                expr.span,
                GlobalDefId {
                    module_id: item.target_module,
                    def_id: item.target_def_id,
                },
            );
            return;
        }
        // Fall back to scanning the target module's def table so direct
        // crate-level tests can run without constructing public surfaces.
        let Some(target_defs) = self
            .all_defs
            .iter()
            .find(|defs| defs.module_id == import.target)
        else {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                format!("import alias `{module_name}` refers to an unloaded module"),
            ));
            return;
        };
        if let Some(def_id) = target_defs.module_scope.types.get(name) {
            // Type prefix path (possibly private — but we just record it; later
            // phases will diagnose visibility if needed).
            self.qualified_type_prefixes.insert(
                expr.span,
                GlobalDefId {
                    module_id: import.target,
                    def_id,
                },
            );
            return;
        }
        let Some(def_id) = target_defs.module_scope.values.get(name) else {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                format!("unknown value `{module_name}::{name}`"),
            ));
            return;
        };
        let Some(def) = target_defs.defs.get(def_id) else {
            return;
        };
        if import.target != self.defs.module_id && def.visibility != Visibility::Public {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                format!("value `{module_name}::{name}` is private"),
            ));
            return;
        }
        if matches!(
            def.kind,
            DefKind::Function | DefKind::Global | DefKind::Comptime
        ) {
            self.qualified_values.insert(
                expr.span,
                GlobalDefId {
                    module_id: import.target,
                    def_id,
                },
            );
        }
    }

    fn resolve_ident(&mut self, name: &str, span: Span) -> ValueNameResolution {
        if let Some(def_id) = self.defs.module_scope.values.get(name) {
            let Some(def) = self.defs.defs.get(def_id) else {
                return ValueNameResolution::Error;
            };
            if matches!(
                def.kind,
                DefKind::Function | DefKind::Global | DefKind::Comptime
            ) {
                return ValueNameResolution::Def(def_id);
            }
        }

        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.lookup_value(name)
            && entry.namespace == PublicNamespace::Value
        {
            if let Some(enum_id) = entry.parent_enum {
                self.variant_enums.insert(span, enum_id);
            }
            return ValueNameResolution::External(GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            });
        }

        // Local bindings and parameters are resolved by nia-local-resolve.
        ValueNameResolution::LocalDeferred
    }

    fn resolve_builtin(&mut self, name: &str, span: Span) -> BuiltinResolution {
        match name {
            "size" => BuiltinResolution::SizeOf,
            "align" => BuiltinResolution::AlignOf,
            "len" => BuiltinResolution::Len,
            "ptr" => BuiltinResolution::Ptr,
            "asm" => BuiltinResolution::Asm,
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("unknown builtin `@{name}`"),
                ));
                BuiltinResolution::Reserved
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_parser::parse_module;

    #[test]
    fn resolves_module_value_names_and_defers_locals() {
        let (module, errors) = parse_module(
            r#"
var counter = 0;

fn add(a: i32, b: i32) i32 {
    a + b + counter
}

fn main() i32 {
    var local = add(counter, 1);
    local
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_values(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        assert!(
            resolved
                .names
                .values()
                .any(|resolution| matches!(resolution, ValueNameResolution::Def(_)))
        );
        assert!(
            resolved
                .names
                .values()
                .any(|resolution| matches!(resolution, ValueNameResolution::LocalDeferred))
        );
    }

    #[test]
    fn validates_builtin_names_only() {
        let (module, errors) = parse_module(
            r#"
fn main() usize {
    var a = @size[usize]();
    var b = @align[usize]();
    var c = @unknown[usize]();
    a + b + c
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_values(&module, &defs);
        assert_eq!(resolved.diagnostics.len(), 1);
        assert!(
            resolved.diagnostics[0]
                .message
                .contains("unknown builtin `@unknown`")
        );
        assert!(
            resolved
                .builtins
                .values()
                .any(|builtin| matches!(builtin, BuiltinResolution::SizeOf))
        );
        assert!(
            resolved
                .builtins
                .values()
                .any(|builtin| matches!(builtin, BuiltinResolution::AlignOf))
        );
    }
}
