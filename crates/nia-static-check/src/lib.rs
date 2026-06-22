// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{ArrayElements, BindingItem, Expr, ExprKind, IndexArg, UnaryOp};
use nia_comptime_check::{ComptimeCheck, ComptimeKey};
use nia_comptime_engine::{ComptimeCommonEnv, ComptimeError, ComptimeValue, ResolvedComptimeEnv};
use nia_comptime_ir::{ResolvedComptimeExpr, ResolvedComptimeTypeArg};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, ModuleId};
use nia_item_signatures::ItemSignatures;
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
use nia_local_resolve::{LocalResolution, LocalUse};
use nia_sema_ir::{BuiltinAssociatedValue, SemanticUseTable, SemanticValueUse};
use nia_span::Span;
use nia_target_config::TargetConfig;
use nia_value_resolve::{ValueNameResolution, ValueResolution};

#[derive(Debug, Clone, PartialEq)]
pub struct StaticCheck {
    pub diagnostics: Vec<Diagnostic>,
}

pub struct StaticCheckInput<'a> {
    pub active_item_tree: &'a ActiveModuleItemTree,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub semantic_uses: &'a SemanticUseTable,
    pub signatures: &'a ItemSignatures,
    pub comptime: &'a ComptimeCheck,
    pub program_defs: &'a HashMap<ModuleId, DefCollection>,
    pub program_comptime: &'a HashMap<ModuleId, ComptimeCheck>,
    pub target: &'a TargetConfig,
}

pub fn check_module_static_initializers(input: StaticCheckInput<'_>) -> StaticCheck {
    let mut checker = StaticChecker {
        defs: input.defs,
        values: input.values,
        locals: input.locals,
        semantic_uses: input.semantic_uses,
        signatures: input.signatures,
        comptime: input.comptime,
        program_defs: input.program_defs,
        program_comptime: input.program_comptime,
        target: input.target,
        diagnostics: Vec::new(),
    };
    checker.check_active_module(input.active_item_tree);
    StaticCheck {
        diagnostics: checker.diagnostics,
    }
}

struct StaticChecker<'a> {
    defs: &'a DefCollection,
    values: &'a ValueResolution,
    locals: &'a LocalResolution,
    semantic_uses: &'a SemanticUseTable,
    signatures: &'a ItemSignatures,
    comptime: &'a ComptimeCheck,
    program_defs: &'a HashMap<ModuleId, DefCollection>,
    program_comptime: &'a HashMap<ModuleId, ComptimeCheck>,
    target: &'a TargetConfig,
    diagnostics: Vec<Diagnostic>,
}

