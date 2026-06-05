// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{ArrayElements, BindingItem, Expr, ExprKind, IndexArg, ItemKind, Module, UnaryOp};
use nia_comptime_check::{ComptimeCheck, ComptimeKey};
use nia_comptime_engine::{ComptimeCommonEnv, ComptimeError, ComptimeValue, ResolvedComptimeEnv};
use nia_comptime_ir::{ResolvedComptimeExpr, ResolvedComptimeTypeArg};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId};
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
    comptime: &ComptimeCheck,
    type_uses: &HashMap<Span, InternedTyId>,
    program_defs: &HashMap<ModuleId, DefCollection>,
    program_comptime: &HashMap<ModuleId, ComptimeCheck>,
) -> StaticCheck {
    let mut checker = StaticChecker {
        defs,
        values,
        locals,
        signatures,
        comptime,
        type_uses,
        program_defs,
        program_comptime,
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
    comptime: &'a ComptimeCheck,
    type_uses: &'a HashMap<Span, InternedTyId>,
    program_defs: &'a HashMap<ModuleId, DefCollection>,
    program_comptime: &'a HashMap<ModuleId, ComptimeCheck>,
    diagnostics: Vec<Diagnostic>,
}

impl StaticChecker<'_> {
    fn check_module(&mut self, module: &Module) {
        for item in &module.items {
            if let ItemKind::Binding(binding) = &item.kind
                && !binding.is_comptime
            {
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
            | ExprKind::ByteString(_)
            | ExprKind::CString(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Bool(_) => None,
            ExprKind::ArrayLiteral { elems } | ExprKind::TypedArrayLiteral { elems, .. } => {
                match elems {
                    ArrayElements::List(elems) => elems
                        .iter()
                        .find_map(|elem| self.static_init_reject_reason(elem)),
                    ArrayElements::Repeat { value, count } => self
                        .static_init_reject_reason(value)
                        .or_else(|| self.static_array_repeat_count_reject_reason(count)),
                }
            }
            ExprKind::StructLiteral { fields } | ExprKind::TypedStructLiteral { fields, .. } => {
                fields
                    .iter()
                    .find_map(|field| self.static_init_reject_reason(&field.value))
            }
            ExprKind::Unary { op, expr: inner } => match op {
                UnaryOp::Neg => self.static_int_expr_reject_reason(expr),
                UnaryOp::Ref | UnaryOp::RefReadOnly => {
                    self.static_address_path_reject_reason(inner)
                }
                UnaryOp::Not | UnaryOp::BitNot | UnaryOp::Deref => {
                    Some("unsupported unary operator")
                }
            },
            ExprKind::Binary { .. } => self.static_int_expr_reject_reason(expr),
            ExprKind::Cast { expr: inner, .. } => self.static_init_reject_reason(inner),
            ExprKind::Builtin { .. } => None,
            ExprKind::TypeTarget { .. } => Some("type target is not static data"),
            ExprKind::Ident(_) => match self.locals.uses.get(&expr.span) {
                Some(LocalUse::ModuleValue) => match self.values.names.get(&expr.span) {
                    Some(ValueNameResolution::Def(def_id)) if self.is_enum_variant(*def_id) => None,
                    Some(ValueNameResolution::Def(def_id)) if self.is_comptime(*def_id) => None,
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
            ExprKind::Range(_) => Some("range expression is not static data"),
            ExprKind::Null => Some("null is not supported in global static data yet"),
            ExprKind::OptionalSome { .. } => {
                Some("optional construction is not supported in global static data yet")
            }
            ExprKind::ErrorOk { .. } | ExprKind::ErrorErr { .. } => {
                Some("error union construction is not supported in global static data yet")
            }
            ExprKind::Try { .. } => Some("`.?` propagation requires runtime control flow"),
            ExprKind::Block(_) => Some("block expressions require comptime execution"),
            ExprKind::If { .. } => Some("if expressions require comptime execution"),
            ExprKind::ComptimeIf(_) => Some("comptime if expressions require target pruning"),
            ExprKind::Switch(_) => Some("switch expressions require comptime execution"),
            ExprKind::Call { .. } => Some("function calls require comptime execution"),
            ExprKind::Assign { .. } => Some("assignment cannot initialize global storage"),
            ExprKind::BracketSuffix { .. } => Some("generic instantiation is not a static value"),
            ExprKind::Underscore => Some("underscore is not a value"),
            ExprKind::Raw(_) => Some("raw expression is not static data"),
        }
    }

    fn static_int_expr_reject_reason(&self, expr: &Expr) -> Option<&'static str> {
        self.eval_static_int_expr(expr)
            .err()
            .map(|_| "expression is not an integer constant expression")
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
        if self.values.qualified_type_prefixes.contains_key(&expr.span) {
            return None;
        }
        match &expr.kind {
            ExprKind::Qualified { lhs, .. } => {
                if self.values.qualified_type_prefixes.contains_key(&expr.span) {
                    return None;
                }
                if self.is_type_prefix_expr(lhs) {
                    return None;
                }
                self.static_address_path_reject_reason(lhs)
            }
            ExprKind::TypeTarget { .. } => None,
            ExprKind::Field { lhs, .. } => self.static_address_path_reject_reason(lhs),
            ExprKind::Index { lhs, index } => {
                self.static_address_path_reject_reason(lhs)
                    .or_else(|| match index {
                        IndexArg::Expr(index) => match self.eval_static_array_index(index) {
                            Ok(_) => None,
                            Err(_) => Some("array index is not a static integer constant"),
                        },
                        IndexArg::Range(_) => Some("range index is not valid in a static address"),
                    })
            }
            ExprKind::BracketSuffix { callee, args } => {
                if let Some(index) = Self::bracket_index_arg(args) {
                    return self.static_address_path_reject_reason(callee).or_else(|| {
                        match self.eval_static_array_index(index) {
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

    fn is_type_prefix_expr(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Ident(_) => {
                matches!(self.locals.uses.get(&expr.span), Some(LocalUse::TypePrefix))
            }
            ExprKind::Qualified { .. } => {
                self.values.qualified_type_prefixes.contains_key(&expr.span)
            }
            ExprKind::TypeTarget { .. } => true,
            ExprKind::BracketSuffix { callee, .. } => self.is_type_prefix_expr(callee),
            _ => false,
        }
    }

    fn bracket_index_arg(args: &[nia_ast::BracketArg]) -> Option<&Expr> {
        if args.len() == 1 {
            args.first().and_then(|arg| arg.expr.as_ref())
        } else {
            None
        }
    }

    fn eval_static_array_index(
        &self,
        expr: &Expr,
    ) -> Result<u64, nia_comptime_engine::ComptimeError> {
        self.eval_static_resolved_expr(expr, |expr, env| {
            nia_comptime_engine::eval_resolved_comptime_array_len_expr(expr, env)
        })
    }

    fn static_array_repeat_count_reject_reason(&self, expr: &Expr) -> Option<&'static str> {
        self.eval_static_array_index(expr)
            .err()
            .map(|_| "array repeat count is not a static usize constant")
    }

    fn eval_static_int_expr(
        &self,
        expr: &Expr,
    ) -> Result<i128, nia_comptime_engine::ComptimeError> {
        self.eval_static_resolved_expr(expr, |expr, env| {
            nia_comptime_engine::eval_resolved_comptime_int_expr(expr, env)
        })
    }

    fn eval_static_resolved_expr<T>(
        &self,
        expr: &Expr,
        eval: impl FnOnce(
            &ResolvedComptimeExpr,
            &mut StaticComptimeEnv<'_>,
        ) -> Result<T, nia_comptime_engine::ComptimeError>,
    ) -> Result<T, nia_comptime_engine::ComptimeError> {
        let name_resolution = |span| self.comptime_name_resolution(span);
        let local_id = |span| self.local_id(span);
        let type_id = |span| self.type_uses.get(&span).copied();
        let context = nia_comptime_ir::ResolvedComptimeLowerInputs::new(
            &name_resolution,
            &local_id,
            &type_id,
        );
        let mut env = StaticComptimeEnv {
            defs: self.defs,
            comptime: self.comptime,
            program_defs: self.program_defs,
            program_comptime: self.program_comptime,
        };
        let expr =
            nia_comptime_ir::lower_expr_resolved_with_context(expr, &context).map_err(|err| {
                nia_comptime_engine::ComptimeError {
                    span: err.span,
                    message: err.message,
                }
            })?;
        eval(&expr, &mut env)
    }

    fn local_id(&self, span: Span) -> Option<LocalId> {
        match self.locals.uses.get(&span) {
            Some(LocalUse::Local(local_id)) => Some(*local_id),
            _ => None,
        }
    }

    fn comptime_name_resolution(
        &self,
        span: Span,
    ) -> Option<nia_comptime_ir::ComptimeNameResolution> {
        if let Some(LocalUse::Local(local_id)) = self.locals.uses.get(&span) {
            return self
                .comptime
                .values
                .contains_key(&ComptimeKey::Local(*local_id))
                .then_some(nia_comptime_ir::ComptimeNameResolution::Local(*local_id));
        }
        if let Some(global_id) = self.global_comptime_use(span) {
            return Some(nia_comptime_ir::ComptimeNameResolution::Global(global_id));
        }
        None
    }

    fn global_comptime_use(&self, span: Span) -> Option<GlobalDefId> {
        if let Some(global_id) = self.values.qualified_values.get(&span).copied() {
            if self.global_def_kind(global_id) == Some(DefKind::Comptime) {
                return Some(global_id);
            }
            return None;
        }
        let Some(ValueNameResolution::Def(def_id)) = self.values.names.get(&span) else {
            return None;
        };
        let def = self.defs.defs.get(*def_id)?;
        (def.kind == DefKind::Comptime).then_some(GlobalDefId {
            module_id: self.defs.module_id,
            def_id: *def_id,
        })
    }

    fn global_def_kind(&self, global_id: GlobalDefId) -> Option<DefKind> {
        (global_id.module_id == self.defs.module_id)
            .then(|| self.defs.defs.get(global_id.def_id).map(|def| def.kind))
            .flatten()
    }

    fn is_global(&self, def_id: DefId) -> bool {
        matches!(
            self.defs.defs.get(def_id).map(|def| def.kind),
            Some(DefKind::Global)
        )
    }

    fn is_comptime(&self, def_id: DefId) -> bool {
        matches!(
            self.defs.defs.get(def_id).map(|def| def.kind),
            Some(DefKind::Comptime)
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

struct StaticComptimeEnv<'a> {
    defs: &'a DefCollection,
    comptime: &'a ComptimeCheck,
    program_defs: &'a HashMap<ModuleId, DefCollection>,
    program_comptime: &'a HashMap<ModuleId, ComptimeCheck>,
}

impl ComptimeCommonEnv for StaticComptimeEnv<'_> {}

impl ResolvedComptimeEnv for StaticComptimeEnv<'_> {
    fn resolve_resolved_name(
        &mut self,
        span: Span,
        resolution: nia_comptime_ir::ComptimeNameResolution,
    ) -> Result<ComptimeValue, ComptimeError> {
        let key = match resolution {
            nia_comptime_ir::ComptimeNameResolution::Local(local_id) => {
                ComptimeKey::Local(local_id)
            }
            nia_comptime_ir::ComptimeNameResolution::Global(global_id) => {
                if self.global_def_kind(global_id) != Some(DefKind::Comptime) {
                    return Err(ComptimeError {
                        span,
                        message: "static constant expression can only use comptime bindings"
                            .to_string(),
                    });
                }
                ComptimeKey::Global(global_id)
            }
        };
        self.value_for_key(key)
            .cloned()
            .ok_or_else(|| ComptimeError {
                span,
                message: "failed to evaluate comptime value".to_string(),
            })
    }

    fn resolve_resolved_layout_builtin(
        &mut self,
        span: Span,
        _builtin: nia_ids::LayoutBuiltin,
        _type_arg: &ResolvedComptimeTypeArg,
    ) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: "layout builtins are not available in static address constants".to_string(),
        })
    }
}

impl StaticComptimeEnv<'_> {
    fn value_for_key(&self, key: ComptimeKey) -> Option<&ComptimeValue> {
        match key {
            ComptimeKey::Local(_) => self.comptime.values.get(&key),
            ComptimeKey::Global(global_id) if global_id.module_id == self.defs.module_id => {
                self.comptime.values.get(&key)
            }
            ComptimeKey::Global(global_id) => self
                .program_comptime
                .get(&global_id.module_id)
                .and_then(|comptime| comptime.values.get(&key)),
        }
    }

    fn global_def_kind(&self, global_id: GlobalDefId) -> Option<DefKind> {
        if global_id.module_id == self.defs.module_id {
            return self.defs.defs.get(global_id.def_id).map(|def| def.kind);
        }
        self.program_defs
            .get(&global_id.module_id)
            .and_then(|defs| defs.defs.get(global_id.def_id))
            .map(|def| def.kind)
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
    use nia_type_normalize::normalize_module_types;
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
        let target = nia_target_config::TargetConfig::host();
        let normalization = normalize_module_types(module_id, &type_lowering.interner, &signatures);
        let comptime_module =
            nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
                module: &module,
                defs: &defs,
                values: &values,
                locals: &locals,
                type_uses: &type_lowering.type_uses,
                const_exprs: &type_lowering.const_exprs,
            });
        assert!(
            comptime_module.diagnostics.is_empty(),
            "{:?}",
            comptime_module.diagnostics
        );
        let comptime =
            nia_comptime_check::check_module_comptime(nia_comptime_check::ComptimeInput {
                module: &comptime_module.module,
                defs: &defs,
                values: &values,
                locals: &locals,
                signatures: &signatures,
                interner: &normalization.interner,
                type_uses: &type_lowering.type_uses,
                normalized: &normalization.normalized,
                target: &target,
                program: nia_comptime_check::ComptimeProgramContext::empty(),
            });
        assert!(
            comptime.diagnostics.is_empty(),
            "{:?}",
            comptime.diagnostics
        );
        check_module_static_initializers(
            &module,
            &defs,
            &values,
            &locals,
            &signatures,
            &comptime,
            &type_lowering.type_uses,
            &HashMap::new(),
            &HashMap::new(),
        )
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
    print: &fn(&i32)
}

fn print_i32(value: &i32) {}
let vtable: Vtable = { print: & print_i32 };
"#,
        );

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn accepts_static_integer_expression_from_comptime_value() {
        let checked = check(
            r#"
comptime let base = 20;
var value: i32 = base + 2;
"#,
        );

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn accepts_static_array_repeat_count_from_comptime_value() {
        let checked = check(
            r#"
comptime let n = 3;
var values: [3]i32 = [1; n];
"#,
        );

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn rejects_static_array_repeat_count_from_runtime_global() {
        let checked = check(
            r#"
var n: usize = 3;
var values: [3]i32 = [1; n];
"#,
        );

        assert!(
            checked.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("array repeat count is not a static usize constant")),
            "{:?}",
            checked.diagnostics
        );
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

    #[test]
    fn accepts_static_global_address_index_from_comptime_value() {
        let checked = check(
            r#"
comptime let idx = 1;
var target: [2]i32 = [1, 2];
var selected: &i32 = &target[idx];
"#,
        );

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }
}
