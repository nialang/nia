// SPDX-License-Identifier: GPL-3.0-or-later
//! Validation of module static initializers against backend-representable rules.
//!
//! Static checking intentionally sits between semantic resolution and static
//! IR construction: it rejects runtime-only operations while retaining enough
//! cross-module context to resolve addresses, layouts, and const values.
use std::sync::Arc;

use nia_ast::{ArrayElements, BindingItem, Block, Expr, ExprKind, IndexArg, StmtKind, UnaryOp};
use nia_const_check::{ConstKey, ConstValues};
use nia_const_eval::{ConstCommonEnv, ConstError, ConstValue, ResolvedConstEnv};
use nia_const_ir::{ResolvedConstExpr, ResolvedConstTypeArg};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, ModuleId};
use nia_item_signatures::{GlobalSignature, ItemSignatures};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
use nia_local_resolve::{LocalKind, LocalResolution, LocalUse};
use nia_sema_ir::{BuiltinAssociatedValue, SemanticUseTable, SemanticValueUse};
use nia_span::Span;
use nia_symbol::{SymbolId, symbol_text_or_unresolved};
use nia_symbol_table::SymbolTable;
use nia_target_config::TargetConfig;
use nia_value_resolve::{ValueNameResolution, ValueResolution};

#[derive(Debug, Clone, PartialEq)]
/// Diagnostics produced while checking one module's static initializers.
pub struct StaticCheck {
    /// Ordered user-facing diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Shared semantic inputs for the compatibility static-check entry point.
pub struct StaticCheckInput<'a> {
    /// Active declarations in the module.
    pub active_item_tree: &'a ActiveModuleItemTree,
    /// Definition table for local and imported items.
    pub defs: &'a DefCollection,
    /// Resolved value names and qualified references.
    pub values: &'a ValueResolution,
    /// Resolved local uses and binding modes.
    pub locals: &'a LocalResolution,
    /// Semantic use facts for associated values and calls.
    pub semantic_uses: &'a SemanticUseTable,
    /// Symbol interner used for diagnostics.
    pub symbols: &'a SymbolTable,
    /// Global signatures used for initializer typing.
    pub signatures: &'a ItemSignatures,
    /// Evaluated const values available to this module.
    pub const_eval: &'a ConstValues,
    /// Cross-module definition lookup.
    pub program_defs: &'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>,
    /// Cross-module const-value lookup.
    pub program_const: &'a dyn Fn(ModuleId) -> Option<Arc<ConstValues>>,
    /// Target width/alignment configuration.
    pub target: &'a TargetConfig,
}

/// Narrow signature view used by precise static checking.
#[derive(Debug, Clone, Copy)]
pub struct StaticCheckSignatures<'a> {
    /// Global signatures keyed by local definition id.
    pub globals: &'a std::collections::HashMap<DefId, GlobalSignature>,
}

/// Checks static initializers using the module's complete signature table.
pub fn check_module_static_initializers(input: StaticCheckInput<'_>) -> StaticCheck {
    check_module_static_initializers_with_signatures(StaticCheckPreciseInput {
        active_item_tree: input.active_item_tree,
        defs: input.defs,
        values: input.values,
        locals: input.locals,
        semantic_uses: input.semantic_uses,
        symbols: input.symbols,
        signatures: StaticCheckSignatures {
            globals: &input.signatures.globals,
        },
        const_eval: input.const_eval,
        program_defs: input.program_defs,
        program_const: input.program_const,
        target: input.target,
    })
}

/// Inputs for the precise static-check path with explicit global signatures.
pub struct StaticCheckPreciseInput<'a> {
    /// Active declarations in the module.
    pub active_item_tree: &'a ActiveModuleItemTree,
    /// Definition table for local and imported items.
    pub defs: &'a DefCollection,
    /// Resolved value names and qualified references.
    pub values: &'a ValueResolution,
    /// Resolved local uses and binding modes.
    pub locals: &'a LocalResolution,
    /// Semantic use facts for associated values and calls.
    pub semantic_uses: &'a SemanticUseTable,
    /// Symbol interner used for diagnostics.
    pub symbols: &'a SymbolTable,
    /// Global signatures used for initializer typing.
    pub signatures: StaticCheckSignatures<'a>,
    /// Evaluated const values available to this module.
    pub const_eval: &'a ConstValues,
    /// Cross-module definition lookup.
    pub program_defs: &'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>,
    /// Cross-module const-value lookup.
    pub program_const: &'a dyn Fn(ModuleId) -> Option<Arc<ConstValues>>,
    /// Target width/alignment configuration.
    pub target: &'a TargetConfig,
}