impl StaticChecker<'_> {
    fn local_use(&self, expr: &Expr) -> Option<LocalUse> {
        self.locals.node_uses.get(&expr.node_key).copied()
    }

    fn value_name(&self, expr: &Expr) -> Option<ValueNameResolution> {
        self.values.node_names.get(&expr.node_key).copied()
    }

    fn qualified_value(&self, expr: &Expr) -> Option<GlobalDefId> {
        self.values
            .node_qualified_values
            .get(&expr.node_key)
            .copied()
    }

    fn qualified_type_prefix(&self, expr: &Expr) -> Option<GlobalDefId> {
        self.values
            .node_qualified_type_prefixes
            .get(&expr.node_key)
            .copied()
    }

    fn check_active_module(&mut self, item_tree: &ActiveModuleItemTree) {
        for item in &item_tree.items {
            if let ItemTreeNodeKind::Binding(binding) = &item.kind
                && !binding.is_comptime
            {
                self.check_global_binding(item.span, binding);
            }
        }
    }

    fn check_global_binding(&mut self, span: Span, binding: &BindingItem) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, span, DefKind::Global) else {
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
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
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
                UnaryOp::Deref if Self::is_string_family_literal(inner) => None,
                UnaryOp::Not | UnaryOp::BitNot | UnaryOp::Deref => {
                    Some("unsupported unary operator")
                }
            },
            ExprKind::Binary { .. } => self.static_int_expr_reject_reason(expr),
            ExprKind::Cast { expr: inner, .. } => self.static_init_reject_reason(inner),
            ExprKind::Builtin { .. } => None,
            ExprKind::TypeTarget { .. } => Some("type target is not static data"),
            ExprKind::Ident(_) => match self.local_use(expr) {
                Some(LocalUse::ModuleValue) => match self.value_name(expr) {
                    Some(ValueNameResolution::Def(def_id)) if self.is_enum_variant(def_id) => None,
                    Some(ValueNameResolution::Def(def_id)) if self.is_comptime(def_id) => None,
                    _ => Some("bare global value is not static data; take its address explicitly"),
                },
                Some(LocalUse::Unresolved) | None => None,
                Some(LocalUse::Local(_)) => Some("local value is not available in global storage"),
                Some(LocalUse::Module) => Some("module namespace is not static data"),
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
            ExprKind::IfPattern(_) => Some("if pattern expressions require comptime execution"),
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

    fn is_string_family_literal(expr: &Expr) -> bool {
        matches!(expr.kind, ExprKind::String(_) | ExprKind::ByteString(_))
    }

    fn static_address_reject_reason(&self, expr: &Expr) -> Option<&'static str> {
        if self.qualified_value(expr).is_some() {
            return None;
        }
        match &expr.kind {
            ExprKind::Ident(_) => match self.local_use(expr) {
                Some(LocalUse::ModuleValue) => match self.value_name(expr) {
                    Some(ValueNameResolution::Def(def_id))
                        if self.is_global(def_id) || self.is_function(def_id) =>
                    {
                        None
                    }
                    Some(ValueNameResolution::Def(_)) => Some("address target is not static"),
                    _ => None,
                },
                Some(LocalUse::Unresolved) | None => None,
                Some(LocalUse::Local(_)) => Some("address target is local storage"),
                Some(LocalUse::Module) => Some("module namespace has no address"),
                Some(LocalUse::TypePrefix) => Some("type prefix has no address"),
            },
            _ => Some("address target is not global storage"),
        }
    }

    fn static_address_path_reject_reason(&self, expr: &Expr) -> Option<&'static str> {
        if self.qualified_value(expr).is_some() {
            return None;
        }
        if self.qualified_type_prefix(expr).is_some() {
            return None;
        }
        match &expr.kind {
            ExprKind::Qualified { lhs, .. } => {
                if self.qualified_type_prefix(expr).is_some() {
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
            ExprKind::Ident(_) => matches!(self.local_use(expr), Some(LocalUse::TypePrefix)),
            ExprKind::Qualified { .. } => self.qualified_type_prefix(expr).is_some(),
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
            nia_comptime_engine::eval_resolved_comptime_int_expr(expr, env).and_then(|value| {
                value
                    .as_i128()
                    .ok_or_else(|| nia_comptime_engine::ComptimeError {
                        span: expr.span(),
                        message: "static integer expression is out of range".to_string(),
                    })
            })
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
        let semantic_uses = self.comptime_semantic_uses();
        let context = nia_comptime_ir::ResolvedComptimeLowerInputs::new(&semantic_uses);
        let mut env = StaticComptimeEnv {
            defs: self.defs,
            comptime: self.comptime,
            program_defs: self.program_defs,
            program_comptime: self.program_comptime,
            target: self.target,
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

    fn comptime_semantic_uses(&self) -> SemanticUseTable {
        let mut builder = SemanticUseTable::builder();
        for (key, value_use) in &self.semantic_uses.node_value_uses {
            match value_use {
                SemanticValueUse::Local(local_id)
                    if self
                        .comptime
                        .values
                        .contains_key(&ComptimeKey::Local(*local_id)) =>
                {
                    builder.insert_node_local_value_use(key.clone(), *local_id);
                }
                SemanticValueUse::Global(global_id)
                    if self.global_comptime_id(*global_id) == Some(*global_id) =>
                {
                    builder.insert_node_global_value_use(key.clone(), *global_id);
                }
                SemanticValueUse::Local(_) | SemanticValueUse::Global(_) => {}
            }
        }
        builder.extend_node_local_defs(
            self.semantic_uses
                .node_local_defs
                .iter()
                .map(|(key, local_id)| (key.clone(), *local_id)),
        );
        builder.extend_node_type_uses(
            self.semantic_uses
                .node_type_uses
                .iter()
                .map(|(key, ty)| (key.clone(), *ty)),
        );
        builder.finish()
    }

    fn global_comptime_id(&self, global_id: GlobalDefId) -> Option<GlobalDefId> {
        (self.global_def_kind(global_id) == Some(DefKind::Comptime)).then_some(global_id)
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
            self.value_name(expr),
            Some(ValueNameResolution::Def(def_id)) if self.is_enum_variant(def_id)
        ) || matches!(
            self.qualified_value(expr),
            Some(def_id) if self.is_enum_variant(def_id.def_id)
        ) || matches!(self.local_use(lhs), Some(LocalUse::TypePrefix))
    }

    fn def_id_for_node(
        &self,
        node_key: &nia_node_id::VersionedNodeKey,
        _span: Span,
        expected: DefKind,
    ) -> Option<DefId> {
        let def_id = self.defs.def_nodes.get(node_key)?;
        let def = self.defs.defs.get(def_id)?;
        (def.kind == expected).then_some(def_id)
    }
}

struct StaticComptimeEnv<'a> {
    defs: &'a DefCollection,
    comptime: &'a ComptimeCheck,
    program_defs: &'a HashMap<ModuleId, DefCollection>,
    program_comptime: &'a HashMap<ModuleId, ComptimeCheck>,
    target: &'a TargetConfig,
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
            nia_comptime_ir::ComptimeNameResolution::BuiltinAssociatedValue(value) => {
                let BuiltinAssociatedValue::PrimitiveIntLimit { primitive, kind } = value;
                let Some(value) = kind.value(primitive, self.target.pointer_width) else {
                    return Err(ComptimeError {
                        span,
                        message: "builtin associated value is not representable at comptime"
                            .to_string(),
                    });
                };
                return Ok(ComptimeValue::Int(value));
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

    fn resolve_resolved_field_offset_builtin(
        &mut self,
        span: Span,
        _type_arg: &ResolvedComptimeTypeArg,
        _field: &str,
    ) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: "field offset builtins are not available in static address constants"
                .to_string(),
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
    use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
    use nia_local_resolve::resolve_module_locals;
    use nia_parser::parse_module;
    use nia_sema_ir::SemanticUseTable;
    use nia_source::SourcePath;
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
        let item_tree = ModuleItemTree::from_module(&module);
        let active_item_tree = ActiveModuleItemTree::new(
            item_tree.active_items_without_comptime(),
            Default::default(),
        );
        let semantic_uses = semantic_use_table(
            module_id,
            &values,
            &locals,
            &type_lowering,
            &active_item_tree,
        );
        let target = nia_target_config::TargetConfig::host();
        let source_path = SourcePath::new("/tmp/nia-static-check-test/main.nia");
        let normalization = normalize_module_types(module_id, &type_lowering.interner, &signatures);
        let comptime_module =
            nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
                active_item_tree: &active_item_tree,
                defs: &defs,
                values: &values,
                locals: &locals,
                semantic_uses: &semantic_uses,
                const_exprs: &type_lowering.const_exprs,
                source_path: &source_path,
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
                semantic_uses: &semantic_uses,
                signatures: &signatures,
                interner: &normalization.interner,
                normalized: &normalization.normalized,
                target: &target,
                source_path: &source_path,
                program: nia_comptime_check::ComptimeProgramContext::empty(),
            });
        assert!(
            comptime.diagnostics.is_empty(),
            "{:?}",
            comptime.diagnostics
        );
        check_module_static_initializers(StaticCheckInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            signatures: &signatures,
            comptime: &comptime,
            program_defs: &HashMap::new(),
            program_comptime: &HashMap::new(),
            target: &target,
        })
    }

    fn semantic_use_table(
        module_id: ModuleId,
        values: &nia_value_resolve::ValueResolution,
        locals: &nia_local_resolve::LocalResolution,
        type_lowering: &nia_type_lower::TypeLowering,
        active_item_tree: &ActiveModuleItemTree,
    ) -> SemanticUseTable {
        let mut builder = SemanticUseTable::builder();
        for (key, local_use) in &locals.node_uses {
            if let nia_local_resolve::LocalUse::Local(local_id) = local_use {
                builder.insert_node_local_value_use(key.clone(), *local_id);
            }
        }
        builder.extend_node_global_value_uses(
            values
                .node_qualified_values
                .iter()
                .map(|(key, global_id)| (key.clone(), *global_id)),
        );
        builder.extend_node_builtin_associated_values(
            values
                .node_builtin_associated_values
                .iter()
                .map(|(key, value)| (key.clone(), *value)),
        );
        for (key, resolution) in &values.node_names {
            match resolution {
                nia_value_resolve::ValueNameResolution::Def(def_id) => {
                    builder.insert_node_global_value_use(
                        key.clone(),
                        nia_ids::GlobalDefId {
                            module_id,
                            def_id: *def_id,
                        },
                    );
                }
                nia_value_resolve::ValueNameResolution::External(global_id) => {
                    builder.insert_node_global_value_use(key.clone(), *global_id);
                }
                nia_value_resolve::ValueNameResolution::Module
                | nia_value_resolve::ValueNameResolution::LocalDeferred
                | nia_value_resolve::ValueNameResolution::Error => {}
            }
        }
        builder.extend_node_local_defs(
            locals
                .node_local_defs
                .iter()
                .map(|(key, local_id)| (key.clone(), *local_id)),
        );
        builder.extend_node_type_uses(
            type_lowering.versioned_type_uses_from_active_item_tree(active_item_tree),
        );
        builder.finish()
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
                .any(|diagnostic| diagnostic.summary.contains("block expressions"))
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("function calls"))
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("bare global value"))
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
                .summary
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
                .any(|diagnostic| diagnostic.summary.contains("static integer constant")),
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
