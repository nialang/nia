use super::*;

pub(super) enum ModuleDefs<'a> {
    Borrowed(&'a DefCollection),
    Shared(std::sync::Arc<DefCollection>),
}

impl ModuleDefs<'_> {
    pub(super) fn as_ref(&self) -> &DefCollection {
        match self {
            ModuleDefs::Borrowed(defs) => defs,
            ModuleDefs::Shared(defs) => defs,
        }
    }
}

pub(super) enum ModuleSignatures<'a> {
    Borrowed(&'a ItemSignatures),
    Shared(std::sync::Arc<ItemSignatures>),
}

pub(super) enum ModuleNormalization<'a> {
    Borrowed(&'a nia_type_normalize::TypeNormalization),
    Shared(std::sync::Arc<nia_type_normalize::TypeNormalization>),
}

impl ModuleNormalization<'_> {
    pub(super) fn as_ref(&self) -> &nia_type_normalize::TypeNormalization {
        match self {
            ModuleNormalization::Borrowed(normalization) => normalization,
            ModuleNormalization::Shared(normalization) => normalization,
        }
    }
}

impl ModuleSignatures<'_> {
    pub(super) fn as_ref(&self) -> &ItemSignatures {
        match self {
            ModuleSignatures::Borrowed(signatures) => signatures,
            ModuleSignatures::Shared(signatures) => signatures,
        }
    }
}

