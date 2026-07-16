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
        let function_bodies = self
            .input
            .module
            .functions()
            .values()
            .map(|function| function.body().clone())
            .collect::<Vec<_>>();
        for body in &function_bodies {
            if let Some(ty) = self.find_local_binding_type_in_resolved_block(body, local_id) {
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
        self.call_locals
            .iter()
            .rev()
            .find_map(|frame| frame.locals.get(&local_id).cloned())
    }

    pub(super) fn def_kind_of(&self, global_id: GlobalDefId) -> Option<DefKind> {
        self.global_defs(global_id.module_id)?
            .as_ref()
            .defs
            .get(global_id.def_id)
            .map(|def| def.kind)
    }

    pub(super) fn resolved_const_function(
        &self,
        callee: &ResolvedConstExpr,
    ) -> Option<GlobalDefId> {
        if let Some(ConstNameResolution::Global(global_id)) = callee.name_resolution()
            && self.def_kind_of(global_id) == Some(DefKind::Function)
        {
            return Some(global_id);
        }
        None
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
        self.call_locals
            .iter()
            .rev()
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

    pub(super) fn source_interner_for_module(&self, module_id: ModuleId) -> Option<TyInterner> {
        if module_id == self.input.defs.module_id {
            Some(self.input.interner.clone())
        } else {
            Some(self.type_normalization_for_module(module_id)?.interner)
        }
    }

    pub(super) fn type_owner(&self, ty: nia_ids::InternedTyId) -> nia_ids::TypeOwner {
        self.input
            .interner
            .type_owner(ty)
            .expect("const type belongs to its session store")
    }

    fn source_interner_for_type(&self, ty: nia_ids::InternedTyId) -> Option<TyInterner> {
        if ty.interner_id == self.input.interner.interner_id()
            && self.input.interner.get(ty).is_some()
        {
            return Some(self.input.interner.clone());
        }
        let module_id = ty.interner_id.module_id();
        if let Some(normalization) = self.value_type_normalization_for_module(module_id)
            && normalization.interner.get(ty).is_some()
        {
            return Some(normalization.interner);
        }
        if let Some(interner) = self
            .type_normalization_for_module(module_id)
            .map(|normalization| normalization.interner)
            && interner.get(ty).is_some()
        {
            return Some(interner);
        }
        if ty.interner_id == self.input.interner.interner_id() {
            return Some(self.input.interner.clone());
        }
        None
    }

    fn working_interner_by_id(&self, interner_id: nia_ids::TyInternerId) -> Option<&TyInterner> {
        self.working_interners
            .values()
            .find(|interner| interner_id == interner.interner_id())
            .map(|interner| &**interner)
    }

    fn source_interner_snapshot_by_id(
        &self,
        interner_id: nia_ids::TyInternerId,
    ) -> Option<TyInterner> {
        if interner_id == self.input.interner.interner_id() {
            return Some(self.input.interner.clone());
        }
        let module_id = interner_id.module_id();
        if let Some(normalization) = self.value_type_normalization_for_module(module_id)
            && interner_id == normalization.interner.interner_id()
        {
            return Some(normalization.interner);
        }
        let interner = self.type_normalization_for_module(module_id)?.interner;
        (interner_id == interner.interner_id()).then_some(interner)
    }

    pub(super) fn active_interner_for_type(&self, ty: nia_ids::InternedTyId) -> TyInterner {
        let active = if let Some(working) = self.working_interner_by_id(ty.interner_id)
            && working.get(ty).is_some()
        {
            if let Some(source) = self.source_interner_snapshot_by_id(ty.interner_id) {
                if source.is_prefix_of(working) {
                    working.clone()
                } else if working.is_prefix_of(&source) {
                    source
                } else {
                    panic!(
                        "Nia ICE: const working type interner {:?} diverged from source snapshot",
                        ty.interner_id
                    );
                }
            } else {
                working.clone()
            }
        } else {
            self.source_interner_for_type(ty).unwrap_or_else(|| {
                panic!(
                    "Nia ICE: missing source type interner {:?} for const type {:?}",
                    ty.interner_id, ty
                )
            })
        };
        if active.get(ty).is_none() {
            panic!(
                "Nia ICE: const type {:?} is not present in active interner {:?}",
                ty,
                active.interner_id()
            );
        }
        active
    }

    pub(super) fn active_ty_kind(&self, ty: nia_ids::InternedTyId) -> TyKind {
        self.active_interner_for_type(ty)
            .get(ty)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "Nia ICE: const type {:?} is not present in active interner",
                    ty
                )
            })
    }

    pub(super) fn ensure_working_interner(&mut self, module_id: ModuleId) -> Option<()> {
        if self.working_interners.contains_key(&module_id) {
            return Some(());
        }
        let interner = self.source_interner_for_module(module_id)?;
        self.working_interners
            .insert(module_id, super::WorkingInterner::Snapshot(interner));
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
    ) -> Option<nia_type_normalize::TypeNormalization> {
        if module_id == self.input.defs.module_id {
            return None;
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
    }

    pub(super) fn value_type_normalization_for_module(
        &self,
        module_id: ModuleId,
    ) -> Option<nia_type_normalize::TypeNormalization> {
        if module_id == self.input.defs.module_id {
            return None;
        }
        if !self
            .program_value_type_normalizations
            .borrow()
            .contains_key(&module_id)
        {
            let normalizations = self
                .input
                .program
                .value_type_normalizations
                .or(self.input.program.type_normalizations)?;
            let normalization = normalizations(module_id)?;
            self.program_value_type_normalizations
                .borrow_mut()
                .insert(module_id, normalization);
        }
        self.program_value_type_normalizations
            .borrow()
            .get(&module_id)
            .cloned()
    }

    pub(super) fn normalized_for_module(
        &self,
        module_id: ModuleId,
    ) -> Option<HashMap<nia_ids::InternedTyId, nia_ids::InternedTyId>> {
        if module_id == self.input.defs.module_id {
            Some(self.input.normalized.clone())
        } else {
            Some(self.type_normalization_for_module(module_id)?.normalized)
        }
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
        if self.ensure_working_interner(module_id).is_none() {
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
        let Some(normalized) = self.normalized_for_module(module_id) else {
            return Err(ConstError {
                span,
                message: "cannot compute layout without normalized module types".to_string(),
            });
        };
        let Some(mut interner) = self.working_interners.remove(&module_id) else {
            return Err(ConstError {
                span,
                message: "cannot compute layout without module type interner".to_string(),
            });
        };
        let array_lengths = |id| layout_array_lengths.get(&id).copied();
        let layout_query =
            |module_id| self.compute_program_layout(module_id, &layout_array_lengths);
        let layouts = nia_layout::compute_layouts_with_program_context(
            defs.as_ref(),
            &mut interner,
            signatures.as_ref(),
            &normalized,
            &array_lengths,
            nia_layout::TargetDataLayout::LP64,
            nia_layout::ProgramLayoutContext {
                symbols: Some(self.input.symbols),
                layouts: Some(&layout_query),
                array_lengths: Some(&array_lengths),
                ..Default::default()
            },
        );
        self.working_interners.insert(module_id, interner);
        let ty = normalized.get(&ty).copied().unwrap_or(ty);
        let ty_module_id = self.type_owner(ty).module_id();
        if let Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) = self.ty_kind(ty)
            && (def_id.module_id != module_id || ty_module_id != module_id)
            && const_args.is_empty()
            && let Some(layouts) =
                self.compute_program_layout(def_id.module_id, &layout_array_lengths)
            && let Some(layout) = layouts.nominal_type_layout(def_id, &args)
        {
            return Ok(ConstValue::Int(IntConst::unsigned(
                layout.builtin_value(builtin) as u128,
            )));
        }
        if ty_module_id != module_id
            && let Some(layouts) = self.compute_program_layout(ty_module_id, &layout_array_lengths)
            && let Some(layout) = layouts.types.get(&ty)
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
        if self.ensure_working_interner(module_id).is_none() {
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
        let Some(normalized) = self.normalized_for_module(module_id) else {
            return Err(ConstError {
                span,
                message: "cannot compute field offset without normalized module types".to_string(),
            });
        };
        let Some(mut interner) = self.working_interners.remove(&module_id) else {
            return Err(ConstError {
                span,
                message: "cannot compute field offset without module type interner".to_string(),
            });
        };
        let array_lengths = |id| layout_array_lengths.get(&id).copied();
        let layout_query =
            |module_id| self.compute_program_layout(module_id, &layout_array_lengths);
        let layouts = nia_layout::compute_layouts_with_program_context(
            defs.as_ref(),
            &mut interner,
            signatures.as_ref(),
            &normalized,
            &array_lengths,
            nia_layout::TargetDataLayout::LP64,
            nia_layout::ProgramLayoutContext {
                symbols: Some(self.input.symbols),
                layouts: Some(&layout_query),
                array_lengths: Some(&array_lengths),
                ..Default::default()
            },
        );
        self.working_interners.insert(module_id, interner);
        let ty = normalized.get(&ty).copied().unwrap_or(ty);
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
        let ty_module_id = self.type_owner(ty).module_id();
        let offset = if def_id.module_id != module_id || ty_module_id != module_id {
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
    ) -> Option<nia_layout::Layouts> {
        let defs = self.global_defs(module_id)?;
        let signatures = self.signatures_for_module(module_id)?;
        let mut interner = self.source_interner_for_module(module_id)?;
        let normalized = self.normalized_for_module(module_id)?;
        let array_lengths_for_layout = |id: GlobalConstExprId| array_lengths.get(&id).copied();
        let layout_query = |module_id| self.compute_program_layout(module_id, array_lengths);
        Some(nia_layout::compute_layouts_with_program_context(
            defs.as_ref(),
            &mut interner,
            signatures.as_ref(),
            &normalized,
            &array_lengths_for_layout,
            nia_layout::TargetDataLayout::LP64,
            nia_layout::ProgramLayoutContext {
                symbols: Some(self.input.symbols),
                layouts: Some(&layout_query),
                array_lengths: Some(&array_lengths_for_layout),
                ..Default::default()
            },
        ))
    }
}
