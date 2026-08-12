// SPDX-License-Identifier: GPL-3.0-or-later
//! Trait-object arguments, associated bindings, and projection target lowering.

use super::*;

impl<'a> TypeLowerer<'a, '_> {
    pub(crate) fn lower_trait_object_type(
        &mut self,
        is_readonly: bool,
        ty: &TypeRef,
    ) -> Option<InternedTyId> {
        let TypeKind::Path { segments } = &ty.kind else {
            return None;
        };
        let type_segment = type_name_segment(segments)?;
        match self
            .resolved
            .node_type_names
            .get(ty.node_key.site())
            .copied()
        {
            Some(TypeNameResolution::BuiltinTrait(trait_id)) => {
                Some(self.lower_builtin_trait_object(ty.span, is_readonly, type_segment, trait_id))
            }
            Some(TypeNameResolution::Def(def_id)) => {
                let def_id = self
                    .resolved
                    .node_qualified_type_names
                    .get(ty.node_key.site())
                    .copied()
                    .unwrap_or(GlobalDefId {
                        module_id: self.module_id,
                        def_id,
                    });
                self.lower_source_trait_object(ty.span, is_readonly, type_segment, def_id)
            }
            Some(TypeNameResolution::External(def_id)) => {
                self.lower_source_trait_object(ty.span, is_readonly, type_segment, def_id)
            }
            _ => None,
        }
    }

    pub(crate) fn lower_source_trait_object(
        &mut self,
        span: Span,
        is_readonly: bool,
        segment: &TypePathSegment,
        def_id: GlobalDefId,
    ) -> Option<InternedTyId> {
        if !self.is_trait_def(def_id) {
            return None;
        }
        let object_args = self.lower_trait_object_args(span, segment, TraitId::Source(def_id))?;
        self.check_type_arg_count(
            span,
            def_id,
            object_args.trait_args.len() + object_args.trait_const_args.len(),
        );
        Some(self.append.intern(TyKind::TraitObject {
            is_readonly,
            trait_id: TraitId::Source(def_id),
            trait_args: object_args.trait_args,
            trait_const_args: object_args.trait_const_args,
            associated_type_bindings: object_args.associated_type_bindings,
        }))
    }

    pub(crate) fn lower_builtin_trait_object(
        &mut self,
        span: Span,
        is_readonly: bool,
        segment: &TypePathSegment,
        trait_id: BuiltinTrait,
    ) -> InternedTyId {
        let object_args = self
            .lower_trait_object_args(span, segment, TraitId::Builtin(trait_id))
            .unwrap_or_default();
        self.check_builtin_trait_arg_count(span, trait_id, object_args.trait_args.len());
        self.append.intern(TyKind::TraitObject {
            is_readonly,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: object_args.trait_args,
            trait_const_args: object_args.trait_const_args,
            associated_type_bindings: object_args.associated_type_bindings,
        })
    }

    pub(crate) fn lower_trait_object_args(
        &mut self,
        _span: Span,
        segment: &TypePathSegment,
        trait_id: TraitId,
    ) -> Option<TraitObjectArgs> {
        let mut object_args = TraitObjectArgs::default();
        let mut seen_assoc_bindings = HashSet::new();
        let mut seen_assoc_binding = false;
        let generic_params = match trait_id {
            TraitId::Source(def_id) => self.generic_params_for_def(def_id).unwrap_or_default(),
            TraitId::Builtin(_) => Vec::new(),
        };
        let generic_owner_module_id = match trait_id {
            TraitId::Source(def_id) => def_id.module_id,
            TraitId::Builtin(_) => self.module_id,
        };
        let mut positional_index = 0usize;
        // Positional arguments form a prefix. Once a binding appears, later positional arguments
        // are diagnosed instead of reordered, preserving source intent and generic positions.
        for arg in &segment.args {
            match arg {
                TypeArg::Type(arg_ty) => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_type_ref(arg_ty)
                            else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    arg_ty.span,
                                    "expected const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_generic_param_type(generic_owner_module_id, ty);
                            object_args
                                .trait_const_args
                                .push(ConstGenericArg { ty, value });
                        }
                        _ => object_args
                            .trait_args
                            .push(self.lower_type_in_context(arg_ty, TypeContext::Value)),
                    }
                    positional_index += 1;
                }
                TypeArg::Const(expr) => {
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_expr(expr) else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    expr.span,
                                    "expected const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_generic_param_type(generic_owner_module_id, ty);
                            object_args
                                .trait_const_args
                                .push(ConstGenericArg { ty, value });
                        }
                        _ => {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_NORMALIZATION,
                                expr.span,
                                "const value generic argument supplied for type parameter",
                            ));
                        }
                    }
                    positional_index += 1;
                }
                TypeArg::TypeOrConst { ty: arg_ty, expr } => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_expr(expr) else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    expr.span,
                                    "expected const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_generic_param_type(generic_owner_module_id, ty);
                            object_args
                                .trait_const_args
                                .push(ConstGenericArg { ty, value });
                        }
                        _ => object_args
                            .trait_args
                            .push(self.lower_type_or_const_type_arg(arg_ty)),
                    }
                    positional_index += 1;
                }
                TypeArg::AssocBinding {
                    key,
                    span,
                    ty: binding_ty,
                } => {
                    seen_assoc_binding = true;
                    let binding_ty = self.lower_type_in_context(binding_ty, TypeContext::Value);
                    let Some(LoweredAssocBindingKey {
                        name,
                        trait_id: binding_trait_id,
                        trait_args: binding_trait_args,
                        trait_const_args: binding_trait_const_args,
                    }) = self.lower_assoc_binding_key(key, Some(trait_id))
                    else {
                        continue;
                    };
                    let seen_key = self.assoc_binding_seen_key(
                        name,
                        binding_trait_id,
                        &binding_trait_args,
                        &binding_trait_const_args,
                    );
                    if !seen_assoc_bindings.insert(seen_key) {
                        let name = self.symbol_name(*name);
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            *span,
                            format!("duplicate associated type binding `{name}`"),
                        ));
                    }
                    let effective_trait = binding_trait_id.unwrap_or(trait_id);
                    if !self.trait_id_has_associated_type(effective_trait, name) {
                        let name = self.symbol_name(*name);
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            *span,
                            format!("trait does not define associated type `{name}`"),
                        ));
                    }
                    object_args
                        .associated_type_bindings
                        .push(AssociatedTypeBindingTy {
                            trait_id: binding_trait_id,
                            trait_args: binding_trait_args,
                            trait_const_args: binding_trait_const_args,
                            name: *name,
                            ty: binding_ty,
                        });
                }
            }
        }
        Some(object_args)
    }

    pub(crate) fn lower_assoc_binding_key<'b>(
        &mut self,
        key: &'b AssocBindingKey,
        target_trait: Option<TraitId>,
    ) -> Option<LoweredAssocBindingKey<'b>> {
        match key {
            AssocBindingKey::Name(name) => Some(LoweredAssocBindingKey {
                name,
                trait_id: None,
                trait_args: Vec::new(),
                trait_const_args: Vec::new(),
            }),
            AssocBindingKey::Projection(projection) => {
                let TypeKind::Projection {
                    ty,
                    trait_ref,
                    name,
                } = &projection.kind
                else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        projection.span,
                        "associated type binding projection key must be `[Self as Trait]::SymbolId`",
                    ));
                    return None;
                };
                if !matches!(ty.kind, TypeKind::SelfType) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        ty.span,
                        "associated type binding projection must project from `Self`",
                    ));
                }
                let lowered_trait = self.lower_type_in_context(trait_ref, TypeContext::TraitBound);
                let (trait_id, trait_args, trait_const_args) = match self
                    .type_store
                    .get(self.normalize_if_known(lowered_trait))
                    .cloned()
                {
                    Some(TyKind::Nominal {
                        def_id,
                        args,
                        const_args,
                    }) => (TraitId::Source(def_id), args, const_args),
                    Some(TyKind::BuiltinTrait { trait_id, args }) => {
                        (TraitId::Builtin(trait_id), args, Vec::new())
                    }
                    Some(TyKind::TraitObject {
                        trait_id,
                        trait_args,
                        trait_const_args,
                        ..
                    }) => (trait_id, trait_args, trait_const_args),
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            trait_ref.span,
                            "associated type binding projection trait must resolve to a trait",
                        ));
                        return None;
                    }
                };
                if let Some(target_trait) = target_trait
                    && trait_id == target_trait
                {
                    return Some(LoweredAssocBindingKey {
                        name,
                        trait_id: None,
                        trait_args,
                        trait_const_args,
                    });
                }
                Some(LoweredAssocBindingKey {
                    name,
                    trait_id: Some(trait_id),
                    trait_args,
                    trait_const_args,
                })
            }
        }
    }

    pub(crate) fn assoc_binding_seen_key(
        &self,
        name: &SymbolId,
        trait_id: Option<TraitId>,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
    ) -> String {
        format!(
            "{trait_id:?}:{trait_args:?}:{trait_const_args:?}:{}",
            symbol_identity_key(*name)
        )
    }

    pub(crate) fn lower_builtin_trait_or_extend_target_type(
        &mut self,
        span: Span,
        segment: &TypePathSegment,
        trait_id: BuiltinTrait,
        context: TypeContext,
    ) -> InternedTyId {
        if context == TypeContext::ExtendTarget {
            let object_args = self
                .lower_trait_object_args(span, segment, TraitId::Builtin(trait_id))
                .unwrap_or_default();
            self.check_builtin_trait_arg_count(span, trait_id, object_args.trait_args.len());
            return self.append.intern(TyKind::TraitObjectPointee {
                trait_id: TraitId::Builtin(trait_id),
                trait_args: object_args.trait_args,
                trait_const_args: object_args.trait_const_args,
                associated_type_bindings: object_args.associated_type_bindings,
            });
        }
        let mut args = Vec::new();
        let mut seen_assoc_bindings = HashSet::new();
        let mut seen_assoc_binding = false;
        for arg in &segment.args {
            match arg {
                TypeArg::Type(arg_ty) => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    args.push(self.lower_type_in_context(arg_ty, TypeContext::Value));
                }
                TypeArg::Const(expr) => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        expr.span,
                        "const value generic arguments are not supported",
                    ));
                }
                TypeArg::TypeOrConst { ty, .. } => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    args.push(self.lower_type_or_const_type_arg(ty));
                }
                TypeArg::AssocBinding {
                    key,
                    span,
                    ty: binding_ty,
                } => {
                    seen_assoc_binding = true;
                    if context == TypeContext::TraitBound {
                        self.lower_type_in_context(binding_ty, TypeContext::Value);
                        let Some(LoweredAssocBindingKey {
                            name,
                            trait_id: binding_trait_id,
                            trait_args: binding_trait_args,
                            trait_const_args: binding_trait_const_args,
                        }) = self.lower_assoc_binding_key(key, Some(TraitId::Builtin(trait_id)))
                        else {
                            continue;
                        };
                        let seen_key = self.assoc_binding_seen_key(
                            name,
                            binding_trait_id,
                            &binding_trait_args,
                            &binding_trait_const_args,
                        );
                        if !seen_assoc_bindings.insert(seen_key) {
                            let name = self.symbol_name(*name);
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_NORMALIZATION,
                                *span,
                                format!("duplicate associated type binding `{name}`"),
                            ));
                        }
                        let valid = match binding_trait_id {
                            Some(TraitId::Builtin(binding_trait)) => {
                                builtin_trait_has_associated_type(binding_trait, name)
                            }
                            Some(TraitId::Source(def_id)) => {
                                self.trait_has_associated_type(def_id, name)
                            }
                            None => builtin_trait_has_associated_type(trait_id, name),
                        };
                        if !valid {
                            let name = self.symbol_name(*name);
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_NORMALIZATION,
                                *span,
                                format!("trait does not define associated type `{name}`"),
                            ));
                        }
                    } else {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            *span,
                            "associated type bindings are only valid in trait bounds",
                        ));
                    }
                }
            }
        }
        self.check_builtin_trait_arg_count(span, trait_id, args.len());
        self.append.intern(TyKind::BuiltinTrait { trait_id, args })
    }

    pub(crate) fn projection_trait_id(
        &mut self,
        trait_ty: InternedTyId,
    ) -> Option<(TraitId, Vec<InternedTyId>, Vec<ConstGenericArg>)> {
        match self.type_store.get(trait_ty).cloned() {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) if self.is_trait_def(def_id) => Some((TraitId::Source(def_id), args, const_args)),
            Some(TyKind::TraitObject {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            }) => Some((trait_id, trait_args, trait_const_args)),
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                Some((TraitId::Builtin(trait_id), args, Vec::new()))
            }
            _ => None,
        }
    }

    pub(crate) fn is_trait_def(&mut self, def_id: GlobalDefId) -> bool {
        self.defs_for_module(def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id).map(|def| def.kind))
            == Some(nia_defs::DefKind::Trait)
    }

    pub(crate) fn trait_has_associated_type(
        &mut self,
        trait_id: GlobalDefId,
        name: &SymbolId,
    ) -> bool {
        let Some(defs) = self.defs_for_module(trait_id.module_id) else {
            return true;
        };
        let Some(members) = defs.scopes.struct_members.get(&trait_id.def_id) else {
            return true;
        };
        members.fields.get(name).is_some_and(|def_id| {
            defs.defs
                .get(def_id)
                .is_some_and(|def| def.kind == nia_defs::DefKind::TraitAssociatedType)
        })
    }

    pub(crate) fn trait_id_has_associated_type(
        &mut self,
        trait_id: TraitId,
        name: &SymbolId,
    ) -> bool {
        match trait_id {
            TraitId::Source(def_id) => self.trait_has_associated_type(def_id, name),
            TraitId::Builtin(trait_id) => builtin_trait_has_associated_type(trait_id, name),
        }
    }

    pub(crate) fn check_type_arg_count(&mut self, span: Span, def_id: GlobalDefId, actual: usize) {
        let Some(defs) = self.defs_for_module(def_id.module_id) else {
            return;
        };
        let Some(def) = defs.defs.get(def_id.def_id) else {
            return;
        };
        let expected = def.generics.len();
        let name = def.name;
        if expected != actual {
            let name = self.symbol_name(name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                format!(
                    "generic argument count mismatch for `{name}`: expected {expected}, got {actual}"
                ),
            ));
        }
    }

    pub(crate) fn generic_params_for_def(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<Vec<GenericParam>> {
        self.defs_for_module(def_id.module_id).and_then(|defs| {
            defs.defs
                .get(def_id.def_id)
                .map(|def| def.generic_params.clone())
        })
    }

    pub(crate) fn lower_generic_param_type(
        &mut self,
        owner_module_id: ModuleId,
        ty: &TypeRef,
    ) -> InternedTyId {
        if owner_module_id == self.module_id {
            return self.lower_type_in_context(ty, TypeContext::Value);
        }
        let TypeKind::Path { segments } = &ty.kind else {
            return self.append.intern(TyKind::Error);
        };
        let [segment] = segments.as_slice() else {
            return self.append.intern(TyKind::Error);
        };
        if !segment.args.is_empty() {
            return self.append.intern(TyKind::Error);
        }
        let Some(name) = type_path_segment_name(segment) else {
            return self.append.intern(TyKind::Error);
        };
        PrimitiveTy::from_known_symbol(*name)
            .map(|primitive| self.append.intern(TyKind::Primitive(primitive)))
            .unwrap_or_else(|| self.append.intern(TyKind::Error))
    }

    pub(crate) fn const_generic_value_from_type_ref(
        &self,
        ty: &TypeRef,
    ) -> Option<ConstGenericValue> {
        let TypeKind::Path { segments } = &ty.kind else {
            return None;
        };
        if segments.len() == 1 && segments[0].args.is_empty() {
            let name = type_path_segment_name(&segments[0])?;
            if self.is_const_generic_param(name) {
                return Some(ConstGenericValue::GenericParam(*name));
            }
        }
        None
    }

    pub(crate) fn lower_const_generic_value_from_type_ref(
        &mut self,
        ty: &TypeRef,
    ) -> Option<ConstGenericValue> {
        if let Some(value) = self.const_generic_value_from_type_ref(ty) {
            return Some(value);
        }
        let TypeKind::Path { segments } = &ty.kind else {
            return None;
        };
        if segments.iter().any(|segment| !segment.args.is_empty()) {
            return None;
        }
        let expr = expr_from_type_path(ty.span, ty.node_key.clone(), segments)?;
        self.lower_const_generic_value_from_expr(&expr)
    }

    pub(crate) fn lower_const_generic_value_from_expr(
        &mut self,
        expr: &Expr,
    ) -> Option<ConstGenericValue> {
        if let ExprKind::Ident(name) = &expr.kind
            && self.is_const_generic_param(name)
        {
            return Some(ConstGenericValue::GenericParam(*name));
        }
        if let ExprKind::Bool(value) = &expr.kind {
            return Some(ConstGenericValue::Bool(*value));
        }
        if let ExprKind::Integer(text) = &expr.kind {
            return parse_integer_const_generic(text).map(ConstGenericValue::Int);
        }
        Some(ConstGenericValue::ConstExpr(
            self.register_const_expr_value(expr),
        ))
    }

    pub(crate) fn check_builtin_trait_arg_count(
        &mut self,
        span: Span,
        trait_id: BuiltinTrait,
        actual: usize,
    ) {
        let expected = trait_id.generic_count();
        if expected != actual {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                format!(
                    "generic argument count mismatch for `{}`: expected {expected}, got {actual}",
                    trait_id.name()
                ),
            ));
        }
    }

    pub(crate) fn with_generics(&mut self, generics: &[GenericParam], f: impl FnOnce(&mut Self)) {
        for generic in generics {
            if let GenericParamKind::Const { ty } = &generic.kind {
                self.lower_type_in_context(ty, TypeContext::Value);
            }
        }
        self.generic_stack.push(generics.to_vec());
        f(self);
        // Restore the lexical environment after processing the declaration. Diagnostic recovery
        // must not leak a failed declaration's generic parameters into its siblings.
        self.generic_stack.pop();
    }

    pub(crate) fn is_const_generic_param(&self, name: &SymbolId) -> bool {
        self.generic_stack.iter().rev().any(|generics| {
            generics.iter().any(|generic| {
                &generic.name == name && matches!(generic.kind, GenericParamKind::Const { .. })
            })
        })
    }

    pub(crate) fn with_self_type(&mut self, self_ty: InternedTyId, f: impl FnOnce(&mut Self)) {
        self.self_type_stack.push(self_ty);
        f(self);
        self.self_type_stack.pop();
    }

    pub(crate) fn with_associated_type_scope(
        &mut self,
        scope: AssociatedTypeScope,
        f: impl FnOnce(&mut Self),
    ) {
        self.associated_type_scope_stack.push(scope);
        f(self);
        self.associated_type_scope_stack.pop();
    }

    pub(crate) fn lower_scoped_associated_type(
        &mut self,
        span: Span,
        name: &SymbolId,
        segment: &TypePathSegment,
    ) -> InternedTyId {
        if !segment.args.is_empty() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                "associated type shorthand cannot take generic arguments",
            ));
            return self.append.intern(TyKind::Error);
        }
        let Some(scope) = self
            .associated_type_scope_stack
            .iter()
            .rev()
            .find(|scope| scope.names.iter().any(|associated| associated == name))
            .cloned()
        else {
            let name = self.symbol_name(*name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                format!("unknown associated type `{name}`"),
            ));
            return self.append.intern(TyKind::Error);
        };
        self.append.intern(TyKind::Projection {
            self_ty: scope.self_ty,
            trait_id: scope.trait_id,
            trait_args: scope.trait_args,
            trait_const_args: scope.trait_const_args,
            name: *name,
        })
    }

    pub(crate) fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(self.symbols, symbol)
    }

    pub(crate) fn local_trait_id(
        &mut self,
        node_key: &nia_node_id::VersionedNodeKey,
    ) -> Option<GlobalDefId> {
        let defs = self.defs_for_module(self.module_id)?;
        let def_id = defs.def_nodes.get(node_key)?;
        Some(GlobalDefId {
            module_id: self.module_id,
            def_id,
        })
    }

    pub(crate) fn associated_type_scope_for_trait_impl(
        &mut self,
        self_ty: InternedTyId,
        trait_ty: InternedTyId,
    ) -> Option<AssociatedTypeScope> {
        let trait_ty = self.normalize_if_known(trait_ty);
        let (trait_id, trait_args, trait_const_args) = self.projection_trait_id(trait_ty)?;
        let names = match trait_id {
            TraitId::Source(def_id) => self.source_trait_associated_type_names(def_id),
            TraitId::Builtin(_) => Vec::new(),
        };
        Some(AssociatedTypeScope {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            names,
        })
    }

    pub(crate) fn source_trait_associated_type_names(
        &mut self,
        trait_id: GlobalDefId,
    ) -> Vec<SymbolId> {
        let Some(defs) = self.defs_for_module(trait_id.module_id) else {
            return Vec::new();
        };
        defs.defs
            .iter()
            .filter_map(|(_, def)| {
                (def.parent == Some(trait_id.def_id)
                    && def.kind == nia_defs::DefKind::TraitAssociatedType)
                    .then_some(def.name)
            })
            .collect()
    }
}
