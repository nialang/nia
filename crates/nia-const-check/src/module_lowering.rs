use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::{ConstKey, ConstModuleInput, ConstModuleLowering, declarations::ConstDeclarations};
use nia_ast::Expr;
use nia_const_ir::{
    ResolvedConstEnum, ResolvedConstEnumVariant, ResolvedConstExpr, ResolvedConstExprKind,
    ResolvedConstLocalInitializer, ResolvedConstModule,
};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, InternedTyId, LocalId};
use nia_item_signatures::{
    EnumVariantPayloadSignature, GenericParamSignatureKind, generic_argument_substitutions,
};
use nia_item_tree::{ItemTreeNode, ItemTreeNodeKind};
use nia_local_resolve::LocalKind;
use nia_node_id::VersionedNodeKey;
use nia_sema_ir::{SemanticUseTable, SemanticValueUse};
use nia_span::Span;
use nia_ty::TyKind;

type LocalTypeHints = HashMap<LocalId, (InternedTyId, Option<nia_ast::TypeRef>)>;
type ExpectedTypeHint<'a> = (Option<InternedTyId>, Option<&'a nia_ast::TypeRef>);

#[derive(Default)]
struct ConstructorMaps {
    aggregate_types: HashMap<VersionedNodeKey, InternedTyId>,
    omitted_members: HashMap<VersionedNodeKey, GlobalDefId>,
    omitted_associated_types: HashMap<VersionedNodeKey, InternedTyId>,
    /// Spans of omitted members whose context type is erroneous. The error
    /// that made it so is reported where it occurs, so such a member fails
    /// to lower without a diagnostic of its own.
    erroneous_members: HashSet<Span>,
}

/// Lowers active module const expressions into identity-resolved const IR.
///
/// The active item tree determines which definitions participate in the
/// result; unresolved names are retained as diagnostics instead of being
/// guessed from their source spelling.
pub fn lower_module_const(input: ConstModuleInput<'_>) -> nia_ice::IceResult<ConstModuleLowering> {
    let mut lowerer = ConstModuleLowerer {
        declarations: ConstDeclarations::collect(&input),
        hinting_globals: RefCell::new(HashSet::new()),
        input,
        module: ResolvedConstModule::default(),
        diagnostics: crate::ConstDiagnostics::default(),
        owner: None,
        normalization: RefCell::new(crate::TypeNormalization {
            normalized: HashMap::new(),
            diagnostics: Vec::new(),
            aliases: Default::default(),
        }),
        internal_error: RefCell::new(None),
    };
    lowerer.lower_module();
    if let Some(error) = lowerer.internal_error.into_inner() {
        return Err(error);
    }
    lowerer
        .diagnostics
        .extend(lowerer.normalization.into_inner().diagnostics);
    Ok(ConstModuleLowering {
        module: Arc::new(lowerer.module),
        diagnostics: lowerer.diagnostics,
    })
}
struct ConstModuleLowerer<'a> {
    input: ConstModuleInput<'a>,
    declarations: ConstDeclarations<'a>,
    /// Untyped const globals whose initializer is being hinted, so a cyclic
    /// initializer yields no hint instead of recursing.
    hinting_globals: RefCell<HashSet<DefId>>,
    module: ResolvedConstModule,
    diagnostics: crate::ConstDiagnostics,
    /// The initializer or function whose lowering is in progress; its
    /// lowering errors are static errors of it.
    owner: Option<crate::types::DiagnosticOwner>,
    normalization: RefCell<crate::TypeNormalization>,
    internal_error: RefCell<Option<nia_ice::Ice>>,
}

