// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) enum ModuleDefs<'a> {
    Borrowed(&'a DefCollection),
    Shared(Arc<DefCollection>),
}

impl ModuleDefs<'_> {
    pub(super) fn as_ref(&self) -> &DefCollection {
        match self {
            ModuleDefs::Borrowed(defs) => defs,
            ModuleDefs::Shared(defs) => defs,
        }
    }
}

impl<'a> BodyChecker<'a> {
    pub(super) fn reject_const_operation(&mut self, span: Span, summary: impl Into<String>) {
        if self.body_filter.checks_const_declarations() {
            self.diagnostics
                .push(Diagnostic::user_error_at(codes::CONST, span, summary));
        }
    }

    pub(super) fn record_expr_node_type(&mut self, expr: &Expr, ty: InternedTyId) {
        let ty = self.normalize_projection(ty);
        self.node_expr_types.insert(expr.node_key.clone(), ty);
        let global_value_use = match self.semantic_uses.node_value_use(&expr.node_key) {
            Some(SemanticValueUse::Global(def_id)) => Some(def_id),
            Some(SemanticValueUse::Local(_)) | None => None,
        };
        if let Some(facts) = self.current_function_facts() {
            facts.node_expr_types.insert(expr.node_key.clone(), ty);
            if let Some(def_id) = global_value_use {
                facts.global_value_uses.insert(def_id);
            }
        }
    }

