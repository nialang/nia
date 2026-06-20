use super::*;

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

    pub(super) fn explicit_type_for_key(&mut self, key: ComptimeKey) -> Option<InternedTyId> {
        match key {
            ComptimeKey::Global(global_id) => {
                let signatures = self.signatures_for_module(global_id.module_id)?;
                signatures.comptimes.get(&global_id.def_id)?.explicit_type
            }
            ComptimeKey::Local(local_id) => self.find_local_binding_type(local_id),
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
        block: &ResolvedComptimeBlock,
        local_id: LocalId,
    ) -> Option<InternedTyId> {
        for stmt in block.stmts() {
            match stmt.kind() {
                ResolvedComptimeStmtKind::Binding(binding) if binding.local_id() == local_id => {
                    return binding.explicit_type();
                }
                ResolvedComptimeStmtKind::If {
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
                ResolvedComptimeStmtKind::ForIn(for_in) => {
                    if let Some(ty) =
                        self.find_local_binding_type_in_resolved_block(for_in.body(), local_id)
                    {
                        return Some(ty);
                    }
                }
                ResolvedComptimeStmtKind::While { body, .. }
                | ResolvedComptimeStmtKind::Loop { body } => {
                    if let Some(ty) = self.find_local_binding_type_in_resolved_block(body, local_id)
                    {
                        return Some(ty);
                    }
                }
                ResolvedComptimeStmtKind::Expr(expr)
                | ResolvedComptimeStmtKind::Return(Some(expr)) => {
                    if let Some(ty) = self.find_local_binding_type_in_resolved_expr(expr, local_id)
                    {
                        return Some(ty);
                    }
                }
                ResolvedComptimeStmtKind::Binding(_)
                | ResolvedComptimeStmtKind::Return(None)
                | ResolvedComptimeStmtKind::Break
                | ResolvedComptimeStmtKind::Continue => {}
            }
        }
        block
            .tail()
            .and_then(|tail| self.find_local_binding_type_in_resolved_expr(tail, local_id))
    }

    pub(super) fn find_local_binding_type_in_resolved_expr(
        &mut self,
        expr: &ResolvedComptimeExpr,
        local_id: LocalId,
    ) -> Option<InternedTyId> {
        match expr.kind() {
            ResolvedComptimeExprKind::If {
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
            ResolvedComptimeExprKind::Switch(switch) => {
                if let Some(ty) = self.find_resolved_pattern_local_type(switch, local_id) {
                    return Some(ty);
                }
                switch
                    .arms()
                    .iter()
                    .find_map(|arm| match arm.body().kind() {
                        ResolvedComptimeSwitchArmBodyKind::Expr(expr) => {
                            self.find_local_binding_type_in_resolved_expr(expr, local_id)
                        }
                        ResolvedComptimeSwitchArmBodyKind::Stmt(stmt) => match stmt.kind() {
                            ResolvedComptimeStmtKind::Binding(binding)
                                if binding.local_id() == local_id =>
                            {
                                binding.explicit_type()
                            }
                            ResolvedComptimeStmtKind::Expr(expr)
                            | ResolvedComptimeStmtKind::Return(Some(expr)) => {
                                self.find_local_binding_type_in_resolved_expr(expr, local_id)
                            }
                            _ => None,
                        },
                        ResolvedComptimeSwitchArmBodyKind::Block(block) => {
                            self.find_local_binding_type_in_resolved_block(block, local_id)
                        }
                    })
            }
            ResolvedComptimeExprKind::Block(block) => {
                self.find_local_binding_type_in_resolved_block(block, local_id)
            }
            _ => None,
        }
    }

    pub(super) fn push_engine_error(&mut self, err: ComptimeError) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::COMPTIME,
            err.span,
            err.message,
        ));
    }

    pub(super) fn initializer_for_key(&self, key: ComptimeKey) -> Option<&ResolvedComptimeExpr> {
        match key {
            ComptimeKey::Global(global_id) => self.global_initializer(global_id),
            ComptimeKey::Local(local_id) => self.local_initializer(local_id),
        }
    }

    pub(super) fn global_initializer(
        &self,
        global_id: GlobalDefId,
    ) -> Option<&ResolvedComptimeExpr> {
        if global_id.module_id == self.input.defs.module_id {
            self.input.module.global_initializers().get(&global_id)
        } else {
            self.input
                .program
                .modules?
                .get(&global_id.module_id)?
                .global_initializers()
                .get(&global_id)
        }
    }

    pub(super) fn global_defs(&self, module_id: ModuleId) -> Option<&DefCollection> {
        if module_id == self.input.defs.module_id {
            Some(self.input.defs)
        } else {
            self.input.program.defs?.get(&module_id)
        }
    }

    pub(super) fn local_initializer(&self, local_id: LocalId) -> Option<&ResolvedComptimeExpr> {
        self.input
            .module
            .local_initializers()
            .get(&local_id)
            .map(|initializer| initializer.value())
    }

    pub(super) fn call_local_value(&self, local_id: LocalId) -> Option<ComptimeValue> {
        self.call_locals
            .iter()
            .rev()
            .find_map(|frame| frame.locals.get(&local_id).cloned())
    }

    pub(super) fn def_kind_of(&self, global_id: GlobalDefId) -> Option<DefKind> {
        self.global_defs(global_id.module_id)?
            .defs
            .get(global_id.def_id)
            .map(|def| def.kind)
    }

    pub(super) fn resolved_comptime_function(
        &self,
        callee: &ResolvedComptimeExpr,
    ) -> Option<GlobalDefId> {
        if let Some(ComptimeNameResolution::Global(global_id)) = callee.name_resolution()
            && self.def_kind_of(global_id) == Some(DefKind::Function)
        {
            return Some(global_id);
        }
        None
    }

    pub(super) fn comptime_function_body(
        &self,
        def_id: GlobalDefId,
    ) -> Option<&nia_comptime_ir::ResolvedComptimeFunction> {
        if def_id.module_id == self.input.defs.module_id {
            self.input.module.functions().get(&def_id)
        } else {
            self.input
                .program
                .modules?
                .get(&def_id.module_id)?
                .functions()
                .get(&def_id)
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

    pub(super) fn interner_for_module(&self, module_id: ModuleId) -> Option<&TyInterner> {
        self.working_interners.get(&module_id)
    }

    pub(super) fn source_interner_for_module(&self, module_id: ModuleId) -> Option<&TyInterner> {
        if module_id == self.input.defs.module_id {
            Some(self.input.interner)
        } else {
            Some(&self.type_normalization_for_module(module_id)?.interner)
        }
    }

    pub(super) fn ensure_working_interner(&mut self, module_id: ModuleId) -> Option<()> {
        if self.working_interners.contains_key(&module_id) {
            return Some(());
        }
        let interner = self.source_interner_for_module(module_id)?.clone();
        self.working_interners.insert(module_id, interner);
        Some(())
    }

    pub(super) fn signatures_for_module(&self, module_id: ModuleId) -> Option<&ItemSignatures> {
        if module_id == self.input.defs.module_id {
            Some(self.input.signatures)
        } else {
            self.input.program.signatures?.get(&module_id)
        }
    }

    pub(super) fn program_enum_signatures(&self) -> HashMap<GlobalDefId, ProgramEnumSignature> {
        let mut enums = HashMap::new();
        if let Some(signatures) = self.input.program.signatures {
            for (module_id, signatures) in signatures {
                for (def_id, signature) in &signatures.enums {
                    let Some(normalization) = self.type_normalization_for_module(*module_id) else {
                        continue;
                    };
                    enums.insert(
                        GlobalDefId {
                            module_id: *module_id,
                            def_id: *def_id,
                        },
                        ProgramEnumSignature {
                            signature: signature.clone(),
                            interner: normalization.interner.clone(),
                        },
                    );
                }
            }
        }
        for (def_id, signature) in &self.input.signatures.enums {
            enums.insert(
                GlobalDefId {
                    module_id: self.input.defs.module_id,
                    def_id: *def_id,
                },
                ProgramEnumSignature {
                    signature: signature.clone(),
                    interner: self.input.interner.clone(),
                },
            );
        }
        enums
    }

    pub(super) fn type_normalization_for_module(
        &self,
        module_id: ModuleId,
    ) -> Option<&nia_type_normalize::TypeNormalization> {
        if module_id == self.input.defs.module_id {
            return None;
        }
        self.input.program.type_normalizations?.get(&module_id)
    }

    pub(super) fn normalized_for_module(
        &self,
        module_id: ModuleId,
    ) -> Option<&HashMap<nia_ids::InternedTyId, nia_ids::InternedTyId>> {
        if module_id == self.input.defs.module_id {
            Some(self.input.normalized)
        } else {
            Some(&self.type_normalization_for_module(module_id)?.normalized)
        }
    }

    pub(super) fn resolve_layout_builtin_for_ty(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        ty: nia_ids::InternedTyId,
    ) -> Result<ComptimeValue, ComptimeError> {
        let module_id = self.current_execution_module_id();
        let layout_array_lengths = self.program_array_lengths_for_layout(ty);
        if self.ensure_working_interner(module_id).is_none() {
            return Err(ComptimeError {
                span,
                message: "cannot compute layout without module type interner".to_string(),
            });
        }
        let Some(defs) = self.global_defs(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute layout without module definitions".to_string(),
            });
        };
        let Some(signatures) = self.signatures_for_module(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute layout without module signatures".to_string(),
            });
        };
        let Some(interner) = self.interner_for_module(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute layout without module type interner".to_string(),
            });
        };
        let Some(normalized) = self.normalized_for_module(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute layout without normalized module types".to_string(),
            });
        };
        let array_lengths = |id| layout_array_lengths.get(&id).copied();
        let layout_query =
            |module_id| self.compute_program_layout(module_id, &layout_array_lengths);
        let layouts = nia_layout::compute_layouts_with_program_context(
            defs,
            interner,
            signatures,
            normalized,
            &array_lengths,
            nia_layout::TargetDataLayout::LP64,
            nia_layout::ProgramLayoutContext {
                layouts: Some(&layout_query),
                array_lengths: Some(&array_lengths),
                ..Default::default()
            },
        );
        let ty = normalized.get(&ty).copied().unwrap_or(ty);
        if let Some(TyKind::Nominal { def_id, args }) = self.ty_kind(ty)
            && (def_id.module_id != module_id || ty.interner_id != module_id)
            && let Some(layouts) =
                self.compute_program_layout(def_id.module_id, &layout_array_lengths)
            && let Some(layout) = layouts.nominal_type_layout(def_id, &args)
        {
            return Ok(ComptimeValue::Int(IntConst::unsigned(
                layout.builtin_value(builtin) as u128,
            )));
        }
        if ty.interner_id != module_id
            && let Some(layouts) =
                self.compute_program_layout(ty.interner_id, &layout_array_lengths)
            && let Some(layout) = layouts.types.get(&ty)
        {
            return Ok(ComptimeValue::Int(IntConst::unsigned(
                layout.builtin_value(builtin) as u128,
            )));
        }
        let Some(layout) = layouts.types.get(&ty) else {
            return Err(ComptimeError {
                span,
                message: format!(
                    "cannot compute layout for comptime builtin `@{}`",
                    builtin.name()
                ),
            });
        };
        Ok(ComptimeValue::Int(IntConst::unsigned(
            layout.builtin_value(builtin) as u128,
        )))
    }

    pub(super) fn resolve_field_offset_builtin_for_ty(
        &mut self,
        span: Span,
        ty: nia_ids::InternedTyId,
        field: &str,
    ) -> Result<ComptimeValue, ComptimeError> {
        let module_id = self.current_execution_module_id();
        let layout_array_lengths = self.program_array_lengths_for_layout(ty);
        if self.ensure_working_interner(module_id).is_none() {
            return Err(ComptimeError {
                span,
                message: "cannot compute field offset without module type interner".to_string(),
            });
        }
        let Some(defs) = self.global_defs(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute field offset without module definitions".to_string(),
            });
        };
        let Some(signatures) = self.signatures_for_module(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute field offset without module signatures".to_string(),
            });
        };
        let Some(interner) = self.interner_for_module(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute field offset without module type interner".to_string(),
            });
        };
        let Some(normalized) = self.normalized_for_module(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute field offset without normalized module types".to_string(),
            });
        };
        let array_lengths = |id| layout_array_lengths.get(&id).copied();
        let layout_query =
            |module_id| self.compute_program_layout(module_id, &layout_array_lengths);
        let layouts = nia_layout::compute_layouts_with_program_context(
            defs,
            interner,
            signatures,
            normalized,
            &array_lengths,
            nia_layout::TargetDataLayout::LP64,
            nia_layout::ProgramLayoutContext {
                layouts: Some(&layout_query),
                array_lengths: Some(&array_lengths),
                ..Default::default()
            },
        );
        let ty = normalized.get(&ty).copied().unwrap_or(ty);
        let Some(TyKind::Nominal { def_id, args }) = self.ty_kind(ty) else {
            return Err(ComptimeError {
                span,
                message: "builtin `@offset` requires a struct or union type argument".to_string(),
            });
        };
        let Some(field_def) = self.field_def_for_nominal(def_id, field) else {
            return Err(ComptimeError {
                span,
                message: format!("type has no field `{field}` for builtin `@offset`"),
            });
        };
        let offset = if def_id.module_id != module_id || ty.interner_id != module_id {
            self.compute_program_layout(def_id.module_id, &layout_array_lengths)
                .and_then(|layouts| layouts.field_offset(def_id, &args, field_def))
        } else {
            layouts.field_offset(def_id, &args, field_def)
        };
        let Some(offset) = offset else {
            return Err(ComptimeError {
                span,
                message: "cannot compute field offset for comptime builtin `@offset`".to_string(),
            });
        };
        Ok(ComptimeValue::Int(IntConst::unsigned(offset as u128)))
    }

    fn field_def_for_nominal(&self, def_id: GlobalDefId, name: &str) -> Option<GlobalDefId> {
        let defs = self.global_defs(def_id.module_id)?;
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
            Some(TyKind::Nominal { def_id, args }) => {
                for arg in args {
                    self.collect_array_len_const_exprs_in_ty_inner(arg, out, seen);
                }
                let Some(signatures) = self.signatures_for_module(def_id.module_id) else {
                    return;
                };
                if let Some(signature) = signatures.structs.get(&def_id.def_id) {
                    for field in &signature.fields {
                        self.collect_array_len_const_exprs_in_ty_inner(field.ty, out, seen);
                    }
                }
                if let Some(signature) = signatures.unions.get(&def_id.def_id) {
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
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in trait_args {
                    self.collect_array_len_const_exprs_in_ty_inner(arg, out, seen);
                }
                for binding in associated_type_bindings {
                    self.collect_array_len_const_exprs_in_ty_inner(binding.ty, out, seen);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.collect_array_len_const_exprs_in_ty_inner(self_ty, out, seen);
                for arg in trait_args {
                    self.collect_array_len_const_exprs_in_ty_inner(arg, out, seen);
                }
            }
            Some(
                TyKind::Range { bound: None, .. }
                | TyKind::Error
                | TyKind::ComptimeOnly
                | TyKind::GenericParam(_)
                | TyKind::Primitive(_)
                | TyKind::Vector { .. },
            )
            | None => {}
        }
    }

    pub(super) fn compute_program_layout(
        &self,
        module_id: ModuleId,
        array_lengths: &HashMap<GlobalConstExprId, u64>,
    ) -> Option<nia_layout::Layouts> {
        let defs = self.global_defs(module_id)?;
        let signatures = self.signatures_for_module(module_id)?;
        let interner = self.source_interner_for_module(module_id)?;
        let normalized = self.normalized_for_module(module_id)?;
        let array_lengths_for_layout = |id: GlobalConstExprId| array_lengths.get(&id).copied();
        let layout_query = |module_id| self.compute_program_layout(module_id, array_lengths);
        Some(nia_layout::compute_layouts_with_program_context(
            defs,
            interner,
            signatures,
            normalized,
            &array_lengths_for_layout,
            nia_layout::TargetDataLayout::LP64,
            nia_layout::ProgramLayoutContext {
                layouts: Some(&layout_query),
                array_lengths: Some(&array_lengths_for_layout),
                ..Default::default()
            },
        ))
    }
}