impl ConstModuleLowerer<'_> {
    fn type_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        if let Some(normalized) = self.normalization.borrow().normalized.get(&ty).copied() {
            return self.input.type_store.get(normalized);
        }
        // Const IR is also lowered while declaration signatures are being
        // discovered. Normalize from raw alias signatures, never by querying
        // another module's completed const or normalization products.
        let external_alias = |id: GlobalDefId| {
            (self.input.signatures_for_module?)(id.module_id)?
                .type_aliases
                .get(&id.def_id)
                .cloned()
        };
        match nia_type_normalize::normalize_module_types(
            nia_type_normalize::TypeNormalizationInput {
                module_id: self.input.defs.module_id,
                type_store: self.input.type_store,
                input_ids: &[ty],
                signatures: self.input.signatures,
                external_alias: Some(&external_alias),
            },
        ) {
            Ok(normalized) => {
                let ty = normalized.normalize(ty);
                let mut cache = self.normalization.borrow_mut();
                cache.normalized.extend(normalized.normalized);
                cache.diagnostics.extend(normalized.diagnostics);
                self.input.type_store.get(ty)
            }
            Err(error) => {
                self.internal_error.borrow_mut().get_or_insert(error);
                None
            }
        }
    }

    fn lower_module(&mut self) {
        self.module.local_owners = self.declarations.local_owners.clone();
        for id in self.input.const_exprs.keys() {
            if let nia_ids::ConstExprId::Syntax(node) = &id.const_expr_id
                && let Some(owner) = self.declarations.expression_owners.get(node)
            {
                self.module.const_expr_owners.insert(id.clone(), *owner);
            }
        }
        for item in self.input.active_item_tree.items.iter() {
            match &item.kind {
                ItemTreeNodeKind::Enum(item_enum) => self.lower_enum(item, item_enum),
                ItemTreeNodeKind::Binding(binding) if binding.is_const() => {
                    self.lower_global_initializer(item.span, binding)
                }
                ItemTreeNodeKind::Function(function) if function.is_const => {
                    self.lower_function(function, DefKind::Function, None)
                }
                ItemTreeNodeKind::Trait(item_trait) => {
                    for method in &item_trait.methods {
                        if method.function.is_const && method.function.body.is_some() {
                            self.lower_function(&method.function, DefKind::TraitMethod, None);
                        }
                    }
                }
                ItemTreeNodeKind::Extend(extend) => {
                    for associated_value in &extend.associated_values {
                        if associated_value.binding.value.is_none() {
                            continue;
                        }
                        if extend.generics.is_empty() {
                            self.lower_global_initializer(
                                associated_value.span,
                                &associated_value.binding,
                            );
                        } else {
                            self.lower_deferred_global_initializer(
                                associated_value.span,
                                &associated_value.binding,
                            );
                        }
                    }
                    let receiver_type = self
                        .input
                        .semantic_uses
                        .node_type_use(&extend.target.node_key)
                        .map(|ty| (ty, &extend.target));
                    for method in &extend.methods {
                        if method.function.is_const {
                            self.lower_function(&method.function, DefKind::Method, receiver_type);
                        }
                    }
                }
                _ => {}
            }
        }
        for (local_id, local) in self.input.locals.locals.iter() {
            if local.kind != LocalKind::ConstBinding {
                continue;
            }
            let Some(binding) = self.declarations.local(local_id) else {
                continue;
            };
            let Some(value) = &binding.value else {
                continue;
            };
            let explicit_type = binding
                .ty
                .as_ref()
                .and_then(|ty| self.input.semantic_uses.node_type_use(&ty.node_key));
            let outer = self
                .owner
                .replace(crate::types::DiagnosticOwner::Initializer(ConstKey::Local(
                    local_id,
                )));
            let lowered = self.lower_initializer(value, explicit_type, binding.ty.as_ref());
            self.owner = outer;
            match lowered {
                Some(value) => {
                    self.module.insert_local_initializer(
                        local_id,
                        ResolvedConstLocalInitializer::new(explicit_type, value),
                    );
                }
                None => self.module.insert_failed_local(local_id),
            }
        }
        for (id, expr) in self.input.const_exprs {
            if let Some(lowered) = self.lower_expr(expr) {
                self.module.insert_const_expr(id.clone(), lowered);
            }
        }
    }

    fn lower_enum(&mut self, item: &ItemTreeNode, item_enum: &nia_ast::EnumItem) {
        let Some(enum_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::Enum) else {
            return;
        };
        let variants = item_enum
            .variants
            .iter()
            .filter_map(|variant| {
                let variant_id =
                    self.def_id_for_node(&variant.node_key, variant.span, DefKind::EnumVariant)?;
                let value = variant
                    .value
                    .as_ref()
                    .and_then(|expr| self.lower_expr(expr));
                Some(ResolvedConstEnumVariant::new(
                    self.global_def_id(variant_id),
                    variant.span,
                    value,
                ))
            })
            .collect();
        self.module.push_enum(ResolvedConstEnum::new(
            self.global_def_id(enum_id),
            item.span,
            variants,
        ));
    }

    fn lower_global_initializer(&mut self, item_span: Span, binding: &nia_ast::BindingItem) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Const)
        else {
            return;
        };
        let owner = crate::types::DiagnosticOwner::Initializer(ConstKey::Global(
            self.global_def_id(def_id),
        ));
        let outer = self.owner.replace(owner);
        self.lower_global_initializer_of(item_span, def_id, binding);
        self.owner = outer;
    }

    fn lower_global_initializer_of(
        &mut self,
        item_span: Span,
        def_id: DefId,
        binding: &nia_ast::BindingItem,
    ) {
        let value = binding.value.as_ref();
        let Some(value) = value else {
            if let Some(builtin) = self
                .input
                .signatures
                .consts
                .get(&def_id)
                .and_then(|signature| signature.builtin)
            {
                self.module.insert_global_initializer(
                    self.global_def_id(def_id),
                    ResolvedConstExpr::from_parts(
                        item_span,
                        ResolvedConstExprKind::BuiltinConstValue(builtin),
                    ),
                );
            }
            return;
        };
        let expected_type = self
            .input
            .signatures
            .consts
            .get(&def_id)
            .and_then(|signature| signature.explicit_type);
        let global_id = self.global_def_id(def_id);
        match self.lower_initializer(value, expected_type, binding.ty.as_ref()) {
            Some(value) => {
                self.module.insert_global_initializer(global_id, value);
            }
            None => self.module.insert_failed_global(global_id),
        }
    }

    /// Lowers an initializer against its declared type. A declared type that
    /// failed to lower was already reported, and nothing can be lowered
    /// against it, so the initializer fails without another diagnostic.
    fn lower_initializer(
        &mut self,
        expr: &Expr,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
    ) -> Option<ResolvedConstExpr> {
        if expected_type.is_some_and(|ty| self.contains_error_type(ty)) {
            return None;
        }
        self.lower_expr_with_expected(expr, expected_type, expected_ref)
    }

    /// Whether `ty` is or contains the error type of a failed type.
    fn contains_error_type(&self, ty: InternedTyId) -> bool {
        let mut pending = vec![ty];
        let mut seen = HashSet::new();
        while let Some(current) = pending.pop() {
            if !seen.insert(current) {
                continue;
            }
            match self.type_kind(current) {
                Some(TyKind::Error) => return true,
                Some(kind) => kind.visit_referenced_types(|referenced| pending.push(referenced)),
                None => {}
            }
        }
        false
    }

    fn lower_deferred_global_initializer(
        &mut self,
        item_span: Span,
        binding: &nia_ast::BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Const)
        else {
            return;
        };
        let owner = crate::types::DiagnosticOwner::Initializer(ConstKey::Global(
            self.global_def_id(def_id),
        ));
        let outer = self.owner.replace(owner);
        self.lower_deferred_global_initializer_of(def_id, binding);
        self.owner = outer;
    }

    fn lower_deferred_global_initializer_of(
        &mut self,
        def_id: DefId,
        binding: &nia_ast::BindingItem,
    ) {
        let value = binding.value.as_ref();
        let Some(value) = value else {
            return;
        };
        let expected_type = self
            .input
            .signatures
            .consts
            .get(&def_id)
            .and_then(|signature| signature.explicit_type);
        if let Some(value) =
            self.lower_expr_with_expected(value, expected_type, binding.ty.as_ref())
        {
            self.module
                .insert_deferred_global_initializer(self.global_def_id(def_id), value);
        }
    }

    fn lower_function(
        &mut self,
        function: &nia_ast::FunctionItem,
        kind: DefKind,
        receiver_type: Option<(InternedTyId, &nia_ast::TypeRef)>,
    ) {
        if function.body.is_none() {
            return;
        }
        let Some(def_id) = self.def_id_for_node(&function.node_key, function.span, kind) else {
            return;
        };
        let outer = self.owner.replace(crate::types::DiagnosticOwner::Function(
            self.global_def_id(def_id),
        ));
        self.lower_function_of(def_id, function, receiver_type);
        self.owner = outer;
    }

    fn lower_function_of(
        &mut self,
        def_id: DefId,
        function: &nia_ast::FunctionItem,
        receiver_type: Option<(InternedTyId, &nia_ast::TypeRef)>,
    ) {
        let function_locals = self.function_locals(function);
        let semantic_uses = self.semantic_uses_with_allowed_locals(&function_locals);
        let expected_type = self
            .input
            .signatures
            .functions
            .get(&def_id)
            .map(|signature| signature.return_type)
            .or_else(|| {
                function
                    .return_type
                    .as_ref()
                    .and_then(|ty| self.input.semantic_uses.node_type_use(&ty.node_key))
            });
        let mut omitted_maps = ConstructorMaps::default();
        let mut local_types = LocalTypeHints::new();
        for param in &function.params {
            let Some(local_id) = self.input.semantic_uses.node_local_def(&param.node_key) else {
                continue;
            };
            if param.receiver.is_some() {
                if let Some((ty, type_ref)) = receiver_type {
                    local_types.insert(local_id, (ty, Some(type_ref.clone())));
                }
            } else if let Some(type_ref) = param.ty.as_ref()
                && let Some(ty) = self.input.semantic_uses.node_type_use(&type_ref.node_key)
            {
                local_types.insert(local_id, (ty, Some(type_ref.clone())));
            }
        }
        if let Some(body) = function.body.as_ref() {
            self.collect_function_omitted_maps(
                body,
                expected_type,
                function.return_type.as_ref(),
                &mut omitted_maps,
                &mut local_types,
            );
        }
        let context = nia_const_ir::ResolvedConstLowerInputs::new(&semantic_uses)
            .with_symbols(self.input.symbols)
            .with_constructor_maps(&omitted_maps.aggregate_types, &omitted_maps.omitted_members)
            .with_omitted_associated_types(&omitted_maps.omitted_associated_types);
        match nia_const_ir::lower_function_resolved_with_context(function.span, function, &context)
        {
            Ok(function) => {
                self.module
                    .insert_function(self.global_def_id(def_id), function);
            }
            Err(err) => self.report_lowering_error(err, &omitted_maps),
        }
    }

    /// Reports why lowering failed, unless it failed at an omitted member
    /// whose context was already reported as erroneous.
    fn report_lowering_error(
        &mut self,
        err: nia_const_ir::ConstLowerError,
        maps: &ConstructorMaps,
    ) {
        if maps.erroneous_members.contains(&err.span) {
            return;
        }
        let diagnostic = Diagnostic::user_error_at(codes::CONST, err.span, err.message);
        match self.owner {
            Some(owner) => self.diagnostics.push_owned(diagnostic, owner),
            None => self.diagnostics.push(diagnostic),
        }
    }

    fn lower_expr(&mut self, expr: &Expr) -> Option<ResolvedConstExpr> {
        self.lower_expr_with_expected(expr, None, None)
    }

    fn lower_expr_with_expected(
        &mut self,
        expr: &Expr,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
    ) -> Option<ResolvedConstExpr> {
        let mut allowed_locals = HashSet::new();
        self.collect_expr_locals(expr, &mut allowed_locals);
        let semantic_uses = self.semantic_uses_with_allowed_locals(&allowed_locals);
        let mut maps = ConstructorMaps::default();
        let mut local_types = LocalTypeHints::new();
        self.collect_expr_omitted_maps(
            expr,
            expected_type,
            expected_ref,
            &mut maps,
            &mut local_types,
        );
        let context = nia_const_ir::ResolvedConstLowerInputs::new(&semantic_uses)
            .with_symbols(self.input.symbols)
            .with_constructor_maps(&maps.aggregate_types, &maps.omitted_members)
            .with_omitted_associated_types(&maps.omitted_associated_types);
        match nia_const_ir::lower_expr_resolved_with_context(expr, &context) {
            Ok(expr) => Some(expr),
            Err(err) => {
                self.report_lowering_error(err, &maps);
                None
            }
        }
    }

    fn collect_constructor_maps(
        &self,
        expr: &Expr,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
        maps: &mut ConstructorMaps,
    ) {
        let aggregate_types = &mut maps.aggregate_types;
        let omitted_members = &mut maps.omitted_members;
        let erroneous_members = &mut maps.erroneous_members;
        if let (Some(expected_type), nia_ast::ExprKind::OmittedAggregateLiteral { .. }) =
            (expected_type, &expr.kind)
        {
            aggregate_types.insert(expr.node_key.clone(), expected_type);
        }
        if let nia_ast::ExprKind::QualifiedStructLiteral { target, .. } = &expr.kind
            && let Some(ty) = self.qualified_aggregate_type(target)
        {
            aggregate_types.insert(expr.node_key.clone(), ty);
        }
        // An omitted constructor used as a callable is selected by its result
        // type, while a constructor call is selected by the expression result.
        let constructor_result = expected_type.and_then(|ty| match self.type_kind(ty) {
            Some(
                TyKind::FunctionPointer { return_type, .. }
                | TyKind::Callable { return_type, .. }
                | TyKind::CallablePointee { return_type, .. },
            ) if matches!(expr.kind, nia_ast::ExprKind::OmittedMember { .. }) => Some(*return_type),
            _ => None,
        });
        let context_type = constructor_result.or(expected_type);
        let erroneous_context = context_type.is_some_and(|ty| self.contains_error_type(ty));
        let enum_id = self.enum_def_for_type(
            context_type,
            if constructor_result.is_some() {
                None
            } else {
                expected_ref
            },
        );
        let variant_for = |name: &nia_symbol::SymbolId| {
            let enum_id = enum_id?;
            self.enum_variant_by_name(enum_id, *name)
        };
        let mut record_member = |member: &Expr| {
            let nia_ast::ExprKind::OmittedMember { name } = &member.kind else {
                return;
            };
            match variant_for(name) {
                Some(variant_id) => {
                    omitted_members.insert(member.node_key.clone(), variant_id);
                }
                None if erroneous_context => {
                    erroneous_members.insert(member.span);
                }
                None => {}
            }
        };
        match &expr.kind {
            nia_ast::ExprKind::OmittedMember { .. } => record_member(expr),
            nia_ast::ExprKind::Call { callee, .. } => record_member(callee),
            nia_ast::ExprKind::QualifiedStructLiteral { target, .. } => record_member(target),
            _ => {}
        }
    }

    fn enum_def_for_type(
        &self,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
    ) -> Option<GlobalDefId> {
        expected_type
            .and_then(|ty| match self.type_kind(ty) {
                Some(TyKind::Nominal { def_id, .. }) => Some(*def_id),
                _ => None,
            })
            .or_else(|| {
                expected_ref.and_then(|ty| {
                    self.input
                        .semantic_uses
                        .node_type_prefix(&ty.node_key)
                        .or_else(|| match &ty.kind {
                            nia_ast::TypeKind::Path { segments } if segments.len() == 1 => {
                                let nia_ast::PathSegmentKind::Name(name) = segments[0].kind else {
                                    return None;
                                };
                                self.input.defs.module_scope.types.get(&name).map(|def_id| {
                                    GlobalDefId {
                                        module_id: self.input.defs.module_id,
                                        def_id,
                                    }
                                })
                            }
                            _ => None,
                        })
                })
            })
            .filter(|id| self.is_enum_def(*id))
    }

    fn is_enum_def(&self, id: GlobalDefId) -> bool {
        if id.module_id == self.input.defs.module_id {
            self.input.defs.scopes.enum_members.contains_key(&id.def_id)
        } else {
            self.input
                .defs_for_module
                .and_then(|defs_for_module| defs_for_module(id.module_id))
                .is_some_and(|defs| defs.scopes.enum_members.contains_key(&id.def_id))
        }
    }

    fn collect_function_omitted_maps(
        &self,
        block: &nia_ast::Block,
        return_type: Option<InternedTyId>,
        return_ref: Option<&nia_ast::TypeRef>,
        maps: &mut ConstructorMaps,
        local_types: &mut LocalTypeHints,
    ) {
        for stmt in &block.stmts {
            self.collect_stmt_omitted_maps(
                stmt,
                (None, None),
                (return_type, return_ref),
                maps,
                local_types,
            );
        }
        if let Some(tail) = block.tail.as_deref() {
            self.collect_expr_omitted_maps(tail, return_type, return_ref, maps, local_types);
        }
    }

    /// Collects contextual constructor identities from one statement.
    ///
    /// Match arms may contain any single statement, not only an expression
    /// statement. Keeping this traversal in one place prevents binding,
    /// return, defer, and nested-control-flow arms from silently losing the
    /// same semantic context that ordinary function blocks receive.
    fn collect_stmt_omitted_maps(
        &self,
        stmt: &nia_ast::Stmt,
        expr_expected: ExpectedTypeHint<'_>,
        return_expected: ExpectedTypeHint<'_>,
        maps: &mut ConstructorMaps,
        local_types: &mut LocalTypeHints,
    ) {
        match &stmt.kind {
            nia_ast::StmtKind::Binding(binding) => {
                if let Some(value) = binding.value.as_ref() {
                    let expected = binding
                        .ty
                        .as_ref()
                        .and_then(|ty| self.input.semantic_uses.node_type_use(&ty.node_key));
                    let inferred = expected.or_else(|| self.expr_type_hint(value, local_types));
                    if let Some(inferred) = inferred {
                        self.collect_pattern_omitted_maps(
                            &binding.pattern,
                            Some(inferred),
                            binding.ty.as_ref(),
                            maps,
                            local_types,
                        );
                    }
                    self.collect_expr_omitted_maps(
                        value,
                        inferred,
                        binding.ty.as_ref(),
                        maps,
                        local_types,
                    );
                }
            }
            nia_ast::StmtKind::Static(binding) => {
                if let Some(value) = binding.value.as_ref() {
                    let expected = binding
                        .ty
                        .as_ref()
                        .and_then(|ty| self.input.semantic_uses.node_type_use(&ty.node_key));
                    self.collect_expr_omitted_maps(
                        value,
                        expected,
                        binding.ty.as_ref(),
                        maps,
                        local_types,
                    );
                }
            }
            nia_ast::StmtKind::Expr(expr) => {
                self.collect_expr_omitted_maps(
                    expr,
                    expr_expected.0,
                    expr_expected.1,
                    maps,
                    local_types,
                );
            }
            nia_ast::StmtKind::Return(Some(value)) => {
                self.collect_expr_omitted_maps(
                    value,
                    return_expected.0,
                    return_expected.1,
                    maps,
                    local_types,
                );
            }
            nia_ast::StmtKind::Defer(expr) => {
                self.collect_expr_omitted_maps(expr, None, None, maps, local_types);
            }
            nia_ast::StmtKind::ForIn(for_in) => {
                self.collect_expr_omitted_maps(&for_in.iter, None, None, maps, local_types);
                self.collect_pattern_omitted_maps(&for_in.pattern, None, None, maps, local_types);
                self.collect_function_omitted_maps(
                    &for_in.body,
                    return_expected.0,
                    return_expected.1,
                    maps,
                    local_types,
                );
            }
            nia_ast::StmtKind::While(while_stmt) => {
                self.collect_expr_omitted_maps(&while_stmt.cond, None, None, maps, local_types);
                self.collect_function_omitted_maps(
                    &while_stmt.body,
                    return_expected.0,
                    return_expected.1,
                    maps,
                    local_types,
                );
            }
            nia_ast::StmtKind::Loop(loop_stmt) => {
                self.collect_function_omitted_maps(
                    &loop_stmt.body,
                    return_expected.0,
                    return_expected.1,
                    maps,
                    local_types,
                );
            }
            nia_ast::StmtKind::Using(_)
            | nia_ast::StmtKind::Return(None)
            | nia_ast::StmtKind::Break
            | nia_ast::StmtKind::Continue => {}
        }
    }

    fn collect_expr_omitted_maps(
        &self,
        expr: &Expr,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
        maps: &mut ConstructorMaps,
        local_types: &mut LocalTypeHints,
    ) {
        self.collect_constructor_maps(expr, expected_type, expected_ref, maps);
        let associated = self.omitted_associated_maps(expr, expected_type, expected_ref);
        maps.omitted_associated_types.extend(associated);
        match &expr.kind {
            nia_ast::ExprKind::ArrayLiteral { elems } => {
                let elem_type = expected_type
                    .and_then(|ty| self.array_element_type(ty))
                    .or_else(|| self.array_element_type_hint(elems, local_types));
                match elems {
                    nia_ast::ArrayElements::List(values) => {
                        for value in values {
                            self.collect_expr_omitted_maps(
                                value,
                                elem_type,
                                None,
                                maps,
                                local_types,
                            );
                        }
                    }
                    nia_ast::ArrayElements::Repeat { value, count } => {
                        self.collect_expr_omitted_maps(value, elem_type, None, maps, local_types);
                        self.collect_expr_omitted_maps(count, None, None, maps, local_types);
                    }
                }
            }
            nia_ast::ExprKind::Tuple(values) => {
                let expected_elements = expected_type.and_then(|ty| match self.type_kind(ty) {
                    Some(TyKind::Tuple(elements)) if elements.len() == values.len() => {
                        Some(elements.clone())
                    }
                    _ => None,
                });
                for (index, value) in values.iter().enumerate() {
                    self.collect_expr_omitted_maps(
                        value,
                        expected_elements
                            .as_ref()
                            .and_then(|elements| elements.get(index).copied()),
                        None,
                        maps,
                        local_types,
                    );
                }
            }
            nia_ast::ExprKind::Call { callee, args } => {
                let parameter_types = self
                    .enum_variant_payload_types(callee, expected_type, expected_ref)
                    .and_then(|payload| match payload {
                        EnumVariantPayloadSignature::Tuple(types) => Some(types),
                        _ => None,
                    })
                    .or_else(|| self.direct_call_parameter_types(callee, local_types));
                let receiver = match &callee.kind {
                    nia_ast::ExprKind::Field { lhs, .. } => Some(lhs.as_ref()),
                    nia_ast::ExprKind::BracketSuffix { callee, .. } => {
                        if let nia_ast::ExprKind::Field { lhs, .. } = &callee.kind {
                            Some(lhs.as_ref())
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                if let Some(lhs) = receiver {
                    let receiver_type = self.expr_type_hint(lhs, local_types);
                    self.collect_expr_omitted_maps(lhs, receiver_type, None, maps, local_types);
                }
                for (index, arg) in args.iter().enumerate() {
                    self.collect_expr_omitted_maps(
                        arg,
                        parameter_types
                            .as_ref()
                            .and_then(|params| params.get(index).copied()),
                        None,
                        maps,
                        local_types,
                    );
                }
            }
            nia_ast::ExprKind::BracketSuffix { callee, args } => {
                self.collect_expr_omitted_maps(callee, None, None, maps, local_types);
                for arg in args {
                    if let Some(expr) = arg.expr.as_ref() {
                        self.collect_expr_omitted_maps(expr, None, None, maps, local_types);
                    }
                }
            }
            nia_ast::ExprKind::Binary { lhs, op, rhs } => {
                // Equality supplies enum context symmetrically, as ordinary
                // body checking does. The boolean result is not that context.
                let equality = matches!(op, nia_ast::BinaryOp::Eq | nia_ast::BinaryOp::Ne);
                let lhs_hint = equality
                    .then(|| self.expr_type_hint(rhs, local_types))
                    .flatten();
                let rhs_hint = equality
                    .then(|| self.expr_type_hint(lhs, local_types))
                    .flatten();
                self.collect_expr_omitted_maps(lhs, lhs_hint, None, maps, local_types);
                self.collect_expr_omitted_maps(rhs, rhs_hint, None, maps, local_types);
            }
            nia_ast::ExprKind::Unary {
                op: nia_ast::UnaryOp::RefReadOnly,
                expr: inner,
            } if expected_type.is_some_and(|ty| {
                matches!(
                    self.type_kind(ty),
                    Some(TyKind::FunctionPointer { .. } | TyKind::Callable { .. })
                )
            }) =>
            {
                self.collect_expr_omitted_maps(inner, expected_type, None, maps, local_types);
            }
            nia_ast::ExprKind::Unary { expr: inner, .. }
            | nia_ast::ExprKind::Try { expr: inner }
            | nia_ast::ExprKind::Cast { expr: inner, .. } => {
                self.collect_expr_omitted_maps(inner, None, None, maps, local_types);
            }
            nia_ast::ExprKind::OptionalSome { expr: inner }
            | nia_ast::ExprKind::ErrorOk { expr: inner }
            | nia_ast::ExprKind::ErrorErr { expr: inner } => {
                let inner_type =
                    expected_type.and_then(|ty| match (&expr.kind, self.type_kind(ty)) {
                        (
                            nia_ast::ExprKind::OptionalSome { .. },
                            Some(TyKind::Optional { elem }),
                        ) => Some(*elem),
                        (
                            nia_ast::ExprKind::ErrorOk { .. },
                            Some(TyKind::ErrorUnion { value, .. }),
                        ) => Some(*value),
                        (
                            nia_ast::ExprKind::ErrorErr { .. },
                            Some(TyKind::ErrorUnion { error, .. }),
                        ) => Some(*error),
                        _ => None,
                    });
                self.collect_expr_omitted_maps(inner, inner_type, None, maps, local_types);
            }
            nia_ast::ExprKind::Field { lhs, .. } | nia_ast::ExprKind::TupleField { lhs, .. } => {
                self.collect_expr_omitted_maps(lhs, None, None, maps, local_types);
            }
            nia_ast::ExprKind::Index { lhs, index } => {
                self.collect_expr_omitted_maps(lhs, None, None, maps, local_types);
                match index {
                    nia_ast::IndexArg::Expr(index) => {
                        self.collect_expr_omitted_maps(index, None, None, maps, local_types);
                    }
                    nia_ast::IndexArg::Range(range) => {
                        if let Some(start) = range.start.as_deref() {
                            self.collect_expr_omitted_maps(start, None, None, maps, local_types);
                        }
                        if let Some(end) = range.end.as_deref() {
                            self.collect_expr_omitted_maps(end, None, None, maps, local_types);
                        }
                    }
                }
            }
            nia_ast::ExprKind::Assign { lhs, rhs, .. } => {
                let lhs_type = self.expr_type_hint(lhs, local_types);
                self.collect_expr_omitted_maps(lhs, lhs_type, None, maps, local_types);
                self.collect_expr_omitted_maps(rhs, lhs_type, None, maps, local_types);
            }
            nia_ast::ExprKind::Qualified { lhs, .. } => {
                self.collect_expr_omitted_maps(lhs, None, None, maps, local_types);
            }
            nia_ast::ExprKind::Range(range) => {
                if let Some(start) = range.start.as_deref() {
                    self.collect_expr_omitted_maps(start, None, None, maps, local_types);
                }
                if let Some(end) = range.end.as_deref() {
                    self.collect_expr_omitted_maps(end, None, None, maps, local_types);
                }
            }
            nia_ast::ExprKind::TypedStructLiteral { ty, fields } => {
                let aggregate_type = self
                    .input
                    .semantic_uses
                    .node_type_use(&ty.node_key)
                    .or(expected_type);
                self.collect_aggregate_field_omitted_maps(
                    fields,
                    aggregate_type,
                    maps,
                    local_types,
                );
            }
            nia_ast::ExprKind::QualifiedStructLiteral { target, fields } => {
                if let Some(ty) = self.qualified_aggregate_type(target) {
                    self.collect_aggregate_field_omitted_maps(fields, Some(ty), maps, local_types);
                    return;
                }
                self.collect_expr_omitted_maps(target, None, None, maps, local_types);
                let field_types =
                    self.enum_variant_field_types(target, expected_type, expected_ref);
                for field in fields {
                    self.collect_expr_omitted_maps(
                        &field.value,
                        field_types
                            .as_ref()
                            .and_then(|types| types.get(&field.name).copied()),
                        None,
                        maps,
                        local_types,
                    );
                }
            }
            nia_ast::ExprKind::OmittedAggregateLiteral { fields } => {
                self.collect_aggregate_field_omitted_maps(fields, expected_type, maps, local_types);
            }
            nia_ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_expr_omitted_maps(cond, None, None, maps, local_types);
                self.collect_function_omitted_maps(
                    then_branch,
                    expected_type,
                    expected_ref,
                    maps,
                    local_types,
                );
                if let Some(else_branch) = else_branch.as_deref() {
                    self.collect_expr_omitted_maps(
                        else_branch,
                        expected_type,
                        expected_ref,
                        maps,
                        local_types,
                    );
                }
            }
            nia_ast::ExprKind::IfPattern(if_pattern) => {
                self.collect_expr_omitted_maps(&if_pattern.target, None, None, maps, local_types);
                self.collect_pattern_omitted_maps(
                    &if_pattern.pattern,
                    None,
                    None,
                    maps,
                    local_types,
                );
                self.collect_function_omitted_maps(
                    &if_pattern.then_branch,
                    expected_type,
                    expected_ref,
                    maps,
                    local_types,
                );
                if let Some(else_branch) = if_pattern.else_branch.as_deref() {
                    self.collect_expr_omitted_maps(
                        else_branch,
                        expected_type,
                        expected_ref,
                        maps,
                        local_types,
                    );
                }
            }
            nia_ast::ExprKind::IfPatternChain(chain) => {
                for clause in &chain.clauses {
                    match clause {
                        nia_ast::IfPatternChainClause::Pattern { target, pattern } => {
                            self.collect_expr_omitted_maps(target, None, None, maps, local_types);
                            self.collect_pattern_omitted_maps(
                                pattern,
                                None,
                                None,
                                maps,
                                local_types,
                            );
                        }
                        nia_ast::IfPatternChainClause::Condition(condition) => {
                            self.collect_expr_omitted_maps(
                                condition,
                                None,
                                None,
                                maps,
                                local_types,
                            );
                        }
                    }
                }
                self.collect_function_omitted_maps(
                    &chain.then_branch,
                    expected_type,
                    expected_ref,
                    maps,
                    local_types,
                );
                if let Some(else_branch) = chain.else_branch.as_deref() {
                    self.collect_expr_omitted_maps(
                        else_branch,
                        expected_type,
                        expected_ref,
                        maps,
                        local_types,
                    );
                }
            }
            nia_ast::ExprKind::Block(block) => self.collect_function_omitted_maps(
                block,
                expected_type,
                expected_ref,
                maps,
                local_types,
            ),
            nia_ast::ExprKind::Match(matched) => {
                self.collect_expr_omitted_maps(&matched.target, None, None, maps, local_types);
                let target_hint = self
                    .input
                    .semantic_uses
                    .node_value_use(&matched.target.node_key)
                    .and_then(|use_| match use_ {
                        SemanticValueUse::Local(local_id) => local_types.get(&local_id).cloned(),
                        SemanticValueUse::Global(_) => None,
                    })
                    .or_else(|| {
                        self.expr_type_hint(&matched.target, local_types)
                            .map(|ty| (ty, None))
                    });
                for arm in &matched.arms {
                    for pattern in &arm.patterns {
                        self.collect_pattern_omitted_maps(
                            pattern,
                            target_hint.as_ref().map(|(ty, _)| *ty),
                            target_hint
                                .as_ref()
                                .and_then(|(_, type_ref)| type_ref.as_ref()),
                            maps,
                            local_types,
                        );
                    }
                    match &arm.body {
                        nia_ast::MatchArmBody::Expr(body) => self.collect_expr_omitted_maps(
                            body,
                            expected_type,
                            expected_ref,
                            maps,
                            local_types,
                        ),
                        nia_ast::MatchArmBody::Block(body) => self.collect_function_omitted_maps(
                            body,
                            expected_type,
                            expected_ref,
                            maps,
                            local_types,
                        ),
                        nia_ast::MatchArmBody::Stmt(stmt) => self.collect_stmt_omitted_maps(
                            stmt,
                            (expected_type, expected_ref),
                            (expected_type, expected_ref),
                            maps,
                            local_types,
                        ),
                    }
                }
            }
            nia_ast::ExprKind::Closure { captures, body, .. } => {
                for capture in captures {
                    self.collect_expr_omitted_maps(&capture.value, None, None, maps, local_types);
                }
                self.collect_expr_omitted_maps(body, None, None, maps, local_types);
            }
            _ => {}
        }
    }

    fn array_element_type(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.type_kind(ty)? {
            TyKind::Array { elem, .. } => Some(*elem),
            TyKind::Pointer { elem, .. } => match self.type_kind(*elem)? {
                TyKind::Array { elem, .. } | TyKind::SlicePointee { elem } => Some(*elem),
                _ => None,
            },
            TyKind::Slice { elem, .. } => Some(*elem),
            _ => None,
        }
    }

    fn collect_aggregate_field_omitted_maps(
        &self,
        fields: &[nia_ast::FieldInit],
        aggregate_type: Option<InternedTyId>,
        maps: &mut ConstructorMaps,
        local_types: &mut LocalTypeHints,
    ) {
        for field in fields {
            let field_type =
                aggregate_type.and_then(|ty| self.aggregate_field_type(ty, field.name));
            self.collect_expr_omitted_maps(&field.value, field_type, None, maps, local_types);
        }
    }

    /// A qualified brace constructor may name a type or an enum variant.
    /// Use resolved type-prefix identity rather than treating both as values.
    /// A generic prefix without arguments has no type of its own: its
    /// arguments are inferred by body checking.
    fn qualified_aggregate_type(&self, target: &Expr) -> Option<InternedTyId> {
        if let Some(ty) = self.input.semantic_uses.node_type_use(&target.node_key) {
            return Some(ty);
        }
        if let nia_ast::ExprKind::BracketSuffix { callee, args } = &target.kind {
            let def_id = self
                .input
                .semantic_uses
                .node_type_prefix(&callee.node_key)?;
            let (args, const_args) = self.with_type_prefix_params(def_id, |params| {
                let (types, consts) = self.explicit_call_substitutions(params, args);
                let type_args = params
                    .iter()
                    .filter(|param| matches!(param.kind, GenericParamSignatureKind::Type))
                    .map(|param| types.get(&param.name).copied())
                    .collect::<Option<Vec<_>>>()?;
                let const_args = params
                    .iter()
                    .filter(|param| matches!(param.kind, GenericParamSignatureKind::Const { .. }))
                    .map(|param| consts.get(&param.name).cloned())
                    .collect::<Option<Vec<_>>>()?;
                Some((type_args, const_args))
            })??;
            return self.intern_type(TyKind::Nominal {
                def_id,
                args,
                const_args,
            });
        }
        let def_id = self
            .input
            .semantic_uses
            .node_type_prefix(&target.node_key)?;
        if self.with_type_prefix_params(def_id, |params| !params.is_empty()) == Some(true) {
            return None;
        }
        self.intern_type(TyKind::Nominal {
            def_id,
            args: Vec::new(),
            const_args: Vec::new(),
        })
    }

    /// The enum a qualified variant path's prefix names. Only its identity is
    /// needed, so a generic prefix whose arguments are left to inference
    /// (`E::A(5)`, or a generic alias of `E`) still names it.
    fn qualified_enum_def(&self, prefix: &Expr) -> Option<GlobalDefId> {
        if let Some(ty) = self.qualified_aggregate_type(prefix) {
            return self.enum_def_for_type(Some(ty), None);
        }
        let prefix = match &prefix.kind {
            nia_ast::ExprKind::BracketSuffix { callee, .. } => callee,
            _ => prefix,
        };
        let def_id = self
            .input
            .semantic_uses
            .node_type_prefix(&prefix.node_key)?;
        match self
            .with_signatures(def_id.module_id, |signatures| {
                signatures
                    .type_aliases
                    .get(&def_id.def_id)
                    .map(|alias| alias.target)
            })
            .flatten()
        {
            Some(target) => self.enum_def_for_type(Some(target), None),
            None => self.is_enum_def(def_id).then_some(def_id),
        }
    }

    /// Runs `f` on the generic parameters of the type a prefix names.
    fn with_type_prefix_params<R>(
        &self,
        def_id: GlobalDefId,
        f: impl FnOnce(&[nia_item_signatures::GenericParamSignature]) -> R,
    ) -> Option<R> {
        self.with_signatures(def_id.module_id, |signatures| {
            let params = signatures
                .enums
                .get(&def_id.def_id)
                .map(|signature| &signature.generic_params)
                .or_else(|| {
                    signatures
                        .type_aliases
                        .get(&def_id.def_id)
                        .map(|signature| &signature.generic_params)
                })
                .or_else(|| {
                    signatures
                        .structs
                        .get(&def_id.def_id)
                        .map(|signature| &signature.generic_params)
                })
                .or_else(|| {
                    signatures
                        .unions
                        .get(&def_id.def_id)
                        .map(|signature| &signature.generic_params)
                })?;
            Some(f(params))
        })?
    }

    /// Runs `f` on the signatures of `module_id`, the active module's or an
    /// imported one's.
    fn with_signatures<R>(
        &self,
        module_id: nia_ids::ModuleId,
        f: impl FnOnce(&nia_item_signatures::ItemSignatures) -> R,
    ) -> Option<R> {
        if module_id == self.input.defs.module_id {
            return Some(f(self.input.signatures));
        }
        let signatures = (self.input.signatures_for_module?)(module_id)?;
        Some(f(&signatures))
    }

    fn intern_type(&self, kind: TyKind) -> Option<InternedTyId> {
        match self
            .input
            .type_store
            .append_for_module(self.input.defs.module_id)
            .intern(kind)
        {
            Ok(ty) => Some(ty),
            Err(error) => {
                self.internal_error.borrow_mut().get_or_insert(error);
                None
            }
        }
    }

    fn aggregate_field_type(
        &self,
        ty: InternedTyId,
        name: nia_symbol::SymbolId,
    ) -> Option<InternedTyId> {
        let TyKind::Nominal {
            def_id,
            args,
            const_args,
        } = self.type_kind(ty)?
        else {
            return None;
        };
        let foreign_signatures = (def_id.module_id != self.input.defs.module_id)
            .then(|| (self.input.signatures_for_module?)(def_id.module_id))
            .flatten();
        if def_id.module_id != self.input.defs.module_id && foreign_signatures.is_none() {
            return None;
        }
        let signatures = foreign_signatures
            .as_deref()
            .unwrap_or(self.input.signatures);
        let (generic_params, field_type) =
            if let Some(signature) = signatures.structs.get(&def_id.def_id) {
                (
                    &signature.generic_params,
                    signature
                        .fields
                        .iter()
                        .find(|field| field.name == name)
                        .map(|field| field.ty)?,
                )
            } else {
                let signature = signatures.unions.get(&def_id.def_id)?;
                (
                    &signature.generic_params,
                    signature
                        .fields
                        .iter()
                        .find(|field| field.name == name)
                        .map(|field| field.ty)?,
                )
            };
        let substitutions = generic_argument_substitutions(generic_params, args, const_args);
        let (type_substitutions, const_substitutions) = substitutions.unwrap_or_default();
        nia_ty::substitute_ty(
            self.input.type_store,
            &self
                .input
                .type_store
                .append_for_module(self.input.defs.module_id),
            field_type,
            &|generic| type_substitutions.get(generic).copied(),
            &|generic| const_substitutions.get(generic).cloned(),
            None,
        )
        .ok()
    }

    fn direct_call_parameter_types(
        &self,
        callee: &Expr,
        local_types: &LocalTypeHints,
    ) -> Option<Vec<InternedTyId>> {
        let (value_callee, bracket_args) = match &callee.kind {
            nia_ast::ExprKind::BracketSuffix { callee, args } => (callee.as_ref(), args.as_slice()),
            _ => (callee, &[] as &[nia_ast::BracketArg]),
        };
        if let nia_ast::ExprKind::Field { lhs, name } = &value_callee.kind {
            return self.method_call_parameter_types(lhs, *name, local_types, bracket_args);
        }
        let SemanticValueUse::Global(global) = self
            .input
            .semantic_uses
            .node_value_use(&value_callee.node_key)?
        else {
            return None;
        };
        let foreign_signatures = (global.module_id != self.input.defs.module_id)
            .then(|| (self.input.signatures_for_module?)(global.module_id))
            .flatten();
        if global.module_id != self.input.defs.module_id && foreign_signatures.is_none() {
            return None;
        }
        let signatures = foreign_signatures
            .as_deref()
            .unwrap_or(self.input.signatures);
        let signature = signatures.functions.get(&global.def_id)?;
        let (type_substitutions, const_substitutions) =
            self.explicit_call_substitutions(&signature.generic_params, bracket_args);
        let params = signature
            .params
            .iter()
            .filter(|param| param.receiver.is_none())
            .map(|param| {
                nia_ty::substitute_ty(
                    self.input.type_store,
                    &self
                        .input
                        .type_store
                        .append_for_module(self.input.defs.module_id),
                    param.ty,
                    &|generic| type_substitutions.get(generic).copied(),
                    &|generic| const_substitutions.get(generic).cloned(),
                    None,
                )
                .ok()
            })
            .collect::<Option<Vec<_>>>()?;
        Some(params)
    }

    /// Reconstructs the substitutions owned by an explicit call-site generic
    /// list. The ordinary checker records these facts, but const lowering also
    /// needs them while it propagates a method's return type into a later
    /// field or method receiver. Keeping this reconstruction declaration-order
    /// based preserves interleaved type/const parameters.
    fn explicit_call_substitutions(
        &self,
        params: &[nia_item_signatures::GenericParamSignature],
        args: &[nia_ast::BracketArg],
    ) -> (
        nia_symbol::SymbolMap<InternedTyId>,
        nia_symbol::SymbolMap<nia_ty::ConstGenericArg>,
    ) {
        let mut type_substitutions = nia_symbol::SymbolMap::default();
        let mut const_substitutions = nia_symbol::SymbolMap::default();
        for (param, arg) in params.iter().zip(args) {
            match param.kind {
                GenericParamSignatureKind::Type => {
                    if let Some(ty) = arg.ty.as_ref()
                        && !matches!(ty.kind, nia_ast::TypeKind::Infer)
                        && let Some(ty) = self.input.semantic_uses.node_type_use(&ty.node_key)
                    {
                        type_substitutions.insert(param.name, ty);
                    }
                }
                GenericParamSignatureKind::Const { ty } => {
                    if let Some(expr) = arg.expr.as_ref()
                        && let Some(value) = self.explicit_const_generic_value(expr)
                    {
                        const_substitutions
                            .insert(param.name, nia_ty::ConstGenericArg { ty, value });
                    }
                }
            }
        }
        (type_substitutions, const_substitutions)
    }

    fn explicit_const_generic_value(&self, expr: &Expr) -> Option<nia_ty::ConstGenericValue> {
        match &expr.kind {
            nia_ast::ExprKind::Integer(text) => nia_literals::eval_int_literal(text)
                .ok()
                .map(nia_ty::IntConst::unsigned)
                .map(nia_ty::ConstGenericValue::Int),
            nia_ast::ExprKind::Bool(value) => Some(nia_ty::ConstGenericValue::Bool(*value)),
            nia_ast::ExprKind::Char(text) => nia_literals::decode_char_literal(text)
                .and_then(char::from_u32)
                .map(nia_ty::ConstGenericValue::Char),
            nia_ast::ExprKind::Unary {
                op: nia_ast::UnaryOp::Neg,
                expr: inner,
            } => match self.explicit_const_generic_value(inner)? {
                nia_ty::ConstGenericValue::Int(value) => Some(nia_ty::ConstGenericValue::Int(
                    nia_ty::IntConst::signed_bits(0u128.wrapping_sub(value.bits())),
                )),
                _ => Some(nia_ty::ConstGenericValue::ConstExpr(
                    nia_ids::GlobalConstExprId {
                        module_id: self.input.defs.module_id,
                        const_expr_id: nia_ids::ConstExprId::from_syntax(expr.node_key.clone()),
                    },
                )),
            },
            nia_ast::ExprKind::Ident(_name) => self
                .input
                .semantic_uses
                .node_const_generic_use(&expr.node_key)
                .copied()
                .map(nia_ty::ConstGenericValue::GenericParam)
                .or_else(|| {
                    matches!(
                        self.input.semantic_uses.node_value_use(&expr.node_key),
                        Some(SemanticValueUse::Global(_))
                    )
                    .then(|| {
                        nia_ty::ConstGenericValue::ConstExpr(nia_ids::GlobalConstExprId {
                            module_id: self.input.defs.module_id,
                            const_expr_id: nia_ids::ConstExprId::from_syntax(expr.node_key.clone()),
                        })
                    })
                }),
            _ => Some(nia_ty::ConstGenericValue::ConstExpr(
                nia_ids::GlobalConstExprId {
                    module_id: self.input.defs.module_id,
                    const_expr_id: nia_ids::ConstExprId::from_syntax(expr.node_key.clone()),
                },
            )),
        }
    }

    fn direct_call_return_type(
        &self,
        callee: &Expr,
        local_types: &LocalTypeHints,
    ) -> Option<InternedTyId> {
        let (value_callee, bracket_args) = match &callee.kind {
            nia_ast::ExprKind::BracketSuffix { callee, args } => (callee.as_ref(), args.as_slice()),
            _ => (callee, &[] as &[nia_ast::BracketArg]),
        };
        if let nia_ast::ExprKind::Field { lhs, name } = &value_callee.kind {
            return self.method_call_return_type(lhs, *name, local_types, bracket_args);
        }
        if let Some(variant) = self.constructor_variant_id(value_callee, None, None)
            && let Some(ty) = self.enum_variant_value_type(variant, value_callee, false)
        {
            return Some(ty);
        }
        let SemanticValueUse::Global(global) = self
            .input
            .semantic_uses
            .node_value_use(&value_callee.node_key)?
        else {
            return None;
        };
        let foreign_signatures = (global.module_id != self.input.defs.module_id)
            .then(|| (self.input.signatures_for_module?)(global.module_id))
            .flatten();
        if global.module_id != self.input.defs.module_id && foreign_signatures.is_none() {
            return None;
        }
        let signatures = foreign_signatures
            .as_deref()
            .unwrap_or(self.input.signatures);
        let signature = signatures.functions.get(&global.def_id)?;
        let (type_substitutions, const_substitutions) =
            self.explicit_call_substitutions(&signature.generic_params, bracket_args);
        nia_ty::substitute_ty(
            self.input.type_store,
            &self
                .input
                .type_store
                .append_for_module(self.input.defs.module_id),
            signature.return_type,
            &|generic| type_substitutions.get(generic).copied(),
            &|generic| const_substitutions.get(generic).cloned(),
            None,
        )
        .ok()
    }

    fn method_call_return_type(
        &self,
        receiver: &Expr,
        name: nia_symbol::SymbolId,
        local_types: &LocalTypeHints,
        bracket_args: &[nia_ast::BracketArg],
    ) -> Option<InternedTyId> {
        let receiver_ty = self.expr_type_hint(receiver, local_types)?;
        let visible = self
            .input
            .visible_extensions_for_module
            .and_then(|provider| provider(self.input.defs.module_id))?;
        let (method, mut type_substitutions, mut const_substitutions) = visible
            .all_methods_named(&name)
            .into_iter()
            .find_map(|(target, method)| {
                let mut types = nia_symbol::SymbolMap::default();
                let mut consts = nia_symbol::SymbolMap::default();
                self.match_type_pattern(target, receiver_ty, &mut types, &mut consts)
                    .then_some((method, types, consts))
            })?;
        let foreign_signatures = (method.def_id.module_id != self.input.defs.module_id)
            .then(|| (self.input.signatures_for_module?)(method.def_id.module_id))
            .flatten();
        if method.def_id.module_id != self.input.defs.module_id && foreign_signatures.is_none() {
            return None;
        }
        let signatures = foreign_signatures
            .as_deref()
            .unwrap_or(self.input.signatures);
        let signature = signatures.functions.get(&method.def_id.def_id)?;
        let (explicit_types, explicit_consts) =
            self.explicit_call_substitutions(&signature.generic_params, bracket_args);
        type_substitutions.extend(explicit_types);
        const_substitutions.extend(explicit_consts);
        nia_ty::substitute_ty(
            self.input.type_store,
            &self
                .input
                .type_store
                .append_for_module(self.input.defs.module_id),
            signature.return_type,
            &|generic| type_substitutions.get(generic).copied(),
            &|generic| const_substitutions.get(generic).cloned(),
            None,
        )
        .ok()
    }

    fn method_call_parameter_types(
        &self,
        receiver: &Expr,
        name: nia_symbol::SymbolId,
        local_types: &LocalTypeHints,
        bracket_args: &[nia_ast::BracketArg],
    ) -> Option<Vec<InternedTyId>> {
        let receiver_ty = self.expr_type_hint(receiver, local_types)?;
        let visible = self
            .input
            .visible_extensions_for_module
            .and_then(|provider| provider(self.input.defs.module_id))?;
        let (method, mut target_substitutions, mut target_const_substitutions) = visible
            .all_methods_named(&name)
            .into_iter()
            .find_map(|(target, method)| {
                let mut type_substitutions = nia_symbol::SymbolMap::default();
                let mut const_substitutions = nia_symbol::SymbolMap::default();
                self.match_type_pattern(
                    target,
                    receiver_ty,
                    &mut type_substitutions,
                    &mut const_substitutions,
                )
                .then_some((method, type_substitutions, const_substitutions))
            })?;
        let foreign_signatures = (method.def_id.module_id != self.input.defs.module_id)
            .then(|| (self.input.signatures_for_module?)(method.def_id.module_id))
            .flatten();
        if method.def_id.module_id != self.input.defs.module_id && foreign_signatures.is_none() {
            return None;
        }
        let signatures = foreign_signatures
            .as_deref()
            .unwrap_or(self.input.signatures);
        let signature = signatures.functions.get(&method.def_id.def_id)?;
        let (explicit_types, explicit_consts) =
            self.explicit_call_substitutions(&signature.generic_params, bracket_args);
        for (name, ty) in explicit_types {
            target_substitutions.insert(name, ty);
        }
        for (name, arg) in explicit_consts {
            target_const_substitutions.insert(name, arg);
        }
        signature
            .params
            .iter()
            .filter(|param| param.receiver.is_none())
            .map(|param| {
                nia_ty::substitute_ty(
                    self.input.type_store,
                    &self
                        .input
                        .type_store
                        .append_for_module(self.input.defs.module_id),
                    param.ty,
                    &|generic| target_substitutions.get(generic).copied(),
                    &|generic| target_const_substitutions.get(generic).cloned(),
                    None,
                )
                .ok()
            })
            .collect::<Option<Vec<_>>>()
    }

    /// Matches a visible extension target against the concrete receiver while
    /// retaining the substitutions needed to contextualize its method fields.
    /// This is the same structural relation used by ordinary method lookup,
    /// reduced to the declaration facts available during const IR lowering.
    fn match_type_pattern(
        &self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut nia_symbol::SymbolMap<InternedTyId>,
        const_substitutions: &mut nia_symbol::SymbolMap<nia_ty::ConstGenericArg>,
    ) -> bool {
        if pattern == actual {
            return true;
        }
        let Some(pattern_kind) = self.type_kind(pattern).cloned() else {
            return false;
        };
        let Some(actual_kind) = self.type_kind(actual).cloned() else {
            return false;
        };
        match (pattern_kind, actual_kind) {
            (TyKind::GenericParam(name), _) => {
                if let Some(existing) = substitutions.get(&name).copied() {
                    existing == actual
                } else {
                    substitutions.insert(name, actual);
                    true
                }
            }
            (TyKind::SelfParam, _) => true,
            (TyKind::Tuple(patterns), TyKind::Tuple(actuals)) => {
                patterns.len() == actuals.len()
                    && patterns.iter().zip(actuals).all(|(pattern, actual)| {
                        self.match_type_pattern(
                            *pattern,
                            actual,
                            substitutions,
                            const_substitutions,
                        )
                    })
            }
            (
                TyKind::Pointer {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                },
                TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                },
            )
            | (
                TyKind::VolatilePointer {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                },
                TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                },
            ) => {
                pattern_readonly == actual_readonly
                    && self.match_type_pattern(
                        pattern_elem,
                        actual_elem,
                        substitutions,
                        const_substitutions,
                    )
            }
            (
                TyKind::Slice {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                },
                TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                },
            ) => {
                pattern_readonly == actual_readonly
                    && self.match_type_pattern(
                        pattern_elem,
                        actual_elem,
                        substitutions,
                        const_substitutions,
                    )
            }
            (TyKind::SlicePointee { elem: pattern }, TyKind::SlicePointee { elem: actual }) => {
                self.match_type_pattern(pattern, actual, substitutions, const_substitutions)
            }
            (
                TyKind::Array {
                    len: pattern_len,
                    elem: pattern_elem,
                },
                TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                },
            ) => {
                self.match_array_len(&pattern_len, &actual_len, const_substitutions)
                    && self.match_type_pattern(
                        pattern_elem,
                        actual_elem,
                        substitutions,
                        const_substitutions,
                    )
            }
            (TyKind::Optional { elem: pattern }, TyKind::Optional { elem: actual })
            | (
                TyKind::Range {
                    kind: _,
                    bound: Some(pattern),
                },
                TyKind::Range {
                    kind: _,
                    bound: Some(actual),
                },
            ) => self.match_type_pattern(pattern, actual, substitutions, const_substitutions),
            (
                TyKind::ErrorUnion {
                    error: pattern_error,
                    value: pattern_value,
                },
                TyKind::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                },
            ) => {
                self.match_type_pattern(
                    pattern_error,
                    actual_error,
                    substitutions,
                    const_substitutions,
                ) && self.match_type_pattern(
                    pattern_value,
                    actual_value,
                    substitutions,
                    const_substitutions,
                )
            }
            (
                TyKind::Nominal {
                    def_id: pattern_def,
                    args: pattern_args,
                    const_args: pattern_consts,
                },
                TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                    const_args: actual_consts,
                },
            ) => {
                pattern_def == actual_def
                    && pattern_args.len() == actual_args.len()
                    && pattern_consts.len() == actual_consts.len()
                    && pattern_args
                        .iter()
                        .zip(actual_args)
                        .all(|(pattern, actual)| {
                            self.match_type_pattern(
                                *pattern,
                                actual,
                                substitutions,
                                const_substitutions,
                            )
                        })
                    && pattern_consts
                        .iter()
                        .zip(actual_consts)
                        .all(|(pattern, actual)| {
                            self.match_const_arg(pattern, &actual, const_substitutions)
                        })
            }
            (TyKind::Primitive(pattern), TyKind::Primitive(actual)) => pattern == actual,
            (TyKind::BuiltinType(pattern), TyKind::BuiltinType(actual)) => pattern == actual,
            (
                TyKind::Vector {
                    elem: pattern_elem,
                    lanes: pattern_lanes,
                },
                TyKind::Vector {
                    elem: actual_elem,
                    lanes: actual_lanes,
                },
            ) => pattern_elem == actual_elem && pattern_lanes == actual_lanes,
            _ => false,
        }
    }

    fn match_array_len(
        &self,
        pattern: &nia_ty::ArrayLenTy,
        actual: &nia_ty::ArrayLenTy,
        substitutions: &mut nia_symbol::SymbolMap<nia_ty::ConstGenericArg>,
    ) -> bool {
        match (pattern, actual) {
            (nia_ty::ArrayLenTy::GenericParam(name), nia_ty::ArrayLenTy::ConstValue(value)) => {
                let argument = nia_ty::ConstGenericArg {
                    ty: self
                        .input
                        .type_store
                        .append_for_module(self.input.defs.module_id)
                        .intern(TyKind::Primitive(nia_ty::PrimitiveTy::Usize))
                        .ok()
                        .unwrap_or_else(|| self.input.type_store.error()),
                    value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(
                        (*value).into(),
                    )),
                };
                if let Some(existing) = substitutions.get(name) {
                    existing == &argument
                } else {
                    substitutions.insert(*name, argument);
                    true
                }
            }
            (left, right) => left == right,
        }
    }

    fn match_const_arg(
        &self,
        pattern: &nia_ty::ConstGenericArg,
        actual: &nia_ty::ConstGenericArg,
        substitutions: &mut nia_symbol::SymbolMap<nia_ty::ConstGenericArg>,
    ) -> bool {
        match &pattern.value {
            nia_ty::ConstGenericValue::GenericParam(name) => {
                if let Some(existing) = substitutions.get(name) {
                    existing == actual
                } else {
                    substitutions.insert(*name, actual.clone());
                    true
                }
            }
            _ => pattern == actual,
        }
    }

    fn expr_type_hint(&self, expr: &Expr, local_types: &LocalTypeHints) -> Option<InternedTyId> {
        if let Some(ty) = self.input.semantic_uses.node_type_use(&expr.node_key) {
            return Some(ty);
        }
        match self.input.semantic_uses.node_value_use(&expr.node_key) {
            Some(SemanticValueUse::Local(local_id)) => {
                return local_types
                    .get(&local_id)
                    .map(|(ty, _)| *ty)
                    .or_else(|| self.local_const_type(local_id));
            }
            Some(SemanticValueUse::Global(id)) => {
                let foreign = (id.module_id != self.input.defs.module_id)
                    .then(|| (self.input.signatures_for_module?)(id.module_id))
                    .flatten();
                let signatures = if id.module_id == self.input.defs.module_id {
                    Some(self.input.signatures)
                } else {
                    foreign.as_deref()
                };
                if let Some(ty) = signatures.and_then(|signatures| {
                    signatures
                        .consts
                        .get(&id.def_id)
                        .and_then(|signature| signature.explicit_type)
                        .or_else(|| {
                            signatures
                                .globals
                                .get(&id.def_id)
                                .and_then(|signature| signature.explicit_type)
                        })
                }) {
                    return Some(ty);
                }
                if id.module_id == self.input.defs.module_id
                    && let Some(ty) = self.untyped_global_const_type(id.def_id)
                {
                    return Some(ty);
                }
            }
            None => {}
        }
        if let Some(variant) = self.constructor_variant_id(expr, None, None)
            && let Some(ty) = self.enum_variant_value_type(variant, expr, true)
        {
            return Some(ty);
        }
        match &expr.kind {
            nia_ast::ExprKind::TypedStructLiteral { ty, .. } => {
                self.input.semantic_uses.node_type_use(&ty.node_key)
            }
            nia_ast::ExprKind::QualifiedStructLiteral { target, .. } => {
                self.qualified_aggregate_type(target)
            }
            nia_ast::ExprKind::Call { callee, .. } => {
                self.direct_call_return_type(callee, local_types)
            }
            nia_ast::ExprKind::Index {
                lhs,
                index: nia_ast::IndexArg::Expr(_),
            } => self.array_element_type(self.expr_type_hint(lhs, local_types)?),
            nia_ast::ExprKind::BracketSuffix { callee, args } if args.len() == 1 => {
                self.array_element_type(self.expr_type_hint(callee, local_types)?)
            }
            nia_ast::ExprKind::Field { lhs, name } => {
                self.aggregate_field_type(self.expr_type_hint(lhs, local_types)?, *name)
            }
            _ => None,
        }
    }

    /// The type of a local const: its declared type, else its initializer's.
    /// A local const initializer refers only to bindings declared before it,
    /// so this cannot recurse into itself.
    fn local_const_type(&self, local_id: LocalId) -> Option<InternedTyId> {
        let binding = self.declarations.local(local_id)?;
        match &binding.ty {
            Some(ty) => self.input.semantic_uses.node_type_use(&ty.node_key),
            None => self.expr_type_hint(binding.value.as_ref()?, &LocalTypeHints::new()),
        }
    }

    /// The type of an untyped const of the active module, from its
    /// initializer. A typed const's type is its signature's.
    fn untyped_global_const_type(&self, def_id: DefId) -> Option<InternedTyId> {
        let binding = self.declarations.global(def_id)?;
        if binding.ty.is_some() {
            return None;
        }
        let value = binding.value.as_ref()?;
        if !self.hinting_globals.borrow_mut().insert(def_id) {
            return None;
        }
        let ty = self.expr_type_hint(value, &LocalTypeHints::new());
        self.hinting_globals.borrow_mut().remove(&def_id);
        ty
    }

    /// The type of the enum value that `variant`, named by `path`, builds:
    /// the value itself when `unit_only`, otherwise a constructor call's
    /// result. Enum arguments written in the path (`E[i32]::A`, or an alias
    /// naming them) give the type; without them only a non-generic enum has
    /// one. A constructor of a generic enum must write its arguments, and
    /// body checking reports a missing one, so such a path has the error
    /// type rather than an instance whose arguments do not match.
    fn enum_variant_value_type(
        &self,
        variant: GlobalDefId,
        path: &Expr,
        unit_only: bool,
    ) -> Option<InternedTyId> {
        let (enum_def, generic_count) =
            self.with_signatures(variant.module_id, |signatures| {
                signatures.enums.iter().find_map(|(id, signature)| {
                    let variant_signature = signature
                        .variants
                        .iter()
                        .find(|candidate| candidate.def_id == variant.def_id)?;
                    (!unit_only
                        || matches!(variant_signature.payload, EnumVariantPayloadSignature::Unit))
                    .then_some((*id, signature.generic_params.len()))
                })
            })??;
        let enum_id = GlobalDefId {
            module_id: variant.module_id,
            def_id: enum_def,
        };
        if let nia_ast::ExprKind::Qualified { lhs, .. } = &path.kind
            && let Some(ty) = self.qualified_aggregate_type(lhs)
        {
            return match self.type_kind(ty) {
                Some(TyKind::Nominal { def_id, .. }) if *def_id == enum_id => Some(ty),
                _ => None,
            };
        }
        if generic_count != 0 {
            return self.intern_type(TyKind::Error);
        }
        self.intern_type(TyKind::Nominal {
            def_id: enum_id,
            args: Vec::new(),
            const_args: Vec::new(),
        })
    }

    fn enum_variant_payload_types(
        &self,
        target: &Expr,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
    ) -> Option<EnumVariantPayloadSignature> {
        let variant_id = self.constructor_variant_id(target, expected_type, expected_ref)?;
        let foreign_signatures = (variant_id.module_id != self.input.defs.module_id)
            .then(|| (self.input.signatures_for_module?)(variant_id.module_id))
            .flatten();
        if variant_id.module_id != self.input.defs.module_id && foreign_signatures.is_none() {
            return None;
        }
        let signatures = foreign_signatures
            .as_deref()
            .unwrap_or(self.input.signatures);
        let (enum_def, signature, variant) =
            signatures.enums.iter().find_map(|(id, signature)| {
                let variant = signature
                    .variants
                    .iter()
                    .find(|v| v.def_id == variant_id.def_id)?;
                Some((*id, signature, variant))
            })?;
        let explicit_target = match &target.kind {
            nia_ast::ExprKind::Qualified { lhs, .. } => self.qualified_aggregate_type(lhs),
            _ => None,
        };
        let substitutions = explicit_target
            .into_iter()
            .chain(expected_type)
            .find_map(|ty| {
                let TyKind::Nominal {
                    def_id,
                    args,
                    const_args,
                } = self.type_kind(ty)?
                else {
                    return None;
                };
                (def_id.module_id == variant_id.module_id && def_id.def_id == enum_def)
                    .then(|| {
                        generic_argument_substitutions(&signature.generic_params, args, const_args)
                    })
                    .flatten()
            });
        let (type_substitutions, const_substitutions) = substitutions.unwrap_or_default();
        let substitute = |ty| match nia_ty::substitute_ty(
            self.input.type_store,
            &self
                .input
                .type_store
                .append_for_module(self.input.defs.module_id),
            ty,
            &|name| type_substitutions.get(name).copied(),
            &|name| const_substitutions.get(name).cloned(),
            None,
        ) {
            Ok(ty) => Some(ty),
            Err(error) => {
                self.internal_error.borrow_mut().get_or_insert(error);
                None
            }
        };
        match &variant.payload {
            EnumVariantPayloadSignature::Unit => Some(EnumVariantPayloadSignature::Unit),
            EnumVariantPayloadSignature::Tuple(types) => Some(EnumVariantPayloadSignature::Tuple(
                types
                    .iter()
                    .map(|ty| substitute(*ty))
                    .collect::<Option<_>>()?,
            )),
            EnumVariantPayloadSignature::Named(fields) => Some(EnumVariantPayloadSignature::Named(
                fields
                    .iter()
                    .map(|field| {
                        let mut field = field.clone();
                        field.ty = substitute(field.ty)?;
                        Some(field)
                    })
                    .collect::<Option<_>>()?,
            )),
        }
    }

    fn enum_variant_field_types(
        &self,
        target: &Expr,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
    ) -> Option<HashMap<nia_symbol::SymbolId, InternedTyId>> {
        let EnumVariantPayloadSignature::Named(fields) =
            self.enum_variant_payload_types(target, expected_type, expected_ref)?
        else {
            return None;
        };
        Some(
            fields
                .into_iter()
                .map(|field| (field.name, field.ty))
                .collect(),
        )
    }

    fn array_element_type_hint(
        &self,
        elems: &nia_ast::ArrayElements,
        local_types: &LocalTypeHints,
    ) -> Option<InternedTyId> {
        let type_hint = |value: &Expr| self.expr_type_hint(value, local_types);
        match elems {
            nia_ast::ArrayElements::List(values) => values.iter().find_map(type_hint),
            nia_ast::ArrayElements::Repeat { value, .. } => type_hint(value),
        }
    }

    fn collect_pattern_omitted_maps(
        &self,
        pattern: &nia_ast::Pattern,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
        maps: &mut ConstructorMaps,
        local_types: &mut LocalTypeHints,
    ) {
        match &pattern.kind {
            nia_ast::PatternKind::Bind { node_key, .. } => {
                if let (Some(local_id), Some(expected_type)) = (
                    self.input.semantic_uses.node_local_def(node_key),
                    expected_type,
                ) {
                    local_types.insert(local_id, (expected_type, expected_ref.cloned()));
                }
            }
            nia_ast::PatternKind::Expr(expr) => {
                self.collect_expr_omitted_maps(expr, expected_type, expected_ref, maps, local_types)
            }
            nia_ast::PatternKind::Nominal {
                constructor,
                fields,
            } => {
                self.collect_constructor_maps(constructor, expected_type, expected_ref, maps);
                if let Some(payload) =
                    self.enum_variant_payload_types(constructor, expected_type, expected_ref)
                {
                    match (&payload, fields) {
                        (
                            nia_item_signatures::EnumVariantPayloadSignature::Tuple(types),
                            nia_ast::NominalPatternFields::Tuple(patterns),
                        ) => {
                            for (pattern, ty) in patterns.iter().zip(types.iter().copied()) {
                                self.collect_pattern_omitted_maps(
                                    pattern,
                                    Some(ty),
                                    None,
                                    maps,
                                    local_types,
                                );
                            }
                        }
                        (
                            nia_item_signatures::EnumVariantPayloadSignature::Named(types),
                            nia_ast::NominalPatternFields::Named { fields, .. },
                        ) => {
                            for field in fields {
                                let ty = types
                                    .iter()
                                    .find(|expected| expected.name == field.name)
                                    .map(|expected| expected.ty);
                                self.collect_pattern_omitted_maps(
                                    &field.pattern,
                                    ty,
                                    None,
                                    maps,
                                    local_types,
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                let elem = expected_type.and_then(|ty| match self.type_kind(ty) {
                    Some(TyKind::Pointer { elem, .. }) => Some(*elem),
                    _ => None,
                });
                self.collect_pattern_omitted_maps(inner, elem, None, maps, local_types);
            }
            nia_ast::PatternKind::OptionalSome(inner) => {
                let elem = expected_type.and_then(|ty| match self.type_kind(ty) {
                    Some(TyKind::Optional { elem }) => Some(*elem),
                    _ => None,
                });
                self.collect_pattern_omitted_maps(inner, elem, None, maps, local_types);
            }
            nia_ast::PatternKind::ErrorOk(inner) | nia_ast::PatternKind::ErrorErr(inner) => {
                let error_arm = matches!(pattern.kind, nia_ast::PatternKind::ErrorErr(_));
                let elem = expected_type.and_then(|ty| match self.type_kind(ty) {
                    Some(TyKind::ErrorUnion { error, value }) => {
                        Some(if error_arm { *error } else { *value })
                    }
                    _ => None,
                });
                self.collect_pattern_omitted_maps(inner, elem, None, maps, local_types);
            }
            nia_ast::PatternKind::Tuple(items) => {
                let types = expected_type
                    .and_then(|ty| match self.type_kind(ty) {
                        Some(TyKind::Tuple(types)) => Some(types.as_slice()),
                        _ => None,
                    })
                    .unwrap_or(&[]);
                for (index, item) in items.iter().enumerate() {
                    self.collect_pattern_omitted_maps(
                        item,
                        types.get(index).copied(),
                        None,
                        maps,
                        local_types,
                    );
                }
            }
            _ => {}
        }
    }

    fn constructor_variant_id(
        &self,
        constructor: &Expr,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
    ) -> Option<GlobalDefId> {
        if let Some(SemanticValueUse::Global(def_id)) = self
            .input
            .semantic_uses
            .node_value_use(&constructor.node_key)
        {
            return Some(def_id);
        }
        let (enum_id, name) = match &constructor.kind {
            nia_ast::ExprKind::OmittedMember { name } => {
                (self.enum_def_for_type(expected_type, expected_ref)?, name)
            }
            nia_ast::ExprKind::Qualified { lhs, name, .. } => (self.qualified_enum_def(lhs)?, name),
            _ => return None,
        };
        self.enum_variant_by_name(enum_id, *name)
    }

    fn enum_variant_by_name(
        &self,
        enum_id: GlobalDefId,
        name: nia_symbol::SymbolId,
    ) -> Option<GlobalDefId> {
        let def_id = if enum_id.module_id == self.input.defs.module_id {
            self.input
                .defs
                .scopes
                .enum_members
                .get(&enum_id.def_id)?
                .variants
                .get(&name)?
        } else {
            let defs = (self.input.defs_for_module?)(enum_id.module_id)?;
            defs.scopes
                .enum_members
                .get(&enum_id.def_id)?
                .variants
                .get(&name)?
        };
        Some(GlobalDefId {
            module_id: enum_id.module_id,
            def_id,
        })
    }

    fn omitted_associated_maps(
        &self,
        expr: &Expr,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
    ) -> HashMap<VersionedNodeKey, InternedTyId> {
        let Some(expected_type) = expected_type else {
            return HashMap::new();
        };
        if self
            .enum_def_for_type(Some(expected_type), expected_ref)
            .is_some()
        {
            return HashMap::new();
        }
        let nia_ast::ExprKind::Call { callee, .. } = &expr.kind else {
            return HashMap::new();
        };
        if !matches!(callee.kind, nia_ast::ExprKind::OmittedMember { .. }) {
            return HashMap::new();
        }
        HashMap::from([(callee.node_key.clone(), expected_type)])
    }

    fn semantic_uses_with_allowed_locals(
        &self,
        allowed_locals: &HashSet<LocalId>,
    ) -> SemanticUseTable {
        let node_value_uses = self.input.semantic_uses.node_value_uses.filter_map_values(
            |value_use| match value_use {
                SemanticValueUse::Local(local_id)
                    if allowed_locals.contains(local_id)
                        || self
                            .input
                            .locals
                            .locals
                            .get(*local_id)
                            .is_some_and(|local| local.kind == LocalKind::ConstBinding) =>
                {
                    Some(*value_use)
                }
                SemanticValueUse::Global(_) => Some(*value_use),
                SemanticValueUse::Local(_) => None,
            },
        );
        SemanticUseTable {
            node_value_uses,
            node_builtin_associated_values: self
                .input
                .semantic_uses
                .node_builtin_associated_values
                .clone(),
            node_associated_const_projections: self
                .input
                .semantic_uses
                .node_associated_const_projections
                .clone(),
            node_const_generic_uses: self.input.semantic_uses.node_const_generic_uses.clone(),
            node_local_defs: self.input.semantic_uses.node_local_defs.clone(),
            node_type_uses: self.input.semantic_uses.node_type_uses.clone(),
            node_type_prefixes: self.input.semantic_uses.node_type_prefixes.clone(),
        }
    }

    fn function_locals(&self, function: &nia_ast::FunctionItem) -> HashSet<LocalId> {
        let mut locals = HashSet::new();
        for param in &function.params {
            if let Some(local_id) = self.input.semantic_uses.node_local_def(&param.node_key) {
                locals.insert(local_id);
            }
        }
        if let Some(body) = &function.body {
            self.collect_block_locals(body, &mut locals);
        }
        locals
    }

    fn collect_block_locals(&self, block: &nia_ast::Block, out: &mut HashSet<LocalId>) {
        for stmt in &block.stmts {
            self.collect_stmt_locals(stmt, out);
        }
        if let Some(tail) = block.tail.as_deref() {
            self.collect_expr_locals(tail, out);
        }
    }

    fn collect_stmt_locals(&self, stmt: &nia_ast::Stmt, out: &mut HashSet<LocalId>) {
        match &stmt.kind {
            nia_ast::StmtKind::Binding(binding) => {
                self.collect_pattern_locals(&binding.pattern, out);
                if let Some(value) = &binding.value {
                    self.collect_expr_locals(value, out);
                }
            }
            nia_ast::StmtKind::Static(binding) => {
                if let Some(value) = &binding.value {
                    self.collect_expr_locals(value, out);
                }
            }
            nia_ast::StmtKind::Expr(expr)
            | nia_ast::StmtKind::Return(Some(expr))
            | nia_ast::StmtKind::Defer(expr) => self.collect_expr_locals(expr, out),
            nia_ast::StmtKind::ForIn(for_stmt) => {
                self.collect_pattern_locals(&for_stmt.pattern, out);
                self.collect_expr_locals(&for_stmt.iter, out);
                self.collect_block_locals(&for_stmt.body, out);
            }
            nia_ast::StmtKind::While(while_stmt) => {
                self.collect_expr_locals(&while_stmt.cond, out);
                self.collect_block_locals(&while_stmt.body, out);
            }
            nia_ast::StmtKind::Loop(loop_stmt) => self.collect_block_locals(&loop_stmt.body, out),
            nia_ast::StmtKind::Using(_)
            | nia_ast::StmtKind::Return(None)
            | nia_ast::StmtKind::Break
            | nia_ast::StmtKind::Continue => {}
        }
    }

    fn collect_expr_locals(&self, expr: &nia_ast::Expr, out: &mut HashSet<LocalId>) {
        match &expr.kind {
            nia_ast::ExprKind::BracketSuffix { callee, args } => {
                self.collect_expr_locals(callee, out);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        self.collect_expr_locals(expr, out);
                    }
                }
            }
            nia_ast::ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.collect_expr_locals(elem, out);
                }
            }
            nia_ast::ExprKind::Closure { captures, body, .. } => {
                for capture in captures {
                    self.collect_expr_locals(&capture.value, out);
                }
                self.collect_expr_locals(body, out);
            }
            nia_ast::ExprKind::TupleField { lhs, .. } => self.collect_expr_locals(lhs, out),
            nia_ast::ExprKind::ArrayLiteral { elems } => match elems {
                nia_ast::ArrayElements::List(elems) => {
                    for elem in elems {
                        self.collect_expr_locals(elem, out);
                    }
                }
                nia_ast::ArrayElements::Repeat { value, count } => {
                    self.collect_expr_locals(value, out);
                    self.collect_expr_locals(count, out);
                }
            },
            nia_ast::ExprKind::TypedStructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_expr_locals(&field.value, out);
                }
            }
            nia_ast::ExprKind::QualifiedStructLiteral { target, fields } => {
                self.collect_expr_locals(target, out);
                for field in fields {
                    self.collect_expr_locals(&field.value, out);
                }
            }
            nia_ast::ExprKind::OmittedAggregateLiteral { fields } => {
                for field in fields {
                    self.collect_expr_locals(&field.value, out);
                }
            }
            nia_ast::ExprKind::OmittedMember { .. } => {}
            nia_ast::ExprKind::Unary { expr, .. }
            | nia_ast::ExprKind::OptionalSome { expr }
            | nia_ast::ExprKind::ErrorOk { expr }
            | nia_ast::ExprKind::ErrorErr { expr }
            | nia_ast::ExprKind::Try { expr }
            | nia_ast::ExprKind::Cast { expr, .. } => self.collect_expr_locals(expr, out),
            nia_ast::ExprKind::Binary { lhs, rhs, .. }
            | nia_ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.collect_expr_locals(lhs, out);
                self.collect_expr_locals(rhs, out);
            }
            nia_ast::ExprKind::Call { callee, args } => {
                self.collect_expr_locals(callee, out);
                for arg in args {
                    self.collect_expr_locals(arg, out);
                }
            }
            nia_ast::ExprKind::Qualified { lhs, .. } | nia_ast::ExprKind::Field { lhs, .. } => {
                self.collect_expr_locals(lhs, out);
            }
            nia_ast::ExprKind::Index { lhs, index } => {
                self.collect_expr_locals(lhs, out);
                match index {
                    nia_ast::IndexArg::Expr(index) => self.collect_expr_locals(index, out),
                    nia_ast::IndexArg::Range(range) => self.collect_range_locals(range, out),
                }
            }
            nia_ast::ExprKind::Ident(_) => {
                if let Some(SemanticValueUse::Local(local_id)) =
                    self.input.semantic_uses.node_value_use(&expr.node_key)
                {
                    out.insert(local_id);
                }
            }
            nia_ast::ExprKind::Range(range) => self.collect_range_locals(range, out),
            nia_ast::ExprKind::Block(block) => self.collect_block_locals(block, out),
            nia_ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_expr_locals(cond, out);
                self.collect_block_locals(then_branch, out);
                if let Some(else_branch) = else_branch.as_deref() {
                    self.collect_expr_locals(else_branch, out);
                }
            }
            nia_ast::ExprKind::IfPattern(if_pattern) => {
                self.collect_expr_locals(&if_pattern.target, out);
                self.collect_pattern_locals(&if_pattern.pattern, out);
                self.collect_block_locals(&if_pattern.then_branch, out);
                if let Some(else_branch) = if_pattern.else_branch.as_deref() {
                    self.collect_expr_locals(else_branch, out);
                }
            }
            nia_ast::ExprKind::IfPatternChain(chain) => {
                for clause in &chain.clauses {
                    match clause {
                        nia_ast::IfPatternChainClause::Pattern { target, pattern } => {
                            self.collect_expr_locals(target, out);
                            self.collect_pattern_locals(pattern, out);
                        }
                        nia_ast::IfPatternChainClause::Condition(condition) => {
                            self.collect_expr_locals(condition, out);
                        }
                    }
                }
                self.collect_block_locals(&chain.then_branch, out);
                if let Some(else_branch) = chain.else_branch.as_deref() {
                    self.collect_expr_locals(else_branch, out);
                }
            }
            nia_ast::ExprKind::Match(matched) => {
                self.collect_expr_locals(&matched.target, out);
                for arm in &matched.arms {
                    for pattern in &arm.patterns {
                        self.collect_pattern_locals(pattern, out);
                    }
                    match &arm.body {
                        nia_ast::MatchArmBody::Expr(expr) => self.collect_expr_locals(expr, out),
                        nia_ast::MatchArmBody::Stmt(stmt) => self.collect_stmt_locals(stmt, out),
                        nia_ast::MatchArmBody::Block(block) => {
                            self.collect_block_locals(block, out)
                        }
                    }
                }
            }
            nia_ast::ExprKind::Error
            | nia_ast::ExprKind::Integer(_)
            | nia_ast::ExprKind::Float(_)
            | nia_ast::ExprKind::String(_)
            | nia_ast::ExprKind::ByteString(_)
            | nia_ast::ExprKind::Char(_)
            | nia_ast::ExprKind::ByteChar(_)
            | nia_ast::ExprKind::Raw(_)
            | nia_ast::ExprKind::Bool(_)
            | nia_ast::ExprKind::Null
            | nia_ast::ExprKind::SelfValue
            | nia_ast::ExprKind::PathRoot(_)
            | nia_ast::ExprKind::Underscore
            | nia_ast::ExprKind::TypeTarget { .. }
            | nia_ast::ExprKind::TraitTarget { .. } => {}
        }
    }

    fn collect_range_locals(&self, range: &nia_ast::SliceRange, out: &mut HashSet<LocalId>) {
        if let Some(start) = range.start.as_deref() {
            self.collect_expr_locals(start, out);
        }
        if let Some(end) = range.end.as_deref() {
            self.collect_expr_locals(end, out);
        }
    }

    fn collect_pattern_locals(&self, pattern: &nia_ast::Pattern, out: &mut HashSet<LocalId>) {
        match &pattern.kind {
            nia_ast::PatternKind::Bind { node_key, .. } => {
                if let Some(local_id) = self.input.semantic_uses.node_local_def(node_key) {
                    out.insert(local_id);
                }
            }
            nia_ast::PatternKind::Pointer(pattern)
            | nia_ast::PatternKind::MutPointer(pattern)
            | nia_ast::PatternKind::OptionalSome(pattern)
            | nia_ast::PatternKind::ErrorOk(pattern)
            | nia_ast::PatternKind::ErrorErr(pattern) => self.collect_pattern_locals(pattern, out),
            nia_ast::PatternKind::Tuple(patterns) => {
                for pattern in patterns {
                    self.collect_pattern_locals(pattern, out);
                }
            }
            nia_ast::PatternKind::Nominal {
                constructor: variant,
                fields,
            } => {
                self.collect_expr_locals(variant, out);
                match fields {
                    nia_ast::NominalPatternFields::Tuple(fields) => {
                        for field in fields {
                            self.collect_pattern_locals(field, out);
                        }
                    }
                    nia_ast::NominalPatternFields::Named { fields, .. } => {
                        for field in fields {
                            self.collect_pattern_locals(&field.pattern, out);
                        }
                    }
                }
            }
            nia_ast::PatternKind::Expr(expr) => self.collect_expr_locals(expr, out),
            nia_ast::PatternKind::Range { start, end, .. } => {
                self.collect_expr_locals(start, out);
                self.collect_expr_locals(end, out);
            }
            nia_ast::PatternKind::Wildcard | nia_ast::PatternKind::OptionalNull => {}
        }
    }

    fn def_id_for_node(
        &self,
        node_key: &VersionedNodeKey,
        _span: Span,
        expected: DefKind,
    ) -> Option<DefId> {
        let def_id = self.input.defs.def_nodes.get(node_key)?;
        let def = self.input.defs.defs.get(def_id)?;
        (def.kind == expected).then_some(def_id)
    }

    fn global_def_id(&self, def_id: DefId) -> GlobalDefId {
        GlobalDefId {
            module_id: self.input.defs.module_id,
            def_id,
        }
    }
}