impl Analyzer<'_> {
    pub(super) fn with_execution_module<T>(
        &mut self,
        module_id: ModuleId,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.execution_module_overrides.push(module_id);
        let result = f(self);
        self.execution_module_overrides.pop();
        result
    }

    pub(super) fn explicit_type_for_key(&mut self, key: ConstKey) -> Option<InternedTyId> {
        match key {
            ConstKey::Global(global_id) => {
                if global_id.module_id != self.input.defs.module_id
                    && let Some(signatures) = self.input.program.value_signatures
                    && let Some(signatures) = signatures(global_id.module_id)
                {
                    return signatures.consts.get(&global_id.def_id)?.explicit_type;
                }
                let signatures = self.signatures_for_module(global_id.module_id)?;
                signatures
                    .as_ref()
                    .consts
                    .get(&global_id.def_id)?
                    .explicit_type
            }
            ConstKey::Local(local_id) => self.find_local_binding_type(local_id),
        }
    }

    pub(super) fn find_local_binding_type(&mut self, local_id: LocalId) -> Option<InternedTyId> {
        // Local ids are allocated per module, while a const call can execute a
        // function from any module. Once a function frame is active, searching
        // unrelated module functions can re-enter type inference through a
        // different switch target (and can bind a payload local to the wrong
        // function). Restrict the fallback to the active function body.
        if let Some(function_id) = self.current_execution_function_id() {
            let body = self.const_function_body(function_id)?.body().clone();
            return self.find_local_binding_type_in_resolved_block(&body, local_id);
        }
        if let Some(initializer) = self.input.module.local_initializers().get(&local_id)
            && initializer.explicit_type().is_some()
        {
            return initializer.explicit_type();
        }
        let global_initializers = self
            .input
            .module
            .global_initializers()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for expr in &global_initializers {
            if let Some(ty) = self.find_local_binding_type_in_resolved_expr(expr, local_id) {
                return Some(ty);
            }
        }
        let local_initializers = self
            .input
            .module
            .local_initializers()
            .values()
            .map(|initializer| initializer.value().clone())
            .collect::<Vec<_>>();
        for expr in &local_initializers {
            if let Some(ty) = self.find_local_binding_type_in_resolved_expr(expr, local_id) {
                return Some(ty);
            }
        }
        None
    }

    pub(super) fn find_local_binding_type_in_resolved_block(
        &mut self,
        block: &ResolvedConstBlock,
        local_id: LocalId,
    ) -> Option<InternedTyId> {
        for stmt in block.stmts() {
            match stmt.kind() {
                ResolvedConstStmtKind::Binding(binding) if binding.local_id() == local_id => {
                    return binding.explicit_type();
                }
                ResolvedConstStmtKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    if let Some(ty) =
                        self.find_local_binding_type_in_resolved_block(then_branch, local_id)
                    {
                        return Some(ty);
                    }
                    if let Some(else_branch) = else_branch
                        && let Some(ty) =
                            self.find_local_binding_type_in_resolved_block(else_branch, local_id)
                    {
                        return Some(ty);
                    }
                }
                ResolvedConstStmtKind::ForIn(for_in) => {
                    if let Some(ty) =
                        self.find_local_binding_type_in_resolved_block(for_in.body(), local_id)
                    {
                        return Some(ty);
                    }
                }
                ResolvedConstStmtKind::While { body, .. }
                | ResolvedConstStmtKind::Loop { body } => {
                    if let Some(ty) = self.find_local_binding_type_in_resolved_block(body, local_id)
                    {
                        return Some(ty);
                    }
                }
                ResolvedConstStmtKind::Expr(expr) | ResolvedConstStmtKind::Return(Some(expr)) => {
                    if let Some(ty) = self.find_local_binding_type_in_resolved_expr(expr, local_id)
                    {
                        return Some(ty);
                    }
                }
                ResolvedConstStmtKind::Binding(_)
                | ResolvedConstStmtKind::Return(None)
                | ResolvedConstStmtKind::Break
                | ResolvedConstStmtKind::Continue => {}
            }
        }
        block
            .tail()
            .and_then(|tail| self.find_local_binding_type_in_resolved_expr(tail, local_id))
    }

    pub(super) fn find_local_binding_type_in_resolved_expr(
        &mut self,
        expr: &ResolvedConstExpr,
        local_id: LocalId,
    ) -> Option<InternedTyId> {
        match expr.kind() {
            ResolvedConstExprKind::If {
                then_branch,
                else_branch,
                ..
            } => self
                .find_local_binding_type_in_resolved_block(then_branch, local_id)
                .or_else(|| {
                    else_branch.as_deref().and_then(|else_branch| {
                        self.find_local_binding_type_in_resolved_expr(else_branch, local_id)
                    })
                }),
            ResolvedConstExprKind::Switch(switch) => {
                if let Some(ty) = self.find_resolved_pattern_local_type(switch, local_id) {
                    return Some(ty);
                }
                switch
                    .arms()
                    .iter()
                    .find_map(|arm| match arm.body().kind() {
                        ResolvedConstSwitchArmBodyKind::Expr(expr) => {
                            self.find_local_binding_type_in_resolved_expr(expr, local_id)
                        }
                        ResolvedConstSwitchArmBodyKind::Stmt(stmt) => match stmt.kind() {
                            ResolvedConstStmtKind::Binding(binding)
                                if binding.local_id() == local_id =>
                            {
                                binding.explicit_type()
                            }
                            ResolvedConstStmtKind::Expr(expr)
                            | ResolvedConstStmtKind::Return(Some(expr)) => {
                                self.find_local_binding_type_in_resolved_expr(expr, local_id)
                            }
                            _ => None,
                        },
                        ResolvedConstSwitchArmBodyKind::Block(block) => {
                            self.find_local_binding_type_in_resolved_block(block, local_id)
                        }
                    })
            }
            ResolvedConstExprKind::Block(block) => {
                self.find_local_binding_type_in_resolved_block(block, local_id)
            }
            _ => None,
        }
    }

    pub(super) fn push_engine_error(&mut self, err: ConstError) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::CONST,
            err.span,
            err.message,
        ));
    }

    pub(super) fn initializer_for_key(&self, key: ConstKey) -> Option<ResolvedConstExpr> {
        match key {
            ConstKey::Global(global_id) => self.global_initializer(global_id),
            ConstKey::Local(local_id) => self.local_initializer(local_id).cloned(),
        }
    }

    pub(super) fn global_initializer(&self, global_id: GlobalDefId) -> Option<ResolvedConstExpr> {
        if global_id.module_id == self.input.defs.module_id {
            self.input
                .module
                .global_initializers()
                .get(&global_id)
                .or_else(|| {
                    self.input
                        .module
                        .deferred_global_initializers()
                        .get(&global_id)
                })
                .cloned()
        } else if let Some(global_initializer) = self.input.program.global_initializer {
            if !self
                .program_global_initializers
                .borrow()
                .contains_key(&global_id)
            {
                let initializer = global_initializer(global_id);
                self.program_global_initializers
                    .borrow_mut()
                    .insert(global_id, initializer);
            }
            self.program_global_initializers
                .borrow()
                .get(&global_id)
                .cloned()
                .flatten()
        } else {
            let module = (self.input.program.module?)(global_id.module_id)?;
            module
                .global_initializers()
                .get(&global_id)
                .or_else(|| module.deferred_global_initializers().get(&global_id))
                .cloned()
        }
    }

    pub(super) fn global_defs(&self, module_id: ModuleId) -> Option<ModuleDefs<'_>> {
        if module_id == self.input.defs.module_id {
            Some(ModuleDefs::Borrowed(self.input.defs))
        } else {
            Some(ModuleDefs::Shared((self.input.program.defs?)(module_id)?))
        }
    }

    pub(super) fn local_initializer(&self, local_id: LocalId) -> Option<&ResolvedConstExpr> {
        self.input
            .module
            .local_initializers()
            .get(&local_id)
            .map(|initializer| initializer.value())
    }

    pub(super) fn call_local_value(&self, local_id: LocalId) -> Option<ConstValue> {
        self.active_execution_frames()
            .find_map(|frame| frame.locals.get(&local_id).cloned())
    }

    pub(super) fn active_execution_frames(&self) -> impl Iterator<Item = &ConstCallFrame> {
        self.call_locals
            .iter()
            .rev()
            .scan(true, |inside_execution, frame| {
                if !*inside_execution {
                    return None;
                }
                if frame.module_id.is_some() {
                    *inside_execution = false;
                }
                Some(frame)
            })
    }

    pub(super) fn def_kind_of(&self, global_id: GlobalDefId) -> Option<DefKind> {
        self.global_defs(global_id.module_id)?
            .as_ref()
            .defs
            .get(global_id.def_id)
            .map(|def| def.kind)
    }

    pub(super) fn resolved_const_callee(
        &mut self,
        callee: &ResolvedConstExpr,
    ) -> Option<ResolvedConstCallee> {
        if let Some(ConstNameResolution::Global(global_id)) = callee.name_resolution()
            && self.def_kind_of(global_id) == Some(DefKind::Function)
        {
            return Some(ResolvedConstCallee {
                function_id: global_id,
                receiver: None,
                target_instantiation: ConstGenericInstantiation::default(),
            });
        }
        let (target_ty, name, receiver) = match callee.kind() {
            ResolvedConstExprKind::Method { receiver, name } => (
                self.resolved_const_arg_runtime_type(receiver, None)?,
                *name,
                Some(receiver.as_ref().clone()),
            ),
            ResolvedConstExprKind::AssociatedFunction { target, name } => {
                let module_id = self.current_execution_module_id();
                let target_ty = match target {
                    ResolvedConstAssociatedTarget::Type(target) => {
                        let target_ty = self.substitute_ty_generics(target.ty());
                        self.type_for_module_or_none(target_ty, module_id)?
                    }
                    ResolvedConstAssociatedTarget::Nominal { def_id, args } => {
                        self.ensure_type_context(module_id)?;
                        let args = args
                            .iter()
                            .map(|arg| self.type_for_module_or_none(arg.ty(), module_id))
                            .collect::<Option<Vec<_>>>()?;
                        self.type_contexts.get(&module_id)?.intern(TyKind::Nominal {
                            def_id: *def_id,
                            args,
                            const_args: Vec::new(),
                        })
                    }
                };
                (target_ty, *name, None)
            }
            _ => return None,
        };
        let visible_extensions =
            (self.input.program.visible_extensions?)(self.current_execution_module_id())?;
        let actual_target_ty = self
            .type_normalization_for_module(self.current_execution_module_id())?
            .as_ref()
            .normalize(target_ty);
        let mut candidates = visible_extensions
            .all_methods_named(&name)
            .into_iter()
            .filter_map(|(candidate_target_ty, method)| {
                let function_id = method.def_id;
                self.signatures_for_module(function_id.module_id)
                    .and_then(|signatures| {
                        signatures
                            .as_ref()
                            .functions
                            .get(&function_id.def_id)
                            .cloned()
                    })
                    .filter(|signature| {
                        signature.is_const
                            && signature
                                .params
                                .first()
                                .is_some_and(|param| param.receiver.is_some())
                                == receiver.is_some()
                    })?;
                let mut target_instantiation = ConstGenericInstantiation::default();
                self.infer_generics_from_tys(
                    callee.span(),
                    function_id.module_id,
                    candidate_target_ty,
                    actual_target_ty,
                    &mut target_instantiation.type_substitutions,
                )
                .ok()?;
                let substituted_target = self.const_expected_param_type(
                    function_id.module_id,
                    candidate_target_ty,
                    &target_instantiation.type_substitutions,
                )?;
                (substituted_target == actual_target_ty).then_some((
                    candidate_target_ty == actual_target_ty,
                    ResolvedConstCallee {
                        function_id,
                        receiver: receiver.clone(),
                        target_instantiation,
                    },
                ))
            })
            .collect::<Vec<_>>();
        if candidates.len() > 1 {
            let exact = candidates.iter().filter(|(is_exact, _)| *is_exact).count();
            if exact == 1 {
                candidates.retain(|(is_exact, _)| *is_exact);
            }
        }
        (candidates.len() == 1).then(|| candidates.remove(0).1)
    }

    pub(super) fn resolved_const_enum_variant(
        &self,
        expr: &ResolvedConstExpr,
    ) -> Option<(GlobalDefId, nia_item_signatures::EnumVariantSignature)> {
        let ConstNameResolution::Global(variant_id) = expr.name_resolution()? else {
            return None;
        };
        if self.def_kind_of(variant_id) != Some(DefKind::EnumVariant) {
            return None;
        }
        let enum_def = self
            .global_defs(variant_id.module_id)?
            .as_ref()
            .defs
            .get(variant_id.def_id)?
            .parent?;
        let enum_id = GlobalDefId {
            module_id: variant_id.module_id,
            def_id: enum_def,
        };
        let variant = self
            .signatures_for_module(enum_id.module_id)?
            .as_ref()
            .enums
            .get(&enum_id.def_id)?
            .variants
            .iter()
            .find(|variant| variant.def_id == variant_id.def_id)?
            .clone();
        Some((enum_id, variant))
    }

    pub(super) fn enum_ty_in_current_module(&self, enum_id: GlobalDefId) -> InternedTyId {
        self.input
            .type_store
            .append_for_module(self.current_execution_module_id())
            .intern(TyKind::Nominal {
                def_id: enum_id,
                args: Vec::new(),
                const_args: Vec::new(),
            })
    }

    pub(super) fn const_function_body(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_const_ir::ResolvedConstFunction> {
        if def_id.module_id == self.input.defs.module_id {
            self.input.module.functions().get(&def_id).cloned()
        } else {
            (self.input.program.module?)(def_id.module_id)?
                .functions()
                .get(&def_id)
                .cloned()
        }
    }

    pub(super) fn current_execution_module_id(&self) -> ModuleId {
        if let Some(module_id) = self.execution_module_overrides.last().copied() {
            return module_id;
        }
        self.call_locals
            .iter()
            .rev()
            .find_map(|frame| frame.module_id)
            .unwrap_or(self.input.defs.module_id)
    }

    pub(super) fn current_execution_function_id(&self) -> Option<GlobalDefId> {
        self.active_execution_frames()
            .find_map(|frame| frame.function_id)
    }

    pub(super) fn current_execution_source_path(&self) -> Option<nia_source::SourcePath> {
        let module_id = self.current_execution_module_id();
        if module_id == self.input.defs.module_id {
            Some(self.input.source_path.clone())
        } else {
            (self.input.program.source_path?)(module_id)
        }
    }

    pub(super) fn primitive_ty_for_module(
        &self,
        module_id: ModuleId,
        primitive: nia_ty::PrimitiveTy,
    ) -> nia_ids::InternedTyId {
        self.input
            .type_store
            .append_for_module(module_id)
            .intern(TyKind::Primitive(primitive))
    }

    pub(super) fn active_ty_kind(&self, ty: nia_ids::InternedTyId) -> TyKind {
        self.input.type_store.get(ty).cloned().unwrap_or_else(|| {
            panic!(
                "Nia ICE: const type {:?} is not present in the session type store",
                ty
            )
        })
    }

    pub(super) fn ensure_type_context(&mut self, module_id: ModuleId) -> Option<()> {
        if self.type_contexts.contains_key(&module_id) {
            return Some(());
        }
        self.type_contexts.insert(
            module_id,
            super::ConstTypeCx::new(self.input.type_store, module_id),
        );
        Some(())
    }

    pub(super) fn signatures_for_module(
        &self,
        module_id: ModuleId,
    ) -> Option<ModuleSignatures<'_>> {
        if module_id == self.input.defs.module_id {
            Some(ModuleSignatures::Borrowed(self.input.signatures))
        } else {
            (self.input.program.signatures?)(module_id).map(ModuleSignatures::Shared)
        }
    }

    pub(super) fn type_normalization_for_module(
        &self,
        module_id: ModuleId,
    ) -> Option<ModuleNormalization<'_>> {
        if module_id == self.input.defs.module_id {
            return Some(ModuleNormalization::Borrowed(self.input.normalization));
        }
        if !self
            .program_type_normalizations
            .borrow()
            .contains_key(&module_id)
        {
            let normalization = (self.input.program.type_normalizations?)(module_id)?;
            self.program_type_normalizations
                .borrow_mut()
                .insert(module_id, normalization);
        }
        self.program_type_normalizations
            .borrow()
            .get(&module_id)
            .cloned()
            .map(ModuleNormalization::Shared)
    }

    pub(super) fn trait_impls_for_solver_module(
        &self,
        module_id: ModuleId,
    ) -> Vec<nia_item_signatures::ProgramTraitImplSignature> {
        let Some(trait_impls_for_module) = self.input.program.trait_impls_for_module else {
            return Vec::new();
        };
        if !self.program_trait_impls.borrow().contains_key(&module_id) {
            let trait_impls = trait_impls_for_module(module_id).unwrap_or_default();
            self.program_trait_impls
                .borrow_mut()
                .insert(module_id, trait_impls);
        }
        self.program_trait_impls
            .borrow()
            .get(&module_id)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn resolve_layout_builtin_for_ty(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        ty: nia_ids::InternedTyId,
    ) -> Result<ConstValue, ConstError> {
        let module_id = self.current_execution_module_id();
        let layout_array_lengths = self.program_array_lengths_for_layout(ty);
        if self.ensure_type_context(module_id).is_none() {
            return Err(ConstError {
                span,
                message: "cannot compute layout without module type interner".to_string(),
            });
        }
        let defs = if module_id == self.input.defs.module_id {
            ModuleDefs::Borrowed(self.input.defs)
        } else if let Some(defs) = self.input.program.defs.and_then(|defs| defs(module_id)) {
            ModuleDefs::Shared(defs)
        } else {
            return Err(ConstError {
                span,
                message: "cannot compute layout without module definitions".to_string(),
            });
        };
        let signatures = if module_id == self.input.defs.module_id {
            ModuleSignatures::Borrowed(self.input.signatures)
        } else if let Some(signatures) = self
            .input
            .program
            .signatures
            .and_then(|signatures| signatures(module_id))
        {
            ModuleSignatures::Shared(signatures)
        } else {
            return Err(ConstError {
                span,
                message: "cannot compute layout without module signatures".to_string(),
            });
        };
        let Some(normalization) = self.type_normalization_for_module(module_id) else {
            return Err(ConstError {
                span,
                message: "cannot compute layout without normalized module types".to_string(),
            });
        };
        let mut root_types = signatures.as_ref().type_roots();
        root_types.push(ty);
        let array_lengths = |id| layout_array_lengths.get(&id).copied();
        let layout_query =
            |module_id| self.compute_program_layout(module_id, &layout_array_lengths);
        let layouts =
            nia_layout::compute_layouts_with_program_context(nia_layout::LayoutComputationInput {
                type_store: self.input.type_store,
                defs: defs.as_ref(),
                signatures: signatures.as_ref(),
                root_types: &root_types,
                normalized: &normalization.as_ref().normalized,
                array_lengths: &array_lengths,
                target: nia_layout::TargetDataLayout::LP64,
                program: nia_layout::ProgramLayoutContext {
                    symbols: Some(self.input.symbols),
                    layouts: Some(&layout_query),
                    array_lengths: Some(&array_lengths),
                    ..Default::default()
                },
            });
        let ty = normalization
            .as_ref()
            .normalized
            .get(&ty)
            .copied()
            .unwrap_or(ty);
        if let Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) = self.ty_kind(ty)
            && def_id.module_id != module_id
            && const_args.is_empty()
            && let Some(layouts) =
                self.compute_program_layout(def_id.module_id, &layout_array_lengths)
            && let Some(layout) = layouts.nominal_type_layout(def_id, &args)
        {
            return Ok(ConstValue::Int(IntConst::unsigned(
                layout.builtin_value(builtin) as u128,
            )));
        }
        let Some(layout) = layouts.types.get(&ty) else {
            return Err(ConstError {
                span,
                message: format!(
                    "cannot compute layout for const builtin `@{}`",
                    builtin.name()
                ),
            });
        };
        Ok(ConstValue::Int(IntConst::unsigned(
            layout.builtin_value(builtin) as u128,
        )))
    }

    pub(super) fn resolve_field_offset_builtin_for_ty(
        &mut self,
        span: Span,
        ty: nia_ids::InternedTyId,
        field: &SymbolId,
    ) -> Result<ConstValue, ConstError> {
        let module_id = self.current_execution_module_id();
        let layout_array_lengths = self.program_array_lengths_for_layout(ty);
        if self.ensure_type_context(module_id).is_none() {
            return Err(ConstError {
                span,
                message: "cannot compute field offset without module type interner".to_string(),
            });
        }
        let defs = if module_id == self.input.defs.module_id {
            ModuleDefs::Borrowed(self.input.defs)
        } else if let Some(defs) = self.input.program.defs.and_then(|defs| defs(module_id)) {
            ModuleDefs::Shared(defs)
        } else {
            return Err(ConstError {
                span,
                message: "cannot compute field offset without module definitions".to_string(),
            });
        };
        let signatures = if module_id == self.input.defs.module_id {
            ModuleSignatures::Borrowed(self.input.signatures)
        } else if let Some(signatures) = self
            .input
            .program
            .signatures
            .and_then(|signatures| signatures(module_id))
        {
            ModuleSignatures::Shared(signatures)
        } else {
            return Err(ConstError {
                span,
                message: "cannot compute field offset without module signatures".to_string(),
            });
        };
        let Some(normalization) = self.type_normalization_for_module(module_id) else {
            return Err(ConstError {
                span,
                message: "cannot compute field offset without normalized module types".to_string(),
            });
        };
        let mut root_types = signatures.as_ref().type_roots();
        root_types.push(ty);
        let array_lengths = |id| layout_array_lengths.get(&id).copied();
        let layout_query =
            |module_id| self.compute_program_layout(module_id, &layout_array_lengths);
        let layouts =
            nia_layout::compute_layouts_with_program_context(nia_layout::LayoutComputationInput {
                type_store: self.input.type_store,
                defs: defs.as_ref(),
                signatures: signatures.as_ref(),
                root_types: &root_types,
                normalized: &normalization.as_ref().normalized,
                array_lengths: &array_lengths,
                target: nia_layout::TargetDataLayout::LP64,
                program: nia_layout::ProgramLayoutContext {
                    symbols: Some(self.input.symbols),
                    layouts: Some(&layout_query),
                    array_lengths: Some(&array_lengths),
                    ..Default::default()
                },
            });
        let ty = normalization
            .as_ref()
            .normalized
            .get(&ty)
            .copied()
            .unwrap_or(ty);
        let Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) = self.ty_kind(ty)
        else {
            return Err(ConstError {
                span,
                message: "builtin `offset` requires a struct or union type argument".to_string(),
            });
        };
        if !const_args.is_empty() {
            return Err(ConstError {
                span,
                message: "builtin `offset` does not support const generic nominal types yet"
                    .to_string(),
            });
        }
        let Some(field_def) = self.field_def_for_nominal(def_id, field) else {
            let field = self.symbol_name(*field);
            return Err(ConstError {
                span,
                message: format!("type has no field `{field}` for builtin `offset`"),
            });
        };
        let offset = if def_id.module_id != module_id {
            self.compute_program_layout(def_id.module_id, &layout_array_lengths)
                .and_then(|layouts| layouts.field_offset(def_id, &args, field_def))
        } else {
            layouts.field_offset(def_id, &args, field_def)
        };
        let Some(offset) = offset else {
            return Err(ConstError {
                span,
                message: "cannot compute field offset for const builtin `offset`".to_string(),
            });
        };
        Ok(ConstValue::Int(IntConst::unsigned(offset as u128)))
    }

    fn field_def_for_nominal(&self, def_id: GlobalDefId, name: &SymbolId) -> Option<GlobalDefId> {
        let defs = self.global_defs(def_id.module_id)?;
        let defs = defs.as_ref();
        defs.scopes
            .struct_members
            .get(&def_id.def_id)
            .and_then(|members| members.fields.get(name))
            .or_else(|| {
                defs.scopes
                    .union_members
                    .get(&def_id.def_id)
                    .and_then(|members| members.fields.get(name))
            })
            .map(|field| GlobalDefId {
                module_id: def_id.module_id,
                def_id: field,
            })
    }

    pub(super) fn program_array_lengths_for_layout(
        &mut self,
        ty: InternedTyId,
    ) -> HashMap<GlobalConstExprId, u64> {
        let mut array_lengths = self.array_lengths.clone();
        let mut needed = HashSet::new();
        self.collect_array_len_const_exprs_in_ty(ty, &mut needed);
        for id in needed {
            if array_lengths.contains_key(&id) {
                continue;
            }
            if let Some(value) = self.eval_array_len_const_expr_id(id) {
                array_lengths.insert(id, value);
            }
        }
        array_lengths
    }

    pub(super) fn collect_array_len_const_exprs_in_ty(
        &self,
        ty: InternedTyId,
        out: &mut HashSet<GlobalConstExprId>,
    ) {
        self.collect_array_len_const_exprs_in_ty_inner(ty, out, &mut HashSet::new());
    }

    pub(super) fn collect_array_len_const_exprs_in_ty_inner(
        &self,
        ty: InternedTyId,
        out: &mut HashSet<GlobalConstExprId>,
        seen: &mut HashSet<InternedTyId>,
    ) {
        if !seen.insert(ty) {
            return;
        }
        match self.ty_kind(ty) {
            Some(TyKind::Array { len, elem }) => {
                if let ArrayLenTy::ConstExpr(id) = len {
                    out.insert(id);
                }
                self.collect_array_len_const_exprs_in_ty_inner(elem, out, seen);
            }
            Some(TyKind::Optional { elem })
            | Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem }) => {
                self.collect_array_len_const_exprs_in_ty_inner(elem, out, seen);
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.collect_array_len_const_exprs_in_ty_inner(error, out, seen);
                self.collect_array_len_const_exprs_in_ty_inner(value, out, seen);
            }
            Some(TyKind::Range {
                bound: Some(bound), ..
            }) => {
                self.collect_array_len_const_exprs_in_ty_inner(bound, out, seen);
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.collect_array_len_const_exprs_in_ty_inner(param, out, seen);
                }
                self.collect_array_len_const_exprs_in_ty_inner(return_type, out, seen);
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                for arg in args {
                    self.collect_array_len_const_exprs_in_ty_inner(arg, out, seen);
                }
                for arg in const_args {
                    self.collect_array_len_const_exprs_in_const_arg(&arg, out, seen);
                }
                let Some(signatures) = self.signatures_for_module(def_id.module_id) else {
                    return;
                };
                if let Some(signature) = signatures.as_ref().structs.get(&def_id.def_id) {
                    for field in &signature.fields {
                        self.collect_array_len_const_exprs_in_ty_inner(field.ty, out, seen);
                    }
                }
                if let Some(signature) = signatures.as_ref().unions.get(&def_id.def_id) {
                    for field in &signature.fields {
                        self.collect_array_len_const_exprs_in_ty_inner(field.ty, out, seen);
                    }
                }
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.collect_array_len_const_exprs_in_ty_inner(arg, out, seen);
                }
            }
            Some(TyKind::TraitObject {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in trait_args {
                    self.collect_array_len_const_exprs_in_ty_inner(arg, out, seen);
                }
                for arg in trait_const_args {
                    self.collect_array_len_const_exprs_in_const_arg(&arg, out, seen);
                }
                for binding in associated_type_bindings {
                    for arg in &binding.trait_args {
                        self.collect_array_len_const_exprs_in_ty_inner(*arg, out, seen);
                    }
                    for arg in &binding.trait_const_args {
                        self.collect_array_len_const_exprs_in_const_arg(arg, out, seen);
                    }
                    self.collect_array_len_const_exprs_in_ty_inner(binding.ty, out, seen);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            }) => {
                self.collect_array_len_const_exprs_in_ty_inner(self_ty, out, seen);
                for arg in trait_args {
                    self.collect_array_len_const_exprs_in_ty_inner(arg, out, seen);
                }
                for arg in trait_const_args {
                    self.collect_array_len_const_exprs_in_const_arg(&arg, out, seen);
                }
            }
            Some(
                TyKind::Range { bound: None, .. }
                | TyKind::Error
                | TyKind::ConstOnly
                | TyKind::GenericParam(_)
                | TyKind::SelfParam
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::Vector { .. },
            )
            | None => {}
        }
    }

    fn collect_array_len_const_exprs_in_const_arg(
        &self,
        arg: &nia_ty::ConstGenericArg,
        out: &mut HashSet<GlobalConstExprId>,
        seen: &mut HashSet<InternedTyId>,
    ) {
        if self.const_generic_arg_type_is_array_len(arg.ty)
            && let nia_ty::ConstGenericValue::ConstExpr(id) = &arg.value
        {
            out.insert(*id);
        }
        self.collect_array_len_const_exprs_in_ty_inner(arg.ty, out, seen);
    }

    fn const_generic_arg_type_is_array_len(&self, ty: InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::Usize))
        )
    }

    pub(super) fn compute_program_layout(
        &self,
        module_id: ModuleId,
        array_lengths: &HashMap<GlobalConstExprId, u64>,
    ) -> Option<Arc<nia_layout::Layouts>> {
        let defs = self.global_defs(module_id)?;
        let signatures = self.signatures_for_module(module_id)?;
        let root_types = signatures.as_ref().type_roots();
        let normalization = self.type_normalization_for_module(module_id)?;
        let array_lengths_for_layout = |id: GlobalConstExprId| array_lengths.get(&id).copied();
        let layout_query = |module_id| self.compute_program_layout(module_id, array_lengths);
        Some(Arc::new(nia_layout::compute_layouts_with_program_context(
            nia_layout::LayoutComputationInput {
                type_store: self.input.type_store,
                defs: defs.as_ref(),
                signatures: signatures.as_ref(),
                root_types: &root_types,
                normalized: &normalization.as_ref().normalized,
                array_lengths: &array_lengths_for_layout,
                target: nia_layout::TargetDataLayout::LP64,
                program: nia_layout::ProgramLayoutContext {
                    symbols: Some(self.input.symbols),
                    layouts: Some(&layout_query),
                    array_lengths: Some(&array_lengths_for_layout),
                    ..Default::default()
                },
            },
        )))
    }
}