/// Checks the subset of initializers that can be emitted as linker/static data.
///
/// This is deliberately narrower than Nia's `const` evaluator: `const` means
/// comptime evaluation, while `static` must have a concrete backend
/// representation and cannot run arbitrary code during startup.
pub fn check_module_static_initializers_with_signatures(
    input: StaticCheckPreciseInput<'_>,
) -> StaticCheck {
    let mut checker = StaticChecker {
        defs: input.defs,
        values: input.values,
        locals: input.locals,
        semantic_uses: input.semantic_uses,
        symbols: input.symbols,
        signatures: input.signatures,
        const_eval: input.const_eval,
        program_defs: input.program_defs,
        program_const: input.program_const,
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
    symbols: &'a SymbolTable,
    signatures: StaticCheckSignatures<'a>,
    const_eval: &'a ConstValues,
    program_defs: &'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>,
    program_const: &'a dyn Fn(ModuleId) -> Option<Arc<ConstValues>>,
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
        // Walk active declarations only: inactive/forked module items must not
        // produce diagnostics or static products for the current revision.
        for item in &item_tree.items {
            match &item.kind {
                ItemTreeNodeKind::Binding(binding) if !binding.is_const() => {
                    self.check_global_binding(item.span, binding);
                }
                ItemTreeNodeKind::Function(function) => {
                    if let Some(body) = &function.body {
                        self.check_block_static_bindings(body);
                    }
                }
                ItemTreeNodeKind::Trait(item_trait) => {
                    for method in &item_trait.methods {
                        if let Some(body) = &method.function.body {
                            self.check_block_static_bindings(body);
                        }
                    }
                }
                ItemTreeNodeKind::Extend(extend) => {
                    for method in &extend.methods {
                        if let Some(body) = &method.function.body {
                            self.check_block_static_bindings(body);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn check_block_static_bindings(&mut self, block: &Block) {
        nia_ast_walk::walk_static_bindings(block, &mut |stmt| {
            if let StmtKind::Static(binding) = &stmt.kind {
                self.check_global_binding(stmt.span, binding);
            }
        });
    }

    fn check_global_binding(&mut self, span: Span, binding: &BindingItem) {
        // Static initializers are checked in declaration context, then lowered
        // through the same const-eval environment used for resolved globals.
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
            ExprKind::Tuple(elems) => elems
                .iter()
                .find_map(|elem| self.static_init_reject_reason(elem)),
            ExprKind::Closure { .. } => Some("closure values require runtime state"),
            ExprKind::ArrayLiteral { elems } => match elems {
                ArrayElements::List(elems) => elems
                    .iter()
                    .find_map(|elem| self.static_init_reject_reason(elem)),
                ArrayElements::Repeat { value, count } => self
                    .static_init_reject_reason(value)
                    .or_else(|| self.static_array_repeat_count_reject_reason(count)),
            },
            ExprKind::TypedStructLiteral { fields, .. }
            | ExprKind::QualifiedStructLiteral { fields, .. } => fields
                .iter()
                .find_map(|field| self.static_init_reject_reason(&field.value)),
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
            ExprKind::TypeTarget { .. } | ExprKind::TraitTarget { .. } => {
                Some("type target is not static data")
            }
            ExprKind::SelfValue => Some("self value is not available in global storage"),
            ExprKind::PathRoot(_) => Some("path root is not static data"),
            ExprKind::Ident(_) => match self.local_use(expr) {
                Some(LocalUse::ModuleValue) => match self.value_name(expr) {
                    Some(ValueNameResolution::Def(def_id)) if self.is_enum_variant(def_id) => None,
                    Some(ValueNameResolution::Def(def_id)) if self.is_const(def_id) => {
                        if self.global_const_integer(def_id) {
                            None
                        } else {
                            Some("const value is not representable as static initializer data")
                        }
                    }
                    Some(ValueNameResolution::External(global_id))
                        if self.global_def_kind(global_id) == Some(DefKind::Const) =>
                    {
                        if self.global_const_integer_id(global_id) {
                            None
                        } else {
                            Some("const value is not representable as static initializer data")
                        }
                    }
                    _ => Some("bare global value is not static data; take its address explicitly"),
                },
                Some(LocalUse::Unresolved) | None => None,
                Some(LocalUse::Local(local_id)) => {
                    if self.local_const_integer(local_id) {
                        None
                    } else if self
                        .locals
                        .locals
                        .get(local_id)
                        .is_some_and(|local| local.kind == LocalKind::ConstBinding)
                    {
                        Some("const value is not representable as static initializer data")
                    } else {
                        Some("local value is not available in global storage")
                    }
                }
                Some(LocalUse::Static(_)) => {
                    Some("bare global value is not static data; take its address explicitly")
                }
                Some(LocalUse::Module) => Some("module namespace is not static data"),
                Some(LocalUse::TypePrefix) => Some("type prefix is not static data"),
            },
            ExprKind::Qualified { lhs, name: _ } => {
                if self.is_enum_variant_access(expr, lhs) {
                    None
                } else if let Some(global_id) = self.qualified_value(expr)
                    && self.global_def_kind(global_id) == Some(DefKind::Const)
                {
                    self.global_const_integer_id(global_id)
                        .then_some(())
                        .map_or(
                            Some("const value is not representable as static initializer data"),
                            |_| None,
                        )
                } else {
                    self.static_address_path_reject_reason(expr)
                }
            }
            ExprKind::Field { lhs, .. } | ExprKind::TupleField { lhs, .. } => {
                self.static_address_path_reject_reason(lhs)
            }
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
            ExprKind::Block(_) => Some("block expressions require const execution"),
            ExprKind::If { .. } => Some("if expressions require const execution"),
            ExprKind::IfPattern(_) => Some("if pattern expressions require const execution"),
            ExprKind::Match(_) => Some("match expressions require const execution"),
            ExprKind::Call { .. } => Some("function calls require const execution"),
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
                Some(LocalUse::Static(_)) => None,
                Some(LocalUse::Module) => Some("module namespace has no address"),
                Some(LocalUse::TypePrefix) => Some("type prefix has no address"),
            },
            _ => Some("address target is not global storage"),
        }
    }

    fn static_address_path_reject_reason(&self, expr: &Expr) -> Option<&'static str> {
        // Address initializers are symbolic relocations, not evaluated pointer
        // expressions. Walk only projections from global storage and require
        // every index to be a compile-time usize; runtime pointer arithmetic
        // would make the initializer depend on startup execution.
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
            ExprKind::TypeTarget { .. } | ExprKind::TraitTarget { .. } => None,
            ExprKind::Field { lhs, .. } | ExprKind::TupleField { lhs, .. } => {
                self.static_address_path_reject_reason(lhs)
            }
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
            ExprKind::TypeTarget { .. } | ExprKind::TraitTarget { .. } => true,
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

    fn eval_static_array_index(&self, expr: &Expr) -> Result<u64, nia_const_eval::ConstError> {
        self.eval_static_resolved_expr(expr, |expr, env| {
            nia_const_eval::eval_resolved_const_array_len_expr(expr, env)
        })
    }

    fn static_array_repeat_count_reject_reason(&self, expr: &Expr) -> Option<&'static str> {
        self.eval_static_array_index(expr)
            .err()
            .map(|_| "array repeat count is not a static usize constant")
    }

    fn eval_static_int_expr(&self, expr: &Expr) -> Result<i128, nia_const_eval::ConstError> {
        self.eval_static_resolved_expr(expr, |expr, env| {
            nia_const_eval::eval_resolved_const_int_expr(expr, env).and_then(|value| {
                value.as_i128().ok_or_else(|| nia_const_eval::ConstError {
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
            &ResolvedConstExpr,
            &mut StaticConstEnv<'_>,
        ) -> Result<T, nia_const_eval::ConstError>,
    ) -> Result<T, nia_const_eval::ConstError> {
        let semantic_uses = self.const_semantic_uses();
        let context =
            nia_const_ir::ResolvedConstLowerInputs::new(&semantic_uses).with_symbols(self.symbols);
        let mut env = StaticConstEnv {
            defs: self.defs,
            const_eval: self.const_eval,
            program_defs: self.program_defs,
            program_const: self.program_const,
            symbols: self.symbols,
            target: self.target,
            budget: nia_const_eval::ConstEvalBudget::default(),
        };
        let expr =
            nia_const_ir::lower_expr_resolved_with_context(expr, &context).map_err(|err| {
                nia_const_eval::ConstError {
                    span: err.span,
                    message: err.message,
                }
            })?;
        eval(&expr, &mut env)
    }

    fn const_semantic_uses(&self) -> SemanticUseTable {
        let mut builder = SemanticUseTable::builder();
        for (key, value_use) in &self.semantic_uses.node_value_uses {
            match value_use {
                SemanticValueUse::Local(local_id)
                    if self
                        .const_eval
                        .values
                        .contains_key(&ConstKey::Local(*local_id)) =>
                {
                    builder.insert_node_local_value_use(key.clone(), *local_id);
                }
                SemanticValueUse::Global(global_id)
                    if self.global_const_id(*global_id) == Some(*global_id) =>
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
        builder.extend_node_type_prefixes(
            self.semantic_uses
                .node_type_prefixes
                .iter()
                .map(|(key, def_id)| (key.clone(), *def_id)),
        );
        builder.finish()
    }

    fn global_const_id(&self, global_id: GlobalDefId) -> Option<GlobalDefId> {
        (self.global_def_kind(global_id) == Some(DefKind::Const)).then_some(global_id)
    }

    fn global_def_kind(&self, global_id: GlobalDefId) -> Option<DefKind> {
        if global_id.module_id == self.defs.module_id {
            return self.defs.defs.get(global_id.def_id).map(|def| def.kind);
        }
        (self.program_defs)(global_id.module_id)
            .and_then(|defs| defs.defs.get(global_id.def_id).map(|def| def.kind))
    }

    fn is_global(&self, def_id: DefId) -> bool {
        matches!(
            self.defs.defs.get(def_id).map(|def| def.kind),
            Some(DefKind::Global)
        )
    }

    fn is_const(&self, def_id: DefId) -> bool {
        matches!(
            self.defs.defs.get(def_id).map(|def| def.kind),
            Some(DefKind::Const)
        )
    }

    fn local_const_integer(&self, local_id: nia_ids::LocalId) -> bool {
        matches!(
            self.const_eval.values.get(&ConstKey::Local(local_id)),
            Some(ConstValue::Int(_))
        )
    }

    fn global_const_integer(&self, def_id: DefId) -> bool {
        self.global_const_integer_id(GlobalDefId {
            module_id: self.defs.module_id,
            def_id,
        })
    }

    fn global_const_integer_id(&self, global_id: GlobalDefId) -> bool {
        matches!(self.const_value(global_id), Some(ConstValue::Int(_)))
    }

    fn const_value(&self, global_id: GlobalDefId) -> Option<ConstValue> {
        let key = ConstKey::Global(global_id);
        if global_id.module_id == self.defs.module_id {
            return self.const_eval.values.get(&key).cloned();
        }
        (self.program_const)(global_id.module_id)
            .and_then(|const_eval| const_eval.values.get(&key).cloned())
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

struct StaticConstEnv<'a> {
    defs: &'a DefCollection,
    const_eval: &'a ConstValues,
    program_defs: &'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>,
    program_const: &'a dyn Fn(ModuleId) -> Option<Arc<ConstValues>>,
    symbols: &'a SymbolTable,
    target: &'a TargetConfig,
    budget: nia_const_eval::ConstEvalBudget,
}

impl ConstCommonEnv for StaticConstEnv<'_> {
    fn begin_const_eval(&mut self) {
        self.budget.begin_session();
    }

    fn end_const_eval(&mut self) {
        self.budget.end_session();
    }

    fn consume_const_eval_step(&mut self, span: Span) -> Result<(), ConstError> {
        // Every recursive/static expression consumes the shared budget so a
        // cyclic or adversarial initializer cannot monopolize compilation.
        self.budget.consume_step(span)
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        StaticConstEnv::symbol_name(self, symbol)
    }

    fn is_enum_variant(&self, def_id: GlobalDefId) -> bool {
        self.global_def_kind(def_id) == Some(DefKind::EnumVariant)
    }
}

impl ResolvedConstEnv for StaticConstEnv<'_> {
    fn resolve_resolved_name(
        &mut self,
        span: Span,
        resolution: nia_const_ir::ConstNameResolution,
    ) -> Result<ConstValue, ConstError> {
        let key = match resolution {
            nia_const_ir::ConstNameResolution::Local(local_id) => ConstKey::Local(local_id),
            nia_const_ir::ConstNameResolution::Global(global_id) => {
                if self.global_def_kind(global_id) != Some(DefKind::Const) {
                    return Err(ConstError {
                        span,
                        message: "static constant expression can only use const bindings"
                            .to_string(),
                    });
                }
                ConstKey::Global(global_id)
            }
            nia_const_ir::ConstNameResolution::BuiltinAssociatedValue(value) => {
                let BuiltinAssociatedValue::PrimitiveIntLimit { primitive, kind } = value;
                let Some(value) = kind.value(primitive, self.target.pointer_width) else {
                    return Err(ConstError {
                        span,
                        message: "builtin associated value is not representable at const"
                            .to_string(),
                    });
                };
                return Ok(ConstValue::Int(value));
            }
            nia_const_ir::ConstNameResolution::GenericParam(name) => {
                return Err(ConstError {
                    span,
                    message: format!(
                        "static constant expression cannot use unresolved const generic parameter `{}`",
                        self.symbol_name(name)
                    ),
                });
            }
            nia_const_ir::ConstNameResolution::AssociatedConstProjection(projection) => {
                return Err(ConstError {
                    span,
                    message: format!(
                        "static constant expression cannot use unresolved associated const value `{}`",
                        self.symbol_name(projection.name)
                    ),
                });
            }
        };
        self.value_for_key(key).ok_or_else(|| ConstError {
            span,
            message: "failed to evaluate const value".to_string(),
        })
    }

    fn resolve_resolved_layout_builtin(
        &mut self,
        span: Span,
        _builtin: nia_ids::LayoutBuiltin,
        _type_arg: &ResolvedConstTypeArg,
    ) -> Result<ConstValue, ConstError> {
        Err(ConstError {
            span,
            message: "layout builtins are not available in static address constants".to_string(),
        })
    }

    fn resolve_resolved_field_offset_builtin(
        &mut self,
        span: Span,
        _type_arg: &ResolvedConstTypeArg,
        _field: &SymbolId,
    ) -> Result<ConstValue, ConstError> {
        Err(ConstError {
            span,
            message: "field offset builtins are not available in static address constants"
                .to_string(),
        })
    }
}

impl StaticConstEnv<'_> {
    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_or_unresolved(self.symbols, symbol)
    }

    fn value_for_key(&self, key: ConstKey) -> Option<ConstValue> {
        match key {
            ConstKey::Local(_) => self.const_eval.values.get(&key).cloned(),
            ConstKey::Global(global_id) if global_id.module_id == self.defs.module_id => {
                self.const_eval.values.get(&key).cloned()
            }
            ConstKey::Global(global_id) => (self.program_const)(global_id.module_id)
                .and_then(|const_eval| const_eval.values.get(&key).cloned()),
        }
    }

    fn global_def_kind(&self, global_id: GlobalDefId) -> Option<DefKind> {
        if global_id.module_id == self.defs.module_id {
            return self.defs.defs.get(global_id.def_id).map(|def| def.kind);
        }
        (self.program_defs)(global_id.module_id)
            .and_then(|defs| defs.defs.get(global_id.def_id).map(|def| def.kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::collect_module_defs;
    use nia_ids::ModuleIdAllocator;
    use nia_item_signatures::{ItemSignatureInput, ItemSignatureSource, collect_item_signatures};
    use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
    use nia_local_resolve::resolve_module_locals;
    use nia_parser::parse_module_with_symbols;
    use nia_sema_ir::SemanticUseTable;
    use nia_source::SourcePath;
    use nia_symbol_table::SymbolTable;
    use nia_type_lower::{TypeLoweringContext, lower_module_types_with_context};
    use nia_type_normalize::normalize_module_types;
    use nia_type_resolve::resolve_module_types_with_symbols;
    use nia_value_resolve::resolve_module_values;

    fn check(source: &str) -> StaticCheck {
        let symbols = SymbolTable::new();
        let (module, errors) = parse_module_with_symbols(source, symbols.clone());
        assert!(errors.is_empty(), "{errors:?}");
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let defs = collect_module_defs(module_id, &module);
        let type_resolution = resolve_module_types_with_symbols(&module, &defs, &symbols);
        let type_store = nia_ty::TypeStore::new();
        let type_lowering = lower_module_types_with_context(
            module_id,
            &module,
            &type_resolution,
            TypeLoweringContext::empty(&type_store),
        );
        let signatures = collect_item_signatures(ItemSignatureInput {
            source: ItemSignatureSource::Module(&module),
            defs: &defs,
            lowered: &type_lowering,
            type_store: &type_store,
            symbols: None,
        });
        let values = resolve_module_values(&module, &defs);
        let locals = resolve_module_locals(&module, &defs, &values);
        let item_tree = ModuleItemTree::from_module(&module);
        let active_item_tree =
            ActiveModuleItemTree::new(item_tree.active_items_without_const(), Default::default());
        let semantic_uses = semantic_use_table(
            module_id,
            &values,
            &locals,
            &type_lowering,
            &active_item_tree,
        );
        let target = nia_target_config::TargetConfig::host();
        let source_path = SourcePath::new("/tmp/nia-static-check-test/main.nia");
        let normalization_input = type_lowering.explicit_type_roots();
        let normalization = normalize_module_types(nia_type_normalize::TypeNormalizationInput {
            module_id,
            type_store: &type_store,
            input_ids: &normalization_input,
            signatures: &signatures,
        });
        let const_module = nia_const_check::lower_module_const(nia_const_check::ConstModuleInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            signatures: &signatures,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            symbols: &symbols,
            const_exprs: &type_lowering.const_exprs,
            source_path: &source_path,
        });
        assert!(
            const_module.diagnostics.is_empty(),
            "{:?}",
            const_module.diagnostics
        );
        let const_input = nia_const_check::ConstInput {
            type_store: &type_store,
            module: &const_module.module,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            symbols: &symbols,
            lowered: &type_lowering,
            signatures: &signatures,
            normalization: &normalization,
            target: &target,
            source_path: &source_path,
            program: nia_const_check::ConstProgramContext::empty(),
        };
        let array_lengths = nia_const_check::compute_module_const_array_lengths(const_input);
        let enum_values =
            nia_const_check::compute_module_const_enum_values(const_input, array_lengths.clone());
        let const_eval =
            nia_const_check::compute_module_const_values(const_input, array_lengths, enum_values);
        assert!(
            const_eval.diagnostics.is_empty(),
            "{:?}",
            const_eval.diagnostics
        );
        let no_program_const = |_| None;
        let no_program_defs = |_| None;
        check_module_static_initializers(StaticCheckInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            symbols: &symbols,
            signatures: &signatures,
            const_eval: &const_eval,
            program_defs: &no_program_defs,
            program_const: &no_program_const,
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

static mut base: i32 = 1;
static mut bad_block = { 1 };
static mut bad_call = make();
static mut bad_bare_ptr: &i32 = base;
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

static mut base: i32 = 1 + 2;
static mut pair: Pair = Pair { x: 1, y: 2 };
static mut xs: [i32; 2] = [1, 2];
static mut p: &i32 = &base;
static mut q: &i32 = &pair.x;
static mut r: &i32 = &xs[1];

struct Vtable {
    print: &fn(&i32)
}

fn print_i32(value: &i32) {}
static vtable: Vtable = Vtable { print: & print_i32 };
"#,
        );

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn accepts_static_integer_expression_from_const_value() {
        let checked = check(
            r#"
const base = 20;
static mut value: i32 = base + 2;
"#,
        );

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn accepts_local_integer_const_in_static_initializer() {
        let checked = check(
            r#"
fn make() i32 {
    const base = 20;
    static mut value: i32 = base + 2;
    value
}
"#,
        );

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn rejects_aggregate_const_in_static_initializer_until_static_ir_supports_it() {
        let checked = check(
            r#"
const values: [i32; 2] = [1, 2];
static mut copy: [i32; 2] = values;
"#,
        );

        assert!(
            checked.diagnostics.iter().any(|diagnostic| diagnostic
                .summary
                .contains("const value is not representable as static initializer data")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn checks_static_nested_in_expression_blocks() {
        let checked = check(
            r#"
fn make() i32 { 1 }

fn main() i32 {
    if true {
        static bad: i32 = make();
        bad
    } else {
        0
    }
}
"#,
        );

        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("function calls")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn accepts_static_array_repeat_count_from_const_value() {
        let checked = check(
            r#"
const n = 3;
static mut values: [i32; 3] = [1; n];
"#,
        );

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn rejects_static_array_repeat_count_from_runtime_global() {
        let checked = check(
            r#"
static mut n: usize = 3;
static mut values: [i32; 3] = [1; n];
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
static mut target: [i32; 2] = [1, 2];
static mut idx: i32 = 1;
static mut bad: &i32 = &target[idx];
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
    fn accepts_static_global_address_index_from_const_value() {
        let checked = check(
            r#"
const idx = 1;
static mut target: [i32; 2] = [1, 2];
static mut selected: &i32 = &target[idx];
"#,
        );

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }
}
