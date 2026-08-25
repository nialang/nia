// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl<'a> BodyChecker<'a> {
    pub(crate) fn profile_stage<T>(
        &mut self,
        name: &'static str,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        if !self.timing {
            return f(self);
        }
        let mut profile = std::mem::take(&mut self.profile);
        let result = profile.time(name, || f(self));
        self.profile = profile;
        result
    }

    pub(super) fn print_profile(&self) {
        if !self.timing || self.profile.is_empty() {
            return;
        }
        self.profile
            .emit_query_timings(|name| format!("{name}[{:?}]", self.timing_module_id));
    }

    pub(super) fn load_checked_body_facts(
        &mut self,
        module_id: ModuleId,
        prechecked: PrecheckedBodyCheck,
    ) {
        let PrecheckedBodyCheck {
            ir,
            facts,
            checked_functions,
            diagnostic_owners,
            diagnostics,
        } = prechecked;
        self.global_inits = ir.global_inits;
        self.checked_functions = checked_functions;
        self.diagnostic_owners = diagnostic_owners;
        self.diagnostics = diagnostics;
        self.load_type_facts(module_id, &facts);
        let facts = facts.into_builder();
        self.generic_instantiations = facts.generic_instantiations;
        self.function_facts = facts
            .function_facts
            .into_iter()
            .map(|(def_id, facts)| (def_id, facts.into_builder()))
            .collect();
        self.node_expr_types = facts.node_expr_types;
        self.node_bracket_suffix_resolutions = facts.node_bracket_suffix_resolutions;
        self.node_pointer_array_to_slice_coercions = facts.node_pointer_array_to_slice_coercions;
        self.node_trait_object_coercions = facts.node_trait_object_coercions;
        self.node_trait_object_upcasts = facts.node_trait_object_upcasts;
        self.node_builtin_values = facts.node_builtin_values;
        self.node_associated_const_projections = facts.node_associated_const_projections;
        self.node_array_repeat_counts = facts.node_array_repeat_counts;
        self.node_pattern_values = facts.node_pattern_values;
        self.node_resolved_calls = facts.node_resolved_calls;
        self.node_function_references = facts.node_function_references;
    }

    pub(super) fn load_type_facts(&mut self, module_id: ModuleId, facts: &SemanticFacts) {
        self.global_types
            .extend(facts.global_types.iter().filter_map(|(def_id, ty)| {
                (def_id.module_id == module_id).then_some((def_id.def_id, *ty))
            }));
        self.const_types
            .extend(facts.const_types.iter().filter_map(|(def_id, ty)| {
                (def_id.module_id == module_id).then_some((def_id.def_id, *ty))
            }));
    }

    pub(super) fn lower_checked_module(
        &mut self,
        active_item_tree: &ActiveModuleItemTree,
        timing: bool,
        module_id: ModuleId,
    ) {
        let function_items = self.function_items_by_id(active_item_tree);
        let functions = self.checked_functions.iter().copied().collect::<Vec<_>>();
        for def_id in functions {
            self.lower_checked_function_by_id(def_id, &function_items, timing, module_id);
        }
    }

    pub(super) fn lower_checked_static_inits(&mut self, active_item_tree: &ActiveModuleItemTree) {
        for item in &active_item_tree.items {
            if let ItemTreeNodeKind::Binding(binding) = &item.kind
                && !binding.is_const()
            {
                self.lower_checked_static_init(item.span, binding);
            }
        }

        let function_items = self.function_items_by_id(active_item_tree);
        let mut visitor = CheckedStaticInitVisitor { checker: self };
        for item in function_items.values() {
            visitor.visit_function(item.function);
        }
    }

    pub(super) fn lower_checked_static_init(
        &mut self,
        item_span: Span,
        binding: &nia_ast::BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Global)
        else {
            return;
        };
        let global_def_id = self.global_def_id(def_id);
        if !self.body_filter.selects_global(global_def_id) {
            return;
        }
        let Some(value) = &binding.value else {
            return;
        };
        let Some(global_ty) = self.global_types.get(&def_id).copied() else {
            return;
        };
        if global_ty == self.error() {
            return;
        }
        // A refused initializer must not degrade into the uninitialized-static
        // zero default: `static x: T = <refused>;` and `static x: T;` publish
        // the same absent entry, and the latter is legitimately zeroed. The
        // refusal is therefore only safe when it carried a diagnostic.
        let diagnostics_before = self.diagnostics.len();
        if let Some(init) = self.lower_global_static_init_checked(value, global_ty) {
            self.global_inits.insert(global_def_id, Arc::new(init));
        } else {
            debug_assert!(
                self.diagnostics.len() > diagnostics_before,
                "a refused static initializer must report a diagnostic; \
                 otherwise the global silently zero-initializes"
            );
        }
    }

    pub(super) fn lower_checked_function_by_id<'ast>(
        &mut self,
        def_id: GlobalDefId,
        function_items: &HashMap<GlobalDefId, FunctionItemRef<'ast>>,
        timing: bool,
        module_id: ModuleId,
    ) {
        if !self.body_filter.includes_function(def_id) {
            return;
        }
        let Some(item) = function_items.get(&def_id) else {
            return;
        };
        let stage = match item.kind {
            DefKind::Function => "body_check.lower_checked.function",
            DefKind::TraitMethod => "body_check.lower_checked.trait_method",
            DefKind::Method => "body_check.lower_checked.extend_method",
            _ => "body_check.lower_checked.function",
        };
        time_body_stage_if_slow(
            timing,
            stage,
            module_id,
            mangle_symbol_id(item.function.name),
            0.020,
            || {
                self.lower_checked_function_with_kind(item.kind, item.function);
            },
        );
    }

    pub(super) fn lower_checked_function_with_kind(
        &mut self,
        kind: DefKind,
        function: &FunctionItem,
    ) {
        let expected = match kind {
            DefKind::Function => DefKind::Function,
            DefKind::Method => DefKind::Method,
            DefKind::TraitMethod => DefKind::TraitMethod,
            _ => return,
        };
        let Some(def_id) = self.def_id_for_node(&function.node_key, function.span, expected) else {
            return;
        };
        let global_def_id = self.global_def_id(def_id);
        if !self
            .program_signature_scope
            .includes_function(global_def_id)
        {
            return;
        }
        let Some(signature) = self.function_signature_for_body(def_id, global_def_id) else {
            return;
        };
        let Some(body) = &function.body else {
            return;
        };
        let previous_return = self.current_return;
        let previous_def_id = self.current_def_id;
        let previous_param_locals = std::mem::take(&mut self.current_param_locals);
        let previous_local_types = std::mem::take(&mut self.local_types);
        let previous_node_expr_types = std::mem::take(&mut self.node_expr_types);
        let previous_node_bracket_suffix_resolutions =
            std::mem::take(&mut self.node_bracket_suffix_resolutions);
        let previous_node_pointer_array_to_slice_coercions =
            std::mem::take(&mut self.node_pointer_array_to_slice_coercions);
        let previous_node_trait_object_coercions =
            std::mem::take(&mut self.node_trait_object_coercions);
        let previous_node_trait_object_upcasts =
            std::mem::take(&mut self.node_trait_object_upcasts);
        let previous_node_builtin_values = std::mem::take(&mut self.node_builtin_values);
        let previous_node_associated_const_projections =
            std::mem::take(&mut self.node_associated_const_projections);
        let previous_node_array_repeat_counts = std::mem::take(&mut self.node_array_repeat_counts);
        let previous_node_pattern_values = std::mem::take(&mut self.node_pattern_values);
        let previous_node_resolved_calls = std::mem::take(&mut self.node_resolved_calls);
        let previous_node_function_references = std::mem::take(&mut self.node_function_references);
        let function_facts = self
            .function_facts
            .get(&global_def_id)
            .cloned()
            .unwrap_or_default();
        self.current_return = signature.return_type;
        self.current_def_id = Some(global_def_id);
        self.next_closure_ordinal = 0;
        self.current_param_locals = function
            .params
            .iter()
            .filter_map(|param| self.local_def(&param.node_key))
            .collect();
        self.local_types = function_facts.local_types;
        self.node_expr_types = function_facts.node_expr_types;
        self.node_bracket_suffix_resolutions = function_facts.node_bracket_suffix_resolutions;
        self.node_pointer_array_to_slice_coercions =
            function_facts.node_pointer_array_to_slice_coercions;
        self.node_trait_object_coercions = function_facts.node_trait_object_coercions;
        self.node_trait_object_upcasts = function_facts.node_trait_object_upcasts;
        self.node_builtin_values = function_facts.node_builtin_values;
        self.node_associated_const_projections = function_facts.node_associated_const_projections;
        self.node_array_repeat_counts = function_facts.node_array_repeat_counts;
        self.node_pattern_values = function_facts.node_pattern_values;
        self.node_resolved_calls = function_facts.node_resolved_calls;
        self.node_function_references = function_facts.node_function_references;
        let lowered = self.profile_stage("body_check.profile.function.lower_body", |this| {
            this.lower_body(body)
        });
        self.function_bodies
            .insert(global_def_id, Arc::new(lowered));
        self.current_return = previous_return;
        self.current_def_id = previous_def_id;
        self.current_param_locals = previous_param_locals;
        self.local_types = previous_local_types;
        self.node_expr_types = previous_node_expr_types;
        self.node_bracket_suffix_resolutions = previous_node_bracket_suffix_resolutions;
        self.node_pointer_array_to_slice_coercions = previous_node_pointer_array_to_slice_coercions;
        self.node_trait_object_coercions = previous_node_trait_object_coercions;
        self.node_trait_object_upcasts = previous_node_trait_object_upcasts;
        self.node_builtin_values = previous_node_builtin_values;
        self.node_associated_const_projections = previous_node_associated_const_projections;
        self.node_array_repeat_counts = previous_node_array_repeat_counts;
        self.node_pattern_values = previous_node_pattern_values;
        self.node_resolved_calls = previous_node_resolved_calls;
        self.node_function_references = previous_node_function_references;
    }

    pub(super) fn check_module(
        &mut self,
        active_item_tree: &ActiveModuleItemTree,
        timing: bool,
        module_id: ModuleId,
    ) {
        if self.body_filter.includes_module_bindings() {
            time_body_stage(timing, "body_check.bindings", module_id, || {
                for item in &active_item_tree.items {
                    if let ItemTreeNodeKind::Binding(binding) = &item.kind {
                        if binding.is_const() {
                            self.check_const_binding(item.span, binding);
                        } else {
                            self.check_global_binding(item.span, binding);
                        }
                    }
                }
            });
        }
        time_body_stage(timing, "body_check.functions", module_id, || {
            let function_items =
                time_body_stage(timing, "body_check.function_index", module_id, || {
                    self.function_items_by_id(active_item_tree)
                });
            time_body_stage(timing, "body_check.function_check", module_id, || {
                self.check_reachable_functions(&function_items, timing, module_id);
            });
        });
        if self.body_filter.includes_module_bindings() {
            time_body_stage(timing, "body_check.extends", module_id, || {
                for item in &active_item_tree.items {
                    if let ItemTreeNodeKind::Extend(extend) = &item.kind
                        && extend.generics.is_empty()
                    {
                        for associated_value in &extend.associated_values {
                            if associated_value.binding.value.is_none() {
                                continue;
                            }
                            self.check_reachable_const_binding(
                                associated_value.span,
                                &associated_value.binding,
                            );
                        }
                    }
                }
            });
        }
    }

    pub(super) fn function_items_by_id<'ast>(
        &mut self,
        active_item_tree: &'ast ActiveModuleItemTree,
    ) -> HashMap<GlobalDefId, FunctionItemRef<'ast>> {
        let mut items = HashMap::new();
        for item in &active_item_tree.items {
            self.collect_function_items_by_id(item, &mut items);
        }
        items
    }

    pub(super) fn collect_function_items_by_id<'ast>(
        &mut self,
        item: &'ast ItemTreeNode,
        items: &mut HashMap<GlobalDefId, FunctionItemRef<'ast>>,
    ) {
        match &item.kind {
            ItemTreeNodeKind::Function(function) => {
                self.insert_function_item(item.span, DefKind::Function, function, items);
            }
            ItemTreeNodeKind::Trait(item_trait) => {
                for method in &item_trait.methods {
                    self.insert_function_item(
                        method.function.span,
                        DefKind::TraitMethod,
                        &method.function,
                        items,
                    );
                }
            }
            ItemTreeNodeKind::Extend(extend) => {
                if has_builtin_attribute(&item.attributes) {
                    return;
                }
                for method in &extend.methods {
                    self.insert_function_item(
                        method.function.span,
                        DefKind::Method,
                        &method.function,
                        items,
                    );
                }
            }
            ItemTreeNodeKind::Module(_)
            | ItemTreeNodeKind::Using(_)
            | ItemTreeNodeKind::Struct(_)
            | ItemTreeNodeKind::Union(_)
            | ItemTreeNodeKind::Enum(_)
            | ItemTreeNodeKind::Binding(_)
            | ItemTreeNodeKind::TypeAlias(_) => {}
        }
    }

    pub(super) fn insert_function_item<'ast>(
        &mut self,
        item_span: Span,
        kind: DefKind,
        function: &'ast FunctionItem,
        items: &mut HashMap<GlobalDefId, FunctionItemRef<'ast>>,
    ) {
        let Some(def_id) = self.def_id_for_node(&function.node_key, function.span, kind) else {
            return;
        };
        let global_def_id = self.global_def_id(def_id);
        items.insert(
            global_def_id,
            FunctionItemRef {
                item_span,
                kind,
                function,
            },
        );
    }

    pub(super) fn check_reachable_functions<'ast>(
        &mut self,
        function_items: &HashMap<GlobalDefId, FunctionItemRef<'ast>>,
        timing: bool,
        module_id: ModuleId,
    ) {
        let initial = self.body_filter.initial_functions(function_items);
        for def_id in initial {
            self.check_reachable_function_by_id(def_id, function_items, timing, module_id);
        }
        while let Some(def_id) = self.pending_functions.pop_front() {
            self.check_reachable_function_by_id(def_id, function_items, timing, module_id);
        }
    }

    pub(super) fn check_reachable_function_by_id<'ast>(
        &mut self,
        def_id: GlobalDefId,
        function_items: &HashMap<GlobalDefId, FunctionItemRef<'ast>>,
        timing: bool,
        module_id: ModuleId,
    ) {
        if !self.body_filter.includes_function(def_id) || !self.checked_functions.insert(def_id) {
            return;
        }
        let Some(item) = function_items.get(&def_id) else {
            return;
        };
        let stage = match item.kind {
            DefKind::Function => "body_check.function",
            DefKind::TraitMethod => "body_check.trait_method",
            DefKind::Method => "body_check.extend_method",
            _ => "body_check.function",
        };
        let threshold = if item.kind == DefKind::Method {
            0.010
        } else {
            0.050
        };
        time_body_stage_if_slow(
            timing,
            stage,
            module_id,
            mangle_symbol_id(item.function.name),
            threshold,
            || {
                self.check_function_with_kind(item.item_span, item.kind, item.function);
            },
        );
    }

    pub(super) fn seed_global_types(&mut self) {
        for (def_id, signature) in self.signatures.globals {
            if let Some(ty) = signature.explicit_type {
                self.global_types.insert(*def_id, ty);
            }
        }
        for (def_id, signature) in self.signatures.consts {
            if let Some(ty) = signature.explicit_type {
                self.const_types.insert(*def_id, ty);
            }
        }
    }

    pub(super) fn check_const_binding(&mut self, item_span: Span, binding: &nia_ast::BindingItem) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Const)
        else {
            return;
        };
        let Some(value) = &binding.value else {
            if self
                .signatures
                .consts
                .get(&def_id)
                .is_some_and(|signature| signature.builtin.is_some())
            {
                return;
            }
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                item_span,
                "const binding requires an initializer",
            ));
            return;
        };
        let const_ty = match binding.ty.as_ref() {
            Some(ty) => {
                let explicit = self.ty_for_type(ty);
                let value_ty = self
                    .const_initializer_runtime_type(value, Some(explicit))
                    .unwrap_or_else(|| {
                        self.with_const_context(|this| {
                            this.check_expr_with_expected(value, Some(explicit))
                        })
                    });
                if !self.is_const_only_ty(value_ty) && !self.types_match(explicit, value_ty) {
                    self.expect_expr_type(value, explicit, value_ty, "const initializer");
                }
                self.materialize_inferred_array_type(explicit, value_ty)
                    .unwrap_or(explicit)
            }
            None => {
                if let Some(ty) = self.const_initializer_runtime_type(value, None) {
                    ty
                } else if matches!(value.kind, ExprKind::ArrayLiteral { .. }) {
                    self.with_const_context(|this| this.infer_array_literal_expr(value))
                } else {
                    self.with_const_context(|this| this.check_expr(value))
                }
            }
        };
        self.const_types.insert(def_id, const_ty);
    }

    pub(super) fn check_reachable_const_binding(
        &mut self,
        item_span: Span,
        binding: &nia_ast::BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Const)
        else {
            return;
        };
        if !self.body_filter.includes_global(self.global_def_id(def_id)) {
            return;
        }
        self.check_const_binding(item_span, binding);
    }

    pub(super) fn const_initializer_runtime_type(
        &mut self,
        value: &Expr,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.with_const_context(|this| {
            let const_expr = this.lower_const_expr(value).ok()?;
            let ty = this.const_expr_type_for_ir_with_expected(&const_expr, expected)?;
            match ty {
                nia_const_check::ConstValueType::Runtime(ty) => Some(ty),
                _ => None,
            }
        })
    }

    pub(super) fn check_global_binding(&mut self, item_span: Span, binding: &nia_ast::BindingItem) {
        self.check_global_binding_inner(item_span, binding, true);
    }

    pub(super) fn check_global_binding_inner(
        &mut self,
        item_span: Span,
        binding: &nia_ast::BindingItem,
        filter_reachable_globals: bool,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Global)
        else {
            return;
        };
        if filter_reachable_globals && !self.body_filter.includes_global(self.global_def_id(def_id))
        {
            return;
        }
        let Some(value) = &binding.value else {
            let Some(signature) = self.signatures.globals.get(&def_id) else {
                return;
            };
            if let Some(ty) = signature.explicit_type {
                self.global_types.insert(def_id, ty);
            } else {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    item_span,
                    "global declaration requires an explicit type",
                ));
            }
            return;
        };
        let global_ty = match binding.ty.as_ref() {
            Some(ty) => {
                let explicit = self.ty_for_type(ty);
                let value_ty = self.check_expr_with_expected(value, Some(explicit));
                if self.is_const_only_ty(value_ty) {
                    self.reject_runtime_const_only_value(value.span, "global initializer");
                    self.error()
                } else {
                    self.expect_expr_type(value, explicit, value_ty, "global initializer");
                    self.materialize_inferred_array_type(explicit, value_ty)
                        .unwrap_or(explicit)
                }
            }
            None => {
                let value_ty = if matches!(value.kind, ExprKind::ArrayLiteral { .. }) {
                    self.infer_array_literal_expr(value)
                } else {
                    self.check_expr(value)
                };
                if self.is_const_only_ty(value_ty) {
                    self.reject_runtime_const_only_value(value.span, "global initializer");
                    self.error()
                } else {
                    value_ty
                }
            }
        };
        self.global_types.insert(def_id, global_ty);
        if global_ty != self.error() {
            let global_def_id = self.global_def_id(def_id);
            match self.product {
                BodyCheckProduct::FactsOnly => {
                    if let Some(init) = self.lower_global_static_init_checked(value, global_ty) {
                        self.static_init_refs
                            .insert(global_def_id, init.value_refs(self.defs.module_id));
                    }
                }
                BodyCheckProduct::Full => {
                    if let Some(init) = self.lower_global_static_init_checked(value, global_ty) {
                        self.global_inits.insert(global_def_id, Arc::new(init));
                    }
                }
                BodyCheckProduct::BodyOnly | BodyCheckProduct::StaticInitOnly => {}
            }
        }
    }

    pub(super) fn check_function_item(&mut self, _item_span: Span, function: &FunctionItem) {
        let Some(def_id) =
            self.def_id_for_node(&function.node_key, function.span, DefKind::Function)
        else {
            return;
        };
        if !self
            .body_filter
            .includes_function(self.global_def_id(def_id))
        {
            return;
        }
        self.check_function(def_id, function);
    }

    pub(super) fn check_function_with_kind(
        &mut self,
        item_span: Span,
        kind: DefKind,
        function: &FunctionItem,
    ) {
        match kind {
            DefKind::Function => self.check_function_item(item_span, function),
            DefKind::Method => self.check_function_def(item_span, function),
            DefKind::TraitMethod => self.check_trait_function_def(item_span, function),
            _ => {}
        }
    }

    pub(super) fn check_function_def(&mut self, _span: Span, function: &FunctionItem) {
        let Some(def_id) = self.def_id_for_node(&function.node_key, function.span, DefKind::Method)
        else {
            return;
        };
        if !self
            .body_filter
            .includes_function(self.global_def_id(def_id))
        {
            return;
        }
        self.check_function(def_id, function);
    }

    pub(super) fn check_trait_function_def(&mut self, _span: Span, function: &FunctionItem) {
        let Some(def_id) =
            self.def_id_for_node(&function.node_key, function.span, DefKind::TraitMethod)
        else {
            return;
        };
        if !self
            .body_filter
            .includes_function(self.global_def_id(def_id))
        {
            return;
        }
        self.check_function(def_id, function);
    }

    pub(super) fn check_function(&mut self, def_id: DefId, function: &FunctionItem) {
        let global_def_id = self.global_def_id(def_id);
        let diagnostic_start = self.diagnostics.len();
        self.check_function_inner(def_id, function);
        let diagnostic_end = self.diagnostics.len();
        if diagnostic_start != diagnostic_end {
            self.diagnostic_owners.resize(diagnostic_end, None);
            self.diagnostic_owners[diagnostic_start..diagnostic_end].fill(Some(global_def_id));
        }
    }

    pub(super) fn check_function_inner(&mut self, def_id: DefId, function: &FunctionItem) {
        let global_def_id = self.global_def_id(def_id);
        if !self
            .program_signature_scope
            .includes_function(global_def_id)
        {
            return;
        }
        let signature = self.profile_stage("body_check.profile.function.signature", |this| {
            this.function_signature_for_body(def_id, global_def_id)
        });
        let Some(signature) = signature else {
            return;
        };
        time_body_stage_if_slow(
            self.timing,
            "body_check.function.projection_obligations",
            self.timing_module_id,
            mangle_symbol_id(function.name),
            0.020,
            || {
                self.profile_stage(
                    "body_check.profile.function.projection_obligations",
                    |this| {
                        this.check_function_signature_projection_obligations(def_id, &signature);
                    },
                );
            },
        );
        let previous_return = self.current_return;
        let previous_def_id = self.current_def_id;
        let previous_param_locals = std::mem::take(&mut self.current_param_locals);
        let previous_local_types = std::mem::take(&mut self.local_types);
        self.current_return = signature.return_type;
        self.current_def_id = Some(global_def_id);
        let self_ty = self.method_self_type(def_id, &signature);
        time_body_stage_if_slow(
            self.timing,
            "body_check.function.object_safe",
            self.timing_module_id,
            mangle_symbol_id(function.name),
            0.020,
            || {
                self.profile_stage("body_check.profile.function.object_safe", |this| {
                    this.check_object_safe_types_in_signature(&signature);
                });
            },
        );
        time_body_stage_if_slow(
            self.timing,
            "body_check.function.seed_params",
            self.timing_module_id,
            mangle_symbol_id(function.name),
            0.020,
            || {
                self.profile_stage("body_check.profile.function.seed_params", |this| {
                    this.seed_param_types(&signature, function, self_ty);
                });
            },
        );
        if let Some(body) = &function.body {
            self.infer_function_closures(body);
            let expected_tail =
                (!self.is_unit(signature.return_type)).then_some(signature.return_type);
            time_body_stage_if_slow(
                self.timing,
                "body_check.function.check_block",
                self.timing_module_id,
                mangle_symbol_id(function.name),
                0.020,
                || {
                    self.profile_stage("body_check.profile.function.check_block", |this| {
                        let body_ty = this.check_block_with_expected(body, expected_tail);
                        if let Some(tail) = body.tail.as_deref() {
                            if !this.is_unit(signature.return_type) {
                                this.expect_expr_type(
                                    tail,
                                    signature.return_type,
                                    body_ty,
                                    "function body",
                                );
                            }
                        } else if this.is_unit(signature.return_type) {
                            this.expect_type(
                                body.span,
                                signature.return_type,
                                body_ty,
                                "function body",
                            );
                        }
                    });
                },
            );
        }
        self.current_return = previous_return;
        self.current_def_id = previous_def_id;
        self.current_param_locals = previous_param_locals;
        self.local_types = previous_local_types;
    }

    pub(super) fn function_signature_for_body(
        &mut self,
        def_id: DefId,
        global_def_id: GlobalDefId,
    ) -> Option<FunctionSignature> {
        if let Some(program_signature) = self.program_signature_scope.function(global_def_id) {
            Some(self.program_function_signature(&program_signature))
        } else {
            let raw_signature = self.signatures.functions.get(&def_id).cloned()?;
            Some(self.local_function_signature(&raw_signature))
        }
    }

    pub(super) fn check_object_safe_types_in_signature(&mut self, signature: &FunctionSignature) {
        for param in &signature.params {
            self.check_object_safe_type(param.span, param.ty);
        }
        self.check_object_safe_type(signature.span, signature.return_type);
    }

    pub(super) fn seed_param_types(
        &mut self,
        signature: &FunctionSignature,
        function: &FunctionItem,
        self_ty: Option<InternedTyId>,
    ) {
        for (param, param_sig) in function.params.iter().zip(&signature.params) {
            if let Some(local_id) = self.local_def(&param.node_key) {
                let ty = if param_sig.receiver.is_some() {
                    self_ty.unwrap_or_else(|| self.error())
                } else {
                    param_sig.ty
                };
                self.record_local_type(local_id, ty);
                self.current_param_locals.push(local_id);
            }
        }
    }
}

fn has_builtin_attribute(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        matches!(
            &attribute.kind,
            AttributeKind::Meta(meta) if meta.path == [known::BUILTIN]
        )
    })
}
