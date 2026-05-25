// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{ArrayElements, BindingItem, Expr, ExprKind, IndexArg, ItemKind, Module, UnaryOp};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_item_signatures::ItemSignatures;
use nia_local_resolve::{LocalResolution, LocalUse};
use nia_span::Span;
use nia_value_resolve::{ValueNameResolution, ValueResolution};

#[derive(Debug, Clone, PartialEq)]
pub struct StaticCheck {
    pub diagnostics: Vec<Diagnostic>,
}

pub fn check_module_static_initializers(
    module: &Module,
    defs: &DefCollection,
    values: &ValueResolution,
    locals: &LocalResolution,
    signatures: &ItemSignatures,
) -> StaticCheck {
    let mut checker = StaticChecker {
        defs,
        values,
        locals,
        signatures,
        diagnostics: Vec::new(),
    };
    checker.check_module(module);
    StaticCheck {
        diagnostics: checker.diagnostics,
    }
}

struct StaticChecker<'a> {
    defs: &'a DefCollection,
    values: &'a ValueResolution,
    locals: &'a LocalResolution,
    signatures: &'a ItemSignatures,
    diagnostics: Vec<Diagnostic>,
}

impl StaticChecker<'_> {
    fn check_module(&mut self, module: &Module) {
        for item in &module.items {
            if let ItemKind::Binding(binding) = &item.kind {
                self.check_global_binding(item.span, binding);
            }
        }
    }

    fn check_global_binding(&mut self, span: Span, binding: &BindingItem) {
        let Some(def_id) = self.def_id_for_span(span, DefKind::Global) else {
            return;
        };
        let Some(signature) = self.signatures.globals.get(&def_id) else {
            return;
        };
        if signature.is_extern {
            return;
        }
        let Some(value) = &binding.value else {
            return;
        };
        if let Some(reason) = self.static_init_reject_reason(value) {
            self.diagnostics.push(Diagnostic::error(
                value.span,
                format!("global initializer is not static data: {reason}"),
            ));
        }
    }

    fn static_init_reject_reason(&self, expr: &Expr) -> Option<&'static str> {
        match &expr.kind {
            ExprKind::Error => None,
            ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Bool(_) => None,
            ExprKind::ArrayLiteral { elems } => match elems {
                ArrayElements::List(elems) => elems
                    .iter()
                    .find_map(|elem| self.static_init_reject_reason(elem)),
                ArrayElements::Repeat { value, .. } => self.static_init_reject_reason(value),
            },
            ExprKind::StructLiteral { fields } => fields
                .iter()
                .find_map(|field| self.static_init_reject_reason(&field.value)),
            ExprKind::Unary { op, expr: inner } => match op {
                UnaryOp::Neg => self.int_const_expr_reject_reason(expr),
                UnaryOp::Ref | UnaryOp::RefConst => self.static_address_path_reject_reason(inner),
                UnaryOp::Not | UnaryOp::Deref => Some("unsupported unary operator"),
            },
            ExprKind::Binary { .. } => self.int_const_expr_reject_reason(expr),
            ExprKind::Cast { expr: inner, .. } => self.static_init_reject_reason(inner),
            ExprKind::Builtin { .. } => None,
            ExprKind::Ident(_) => match self.locals.uses.get(&expr.span) {
                Some(LocalUse::ModuleValue) => match self.values.names.get(&expr.span) {
                    Some(ValueNameResolution::Def(def_id)) if self.is_enum_variant(*def_id) => None,
                    _ => Some("bare global value is not static data; take its address explicitly"),
                },
                Some(LocalUse::Unresolved) | None => None,
                Some(LocalUse::Local(_)) => Some("local value is not available in global storage"),
                Some(LocalUse::ImportAlias) => Some("import alias is not static data"),
                Some(LocalUse::TypePrefix) => Some("type prefix is not static data"),
            },
            ExprKind::Qualified { lhs, name: _ } => {
                if self.is_enum_variant_access(expr, lhs) {
                    None
                } else {
                    self.static_address_path_reject_reason(expr)
                }
            }
            ExprKind::Field { lhs, .. } => self.static_address_path_reject_reason(lhs),
            ExprKind::Index { .. } => self.static_address_path_reject_reason(expr),
            ExprKind::BracketSuffix { args, .. } if Self::bracket_index_arg(args).is_some() => {
                self.static_address_path_reject_reason(expr)
            }
            ExprKind::Block(_) => Some("block expressions require comptime execution"),
            ExprKind::If { .. } => Some("if expressions require comptime execution"),
            ExprKind::Call { .. } => Some("function calls require comptime execution"),
            ExprKind::Assign { .. } => Some("assignment cannot initialize global storage"),
            ExprKind::BracketSuffix { .. } => Some("generic instantiation is not a static value"),
            ExprKind::Underscore => Some("underscore is not a value"),
            ExprKind::Raw(_) => Some("raw expression is not static data"),
        }
    }

    fn int_const_expr_reject_reason(&self, expr: &Expr) -> Option<&'static str> {
        match &expr.kind {
            ExprKind::Integer(_) => None,
            ExprKind::Unary {
                op: UnaryOp::Neg,
                expr: inner,
            } => self.int_const_expr_reject_reason(inner),
            ExprKind::Binary { lhs, rhs, .. } => self
                .int_const_expr_reject_reason(lhs)
                .or_else(|| self.int_const_expr_reject_reason(rhs)),
            _ => Some("expression is not an integer constant expression"),
        }
    }

    fn static_address_reject_reason(&self, expr: &Expr) -> Option<&'static str> {
        if self.values.qualified_values.contains_key(&expr.span) {
            return None;
        }
        match &expr.kind {
            ExprKind::Ident(_) => match self.locals.uses.get(&expr.span) {
                Some(LocalUse::ModuleValue) => match self.values.names.get(&expr.span) {
                    Some(ValueNameResolution::Def(def_id))
                        if self.is_global(*def_id) || self.is_function(*def_id) =>
                    {
                        None
                    }
                    Some(ValueNameResolution::Def(_)) => Some("address target is not static"),
                    _ => None,
                },
                Some(LocalUse::Unresolved) | None => None,
                Some(LocalUse::Local(_)) => Some("address target is local storage"),
                Some(LocalUse::ImportAlias) => Some("import alias has no address"),
                Some(LocalUse::TypePrefix) => Some("type prefix has no address"),
            },
            _ => Some("address target is not global storage"),
        }
    }

    fn static_address_path_reject_reason(&self, expr: &Expr) -> Option<&'static str> {
        if self.values.qualified_values.contains_key(&expr.span) {
            return None;
        }
        match &expr.kind {
            ExprKind::Field { lhs, .. } => self.static_address_path_reject_reason(lhs),
            ExprKind::Index { lhs, index } => {
                self.static_address_path_reject_reason(lhs)
                    .or_else(|| match index {
                        IndexArg::Expr(index) => match nia_const_eval::eval_array_len_expr(index) {
                            Ok(_) => None,
                            Err(_) => Some("array index is not a static integer constant"),
                        },
                        IndexArg::Range(_) => Some("range index is not valid in a static address"),
                    })
            }
            ExprKind::BracketSuffix { callee, args } => {
                if let Some(index) = Self::bracket_index_arg(args) {
                    return self.static_address_path_reject_reason(callee).or_else(|| {
                        match nia_const_eval::eval_array_len_expr(index) {
                            Ok(_) => None,
                            Err(_) => Some("array index is not a static integer constant"),
                        }
                    });
                }
                self.static_address_reject_reason(expr)
            }
            _ => self.static_address_reject_reason(expr),
        }
    }

    fn bracket_index_arg(args: &[nia_ast::BracketArg]) -> Option<&Expr> {
        if args.len() == 1 {
            args.first().and_then(|arg| arg.expr.as_ref())
        } else {
            None
        }
    }

    fn is_global(&self, def_id: DefId) -> bool {
        matches!(
            self.defs.defs.get(def_id).map(|def| def.kind),
            Some(DefKind::Global)
        )
    }

    fn is_function(&self, def_id: DefId) -> bool {
        matches!(
            self.defs.defs.get(def_id).map(|def| def.kind),
            Some(DefKind::Function | DefKind::Method)
        )
    }

    fn is_enum_variant(&self, def_id: DefId) -> bool {
        matches!(
            self.defs.defs.get(def_id).map(|def| def.kind),
            Some(DefKind::EnumVariant)
        )
    }

    fn is_enum_variant_access(&self, expr: &Expr, lhs: &Expr) -> bool {
        matches!(
            self.values.names.get(&expr.span),
            Some(ValueNameResolution::Def(def_id)) if self.is_enum_variant(*def_id)
        ) || matches!(
            self.values.qualified_values.get(&expr.span),
            Some(def_id) if self.is_enum_variant(def_id.def_id)
        ) || matches!(self.locals.uses.get(&lhs.span), Some(LocalUse::TypePrefix))
    }

    fn def_id_for_span(&self, span: Span, expected: DefKind) -> Option<DefId> {
        let def_id = self.defs.def_spans.get(span)?;
        let def = self.defs.defs.get(def_id)?;
        (def.kind == expected).then_some(def_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_item_signatures::collect_item_signatures;
    use nia_local_resolve::resolve_module_locals;
    use nia_parser::parse_module;
    use nia_type_lower::lower_module_types_with_id;
    use nia_type_resolve::resolve_module_types;
    use nia_value_resolve::resolve_module_values;

    fn check(source: &str) -> StaticCheck {
        let (module, errors) = parse_module(source);
        assert!(errors.is_empty(), "{errors:?}");
        let module_id = ModuleId(0);
        let defs = collect_module_defs(module_id, &module);
        let type_resolution = resolve_module_types(&module, &defs);
        let type_lowering = lower_module_types_with_id(module_id, &module, &type_resolution);
        let signatures = collect_item_signatures(&module, &defs, &type_lowering);
        let values = resolve_module_values(&module, &defs);
        let locals = resolve_module_locals(&module, &defs, &values);
        check_module_static_initializers(&module, &defs, &values, &locals, &signatures)
    }

    #[test]
    fn rejects_block_call_and_bare_global_initializers() {
        let checked = check(
            r#"
fn make() i32 { 1 }

var base: i32 = 1;
var bad_block = { 1 };
var bad_call = make();
var bad_bare_ptr: &i32 = base;
"#,
        );

        assert_eq!(checked.diagnostics.len(), 3, "{:?}", checked.diagnostics);
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("block expressions"))
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("function calls"))
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("bare global value"))
        );
    }

    #[test]
    fn accepts_static_data_and_global_addresses() {
        let checked = check(
            r#"
struct Pair {
    x: i32,
    y: i32,
}

var base: i32 = 1 + 2;
var pair: Pair = { x: 1, y: 2 };
var xs: [2]i32 = [1, 2];
var p: &i32 = &base;
var q: &i32 = &pair.x;
var r: &i32 = &xs[1];

struct Vtable {
    print: &const fn(&i32)
}

fn print_i32(value: &i32) {}
const vtable: Vtable = { print: &const print_i32 };
"#,
        );

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn rejects_non_static_global_address_indexes() {
        let checked = check(
            r#"
var target: [2]i32 = [1, 2];
var idx: i32 = 1;
var bad: &i32 = &target[idx];
"#,
        );

        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("static integer constant")),
            "{:?}",
            checked.diagnostics
        );
    }
}