    pub(super) fn record_bracket_suffix_node_resolution(
        &mut self,
        expr: &Expr,
        resolution: BracketSuffixResolution,
    ) {
        self.node_bracket_suffix_resolutions
            .insert(expr.node_key.clone(), resolution);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_bracket_suffix_resolutions
                .insert(expr.node_key.clone(), resolution);
        }
    }

    pub(super) fn record_resolved_node_call(
        &mut self,
        span: Span,
        key: &VersionedNodeKey,
        call: ResolvedCall,
    ) {
        if self.in_const_context() {
            return;
        }
        if self.body_filter.checks_const_declarations()
            && !self.node_resolved_calls.contains_key(key)
            && !self.resolved_call_is_const_capable(&call)
        {
            let summary = match &call {
                ResolvedCall::BuiltinFunction { builtin, .. } => {
                    format!(
                        "builtin `{}` is not available during const evaluation",
                        builtin.name()
                    )
                }
                ResolvedCall::BuiltinMethod {
                    method: nia_sema_ir::BuiltinMethod::Iter,
                    ..
                } => "`Iterable::iter` trait witness used during const evaluation must be declared `const fn`"
                    .to_string(),
                ResolvedCall::BuiltinMethod { method, .. } => format!(
                    "builtin method `{}` is not available during const evaluation",
                    method.name()
                ),
                ResolvedCall::BuiltinPlaceMethod {
                    method: BuiltinTraitMethod::IteratorNext,
                    ..
                } => "`Iterator::next` trait witness used during const evaluation must be declared `const fn`"
                    .to_string(),
                ResolvedCall::BuiltinPlaceMethod { method, .. } => format!(
                    "builtin method `{}` is not available during const evaluation",
                    method.name()
                ),
                ResolvedCall::Closure => {
                    "closure calls are not available during const evaluation".to_string()
                }
                ResolvedCall::FunctionPointer => {
                    "indirect function calls are not available during const evaluation".to_string()
                }
                _ => "const expression can only call `const fn`".to_string(),
            };
            self.diagnostics
                .push(Diagnostic::user_error_at(codes::CONST, span, summary));
        }
        self.enqueue_same_module_resolved_call(&call);
        self.node_resolved_calls.insert(key.clone(), call.clone());
        if let Some(facts) = self.current_function_facts() {
            facts.node_resolved_calls.insert(key.clone(), call);
        }
    }

    fn resolved_call_is_const_capable(&mut self, call: &ResolvedCall) -> bool {
        let def_id = match call {
            ResolvedCall::Function(def_id)
            | ResolvedCall::FunctionInstance { def_id, .. }
            | ResolvedCall::Method { def_id, .. } => *def_id,
            ResolvedCall::TraitMethod { method_id, .. }
            | ResolvedCall::TraitAssociatedFunction { method_id, .. } => *method_id,
            ResolvedCall::DynamicTraitMethod { .. }
            | ResolvedCall::Closure
            | ResolvedCall::FunctionPointer => {
                return false;
            }
            ResolvedCall::BuiltinFunction { builtin, .. } => return builtin.is_const_capable(),
            ResolvedCall::BuiltinTraitMethod { op, .. } => {
                return op.is_const_capable();
            }
            ResolvedCall::BuiltinMethod { method, self_ty } => {
                return method.is_const_capable()
                    && (!matches!(method, nia_sema_ir::BuiltinMethod::Iter)
                        || self.builtin_trait_witness_is_const_capable(
                            *self_ty,
                            BuiltinTraitMethod::IterableIter,
                        ));
            }
            ResolvedCall::BuiltinPlaceMethod {
                method, self_ty, ..
            } => {
                return method.is_const_capable()
                    && (!matches!(method, BuiltinTraitMethod::IteratorNext)
                        || self.builtin_trait_witness_is_const_capable(*self_ty, *method));
            }
        };
        self.resolved_function_signature(def_id)
            .is_some_and(|resolved| resolved.signature.is_const)
    }

    pub(super) fn builtin_trait_witness_is_const_capable(
        &mut self,
        self_ty: InternedTyId,
        method: BuiltinTraitMethod,
    ) -> bool {
        let trait_id = nia_ty::TraitId::Builtin(method.trait_id());
        let resolution =
            self.current_context_resolve_trait_obligation(self_ty, trait_id, Vec::new());
        let nia_trait_solve::TraitResolution::User(user_impl) = resolution else {
            // Ordinary trait checking owns unsatisfied and ambiguous diagnostics.
            // Intrinsic and assumed obligations have no concrete user witness.
            return true;
        };
        let Some(impl_signature) = self.program_trait_impls.get(user_impl.impl_index) else {
            return false;
        };
        let impl_module_id = impl_signature.module_id;
        let impl_id = impl_signature.impl_id;
        let method_id = self
            .with_visible_extensions(|extensions| {
                extensions.all_trait_witnesses_named(&method.symbol_id())
            })
            .into_iter()
            .find_map(|(_, witness)| {
                (witness.def_id.module_id == impl_module_id
                    && witness.impl_id == impl_id
                    && witness.trait_id == Some(trait_id))
                .then_some(witness.def_id)
            });
        method_id
            .and_then(|method_id| self.resolved_function_signature(method_id))
            .is_some_and(|resolved| resolved.signature.is_const)
    }

    pub(super) fn record_trait_method_ref(&mut self, reference: SemanticTraitMethodRef) {
        if let Some(facts) = self.current_function_facts() {
            facts.trait_method_refs.push(reference);
        }
    }

    pub(super) fn record_builtin_trait_method_ref(
        &mut self,
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
    ) {
        self.record_trait_method_ref(SemanticTraitMethodRef {
            module_id: self.timing_module_id,
            trait_id: nia_ty::TraitId::Builtin(method.trait_id()),
            method_name: method.symbol_id(),
            self_ty,
            trait_args,
        });
    }

    pub(super) fn enqueue_same_module_resolved_call(&mut self, call: &ResolvedCall) {
        let def_id = match call {
            ResolvedCall::Function(def_id)
            | ResolvedCall::FunctionInstance { def_id, .. }
            | ResolvedCall::Method { def_id, .. } => *def_id,
            ResolvedCall::TraitMethod { method_id, .. }
            | ResolvedCall::TraitAssociatedFunction { method_id, .. } => *method_id,
            ResolvedCall::DynamicTraitMethod { .. }
            | ResolvedCall::BuiltinFunction { .. }
            | ResolvedCall::BuiltinTraitMethod { .. }
            | ResolvedCall::BuiltinMethod { .. }
            | ResolvedCall::BuiltinPlaceMethod { .. }
            | ResolvedCall::Closure
            | ResolvedCall::FunctionPointer => return,
        };
        if Some(def_id.module_id) != self.current_def_id.map(|current| current.module_id) {
            return;
        }
        if self.checked_functions.contains(&def_id) {
            return;
        }
        if self.body_filter.add_function(def_id) {
            self.pending_functions.push_back(def_id);
        }
    }

    pub(super) fn record_pointer_array_to_slice_node_coercion(
        &mut self,
        expr: &Expr,
        coercion: PointerArrayToSliceCoercion,
    ) {
        self.node_pointer_array_to_slice_coercions
            .insert(expr.node_key.clone(), coercion);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_pointer_array_to_slice_coercions
                .insert(expr.node_key.clone(), coercion);
        }
    }

    pub(super) fn record_trait_object_node_coercion(
        &mut self,
        expr: &Expr,
        coercion: TraitObjectCoercion,
    ) {
        self.node_trait_object_coercions
            .insert(expr.node_key.clone(), coercion);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_trait_object_coercions
                .insert(expr.node_key.clone(), coercion);
        }
    }

    pub(super) fn record_trait_object_node_upcast(
        &mut self,
        expr: &Expr,
        upcast: TraitObjectUpcast,
    ) {
        self.node_trait_object_upcasts
            .insert(expr.node_key.clone(), upcast);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_trait_object_upcasts
                .insert(expr.node_key.clone(), upcast);
        }
    }

    pub(super) fn record_builtin_node_value(&mut self, expr: &Expr, value: BuiltinValue) {
        self.node_builtin_values
            .insert(expr.node_key.clone(), value.clone());
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_builtin_values
                .insert(expr.node_key.clone(), value);
        }
    }

    pub(super) fn record_associated_const_projection(
        &mut self,
        expr: &Expr,
        projection: AssociatedConstProjection,
    ) {
        self.node_associated_const_projections
            .insert(expr.node_key.clone(), projection.clone());
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_associated_const_projections
                .insert(expr.node_key.clone(), projection);
        }
    }

    pub(super) fn record_function_node_reference(
        &mut self,
        _span: Span,
        key: &VersionedNodeKey,
        reference: FunctionReference,
    ) {
        self.node_function_references
            .insert(key.clone(), reference.clone());
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_function_references
                .insert(key.clone(), reference);
        }
    }

    pub(super) fn record_array_repeat_count(&mut self, expr: &Expr, value: u64) {
        self.node_array_repeat_counts
            .insert(expr.node_key.clone(), value);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_array_repeat_counts
                .insert(expr.node_key.clone(), value);
        }
    }

    pub(super) fn record_pattern_value(&mut self, expr: &Expr, value: i128) {
        self.node_pattern_values
            .insert(expr.node_key.clone(), value);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_pattern_values
                .insert(expr.node_key.clone(), value);
        }
    }

    pub(super) fn record_local_type(&mut self, local_id: LocalId, ty: InternedTyId) {
        let ty = self.normalize_aliases_in_type(ty);
        self.local_types.insert(local_id, ty);
        if let Some(facts) = self.current_function_facts() {
            facts.local_types.insert(local_id, ty);
        }
    }

    pub(super) fn current_function_facts(&mut self) -> Option<&mut FunctionSemanticFactsBuilder> {
        self.current_def_id
            .map(|def_id| self.function_facts.entry(def_id).or_default())
    }

    pub(super) fn expr_ty(&mut self, expr: &Expr) -> Option<InternedTyId> {
        if let Some(ty) = self.node_expr_types.get(&expr.node_key).copied() {
            return Some(ty);
        }
        if let Some(nia_local_resolve::LocalUse::Local(local_id)) = self.local_use(expr)
            && let Some(ty) = self.local_types.get(&local_id).copied()
        {
            return Some(ty);
        }
        let ty = self.type_lowering.ty_for_key(&expr.node_key)?;
        Some(ty)
    }

    pub(super) fn bracket_suffix_resolution(&self, expr: &Expr) -> Option<BracketSuffixResolution> {
        self.node_bracket_suffix_resolutions
            .get(&expr.node_key)
            .copied()
    }

    pub(super) fn resolved_call(&self, expr: &Expr) -> Option<ResolvedCall> {
        self.node_resolved_calls.get(&expr.node_key).cloned()
    }

    pub(super) fn function_reference(&self, expr: &Expr) -> Option<&FunctionReference> {
        self.node_function_references.get(&expr.node_key)
    }

    pub(super) fn builtin_value(&self, expr: &Expr) -> Option<&BuiltinValue> {
        self.node_builtin_values.get(&expr.node_key)
    }

    pub(super) fn local_def(&self, key: &VersionedNodeKey) -> Option<LocalId> {
        self.locals.node_local_defs.get(key).copied()
    }

    pub(super) fn local_use(&self, expr: &Expr) -> Option<nia_local_resolve::LocalUse> {
        self.locals.node_uses.get(&expr.node_key).copied()
    }

    pub(super) fn value_name(&self, expr: &Expr) -> Option<nia_value_resolve::ValueNameResolution> {
        self.values.node_names.get(&expr.node_key).copied()
    }

    pub(super) fn qualified_value(&self, expr: &Expr) -> Option<GlobalDefId> {
        let global_id = self
            .values
            .node_qualified_values
            .get(&expr.node_key)
            .copied()?;
        match self.semantic_uses.node_value_use(&expr.node_key) {
            Some(SemanticValueUse::Global(value_use)) if value_use == global_id => Some(global_id),
            _ => None,
        }
    }

    pub(super) fn variant_enum(&self, expr: &Expr) -> Option<GlobalDefId> {
        self.values.node_variant_enums.get(&expr.node_key).copied()
    }

    pub(super) fn qualified_type_prefix(&self, expr: &Expr) -> Option<GlobalDefId> {
        self.values
            .node_qualified_type_prefixes
            .get(&expr.node_key)
            .copied()
    }

    pub(super) fn with_const_context<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.const_context_depth += 1;
        let result = f(self);
        self.const_context_depth -= 1;
        result
    }

    pub(super) fn in_const_context(&self) -> bool {
        self.const_context_depth > 0
    }

    pub(super) fn defs_for_module(&self, module_id: ModuleId) -> Option<ModuleDefs<'_>> {
        if module_id == self.defs.module_id {
            Some(ModuleDefs::Borrowed(self.defs))
        } else {
            Some(ModuleDefs::Shared((self.program.defs?)(module_id)?))
        }
    }
}
