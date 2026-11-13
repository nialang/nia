// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{BodyChecker, ReceiverBase};
use nia_ast::GenericParamKind;
use nia_defs::{DefId, DefKind};
use nia_ids::{GlobalDefId, InternedTyId, ReceiverKind};
use nia_item_signatures::{FunctionSignature, GenericParamSignature};
use nia_symbol::{SymbolId, SymbolMap};
use nia_ty::{ArrayLenTy, ConstGenericArg, ConstGenericValue, TyKind};

impl<'a> BodyChecker<'a> {
    /// Applies both type and const substitutions to a trait reference argument.
    ///
    /// The argument's type may itself mention generic parameters, while its
    /// value can be a generic const parameter. Keeping these parts in sync is
    /// required because trait dispatch identity includes the complete const
    /// argument, not just its value.
    pub(crate) fn substitute_const_generic_arg_with_self(
        &mut self,
        mut arg: ConstGenericArg,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
        self_ty: InternedTyId,
    ) -> ConstGenericArg {
        if let ConstGenericValue::GenericParam(name) = arg.value
            && let Some(resolved) = const_substitutions.get(&name)
        {
            arg = resolved.clone();
            return self.substitute_const_generic_arg_types_with_self(
                arg,
                substitutions,
                const_substitutions,
                self_ty,
            );
        }
        self.substitute_const_generic_arg_types_with_self(
            arg,
            substitutions,
            const_substitutions,
            self_ty,
        )
    }

    /// Substitutes the types inside a const argument, its own type and a
    /// layout query's operand, through [`nia_ty::substitute_const_generic_arg`].
    fn substitute_const_generic_arg_types_with_self(
        &mut self,
        arg: ConstGenericArg,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
        self_ty: InternedTyId,
    ) -> ConstGenericArg {
        let this = std::cell::RefCell::new(self);
        let substituted: Result<ConstGenericArg, std::convert::Infallible> =
            nia_ty::substitute_const_generic_arg(
                &arg,
                &|name| const_substitutions.get(name).cloned(),
                &|ty| {
                    Ok(this.borrow_mut().substitute_generics_and_consts_with_self(
                        ty,
                        substitutions,
                        const_substitutions,
                        self_ty,
                    ))
                },
            );
        match substituted {
            Ok(arg) => arg,
        }
    }

    pub(crate) fn method_self_type(
        &mut self,
        def_id: DefId,
        signature: &FunctionSignature,
    ) -> Option<InternedTyId> {
        let method = self.defs.defs.get(def_id)?;
        if !matches!(method.kind, DefKind::Method | DefKind::TraitMethod) {
            return None;
        }
        if let Some(self_nominal) = self.method_owner_type(def_id) {
            let receiver = signature.params.first()?.receiver?;
            if let Some(object_ty) = self.method_owner_trait_object_type(def_id) {
                return Some(self.receiver_ty_for_target(object_ty, receiver));
            }
            return Some(self.receiver_ty_for_target(self_nominal, receiver));
        }
        if method.kind == DefKind::TraitMethod {
            let self_nominal = self.interner.intern(TyKind::SelfParam);
            let receiver = signature.params.first()?.receiver?;
            return Some(match receiver {
                ReceiverKind::Value => self_nominal,
                ReceiverKind::RefReadOnly => self.interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem: self_nominal,
                }),
                ReceiverKind::Ref => self.interner.intern(TyKind::Pointer {
                    is_readonly: false,
                    elem: self_nominal,
                }),
            });
        }
        None
    }

    pub(crate) fn method_owner_type(&mut self, def_id: DefId) -> Option<InternedTyId> {
        let method_id = self.global_def_id(def_id);
        self.method_owner_type_by_global(method_id)
    }

    pub(crate) fn method_owner_type_by_global(
        &mut self,
        method_id: GlobalDefId,
    ) -> Option<InternedTyId> {
        self.ensure_extension_method_lookup_for_id(method_id)
            .map(|method| method.target_ty)
    }

    pub(crate) fn method_owner_trait_object_type(&mut self, def_id: DefId) -> Option<InternedTyId> {
        let owner_ty = self.method_owner_type(def_id)?;
        matches!(
            self.interner.get(self.normalization.normalize(owner_ty)),
            Some(TyKind::TraitObjectPointee { .. })
        )
        .then_some(owner_ty)
    }

    pub(crate) fn receiver_base_type(&self, ty: InternedTyId) -> Option<ReceiverBase> {
        self.receiver_base_type_inner(ty, false, false)
    }

    pub(crate) fn receiver_ty_for_target(
        &mut self,
        target_ty: InternedTyId,
        receiver: ReceiverKind,
    ) -> InternedTyId {
        if let Some(TyKind::TraitObject {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
            ..
        }) = self.interner.get(target_ty).cloned()
        {
            return match receiver {
                ReceiverKind::Value => target_ty,
                ReceiverKind::RefReadOnly => self.interner.intern(TyKind::TraitObject {
                    is_readonly: true,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                }),
                ReceiverKind::Ref => self.interner.intern(TyKind::TraitObject {
                    is_readonly: false,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                }),
            };
        }
        if let Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) = self.interner.get(target_ty).cloned()
        {
            return match receiver {
                ReceiverKind::Value => target_ty,
                ReceiverKind::RefReadOnly => self.interner.intern(TyKind::TraitObject {
                    is_readonly: true,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                }),
                ReceiverKind::Ref => self.interner.intern(TyKind::TraitObject {
                    is_readonly: false,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                }),
            };
        }
        if let Some(TyKind::SlicePointee { elem }) = self.interner.get(target_ty).cloned() {
            return match receiver {
                ReceiverKind::Value => target_ty,
                ReceiverKind::RefReadOnly => self.interner.intern(TyKind::Slice {
                    is_readonly: true,
                    elem,
                }),
                ReceiverKind::Ref => self.interner.intern(TyKind::Slice {
                    is_readonly: false,
                    elem,
                }),
            };
        }
        match receiver {
            ReceiverKind::Value => target_ty,
            ReceiverKind::RefReadOnly => self.interner.intern(TyKind::Pointer {
                is_readonly: true,
                elem: target_ty,
            }),
            ReceiverKind::Ref => self.interner.intern(TyKind::Pointer {
                is_readonly: false,
                elem: target_ty,
            }),
        }
    }

    fn receiver_base_type_inner(
        &self,
        ty: InternedTyId,
        from_pointer: bool,
        has_readonly_pointer: bool,
    ) -> Option<ReceiverBase> {
        match self.interner.get(ty) {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => Some(ReceiverBase {
                def_id: *def_id,
                args: args.clone(),
                const_args: const_args.clone(),
                from_pointer,
                has_readonly_pointer,
            }),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                self.receiver_base_type_inner(*elem, true, has_readonly_pointer || *is_readonly)
            }
            _ => None,
        }
    }

    pub(crate) fn global_def_id(&self, def_id: DefId) -> GlobalDefId {
        GlobalDefId {
            module_id: self.defs.module_id,
            def_id,
        }
    }

    pub(crate) fn generic_substitutions(
        &self,
        generics: &[SymbolId],
        args: &[InternedTyId],
    ) -> SymbolMap<InternedTyId> {
        generics
            .iter()
            .zip(args)
            .map(|(name, ty)| (*name, *ty))
            .collect()
    }

    pub(crate) fn current_const_generic_arg(&mut self, name: &SymbolId) -> Option<ConstGenericArg> {
        let current_def_id = self.current_def_id?;
        let ty = self
            .current_const_generic_type(current_def_id, name)
            .or_else(|| self.current_extend_const_generic_type(current_def_id, name))
            .or_else(|| self.current_method_owner_const_generic_type(current_def_id, name))?;
        Some(ConstGenericArg {
            ty,
            value: ConstGenericValue::GenericParam(*name),
        })
    }

    /// The type of a const parameter of the `extend` block that declares the
    /// current method, as `N` in `extend[N: usize] Box : Make[N]`. The block
    /// is not the method's definition parent, and `N` need not appear in
    /// the extended type, so neither of the other lookups finds it.
    fn current_extend_const_generic_type(
        &mut self,
        current_def_id: GlobalDefId,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        if current_def_id.module_id != self.defs.module_id {
            return None;
        }
        self.signatures
            .trait_impls
            .iter()
            .find_map(|impl_signature| {
                impl_signature
                    .methods
                    .iter()
                    .any(|method| method.def_id == current_def_id.def_id)
                    .then(|| {
                        impl_signature
                            .generic_params
                            .iter()
                            .find_map(|param| match &param.kind {
                                nia_item_signatures::GenericParamSignatureKind::Const { ty }
                                    if &param.name == name =>
                                {
                                    Some(*ty)
                                }
                                _ => None,
                            })
                    })
                    .flatten()
            })
    }

    fn current_const_generic_type(
        &mut self,
        current_def_id: GlobalDefId,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        let ty = {
            let defs = self.defs_for_module(current_def_id.module_id)?;
            let defs = defs.as_ref();
            let mut def_id = Some(current_def_id.def_id);
            let mut ty = None;
            while let Some(id) = def_id {
                let def = defs.defs.get(id)?;
                if let Some(param) = def.generic_params.iter().find(|param| {
                    &param.name == name && matches!(param.kind, GenericParamKind::Const { .. })
                }) && let GenericParamKind::Const { ty: param_ty } = &param.kind
                {
                    ty = Some(param_ty.clone());
                    break;
                }
                def_id = def.parent;
            }
            ty
        }?;
        Some(self.ty_for_type(&ty))
    }

    fn current_method_owner_const_generic_type(
        &mut self,
        current_def_id: GlobalDefId,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        if current_def_id.module_id != self.defs.module_id {
            return None;
        }
        let def = self.defs.defs.get(current_def_id.def_id)?;
        if def.kind != DefKind::Method {
            return None;
        }
        let owner_ty = self.method_owner_type(current_def_id.def_id)?;
        self.const_generic_type_from_ty(owner_ty, name)
    }

    fn const_generic_type_from_ty(
        &mut self,
        ty: InternedTyId,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        match self.interner.get(self.normalization.normalize(ty)).cloned()? {
            TyKind::Nominal { const_args, .. } => const_args.into_iter().find_map(|arg| {
                matches!(arg.value, ConstGenericValue::GenericParam(ref arg_name) if arg_name == name)
                    .then_some(arg.ty)
            }),
            TyKind::Array {
                len: ArrayLenTy::GenericParam(len_name),
                ..
            } if &len_name == name => Some(self.primitive(nia_ty::PrimitiveTy::Usize)),
            TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. } => {
                self.const_generic_type_from_ty(elem, name)
            }
            _ => None,
        }
    }

    pub(crate) fn generic_substitutions_and_consts_for_def(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> (SymbolMap<InternedTyId>, SymbolMap<ConstGenericArg>) {
        let Some(defs) = self.defs_for_module(def_id.module_id) else {
            return (SymbolMap::default(), SymbolMap::default());
        };
        let Some(def) = defs.as_ref().defs.get(def_id.def_id) else {
            return (SymbolMap::default(), SymbolMap::default());
        };
        let mut type_index = 0;
        let mut const_index = 0;
        let mut substitutions = SymbolMap::default();
        let mut const_substitutions = SymbolMap::default();
        for param in &def.generic_params {
            match param.kind {
                GenericParamKind::Type => {
                    if let Some(arg) = args.get(type_index).copied() {
                        substitutions.insert(param.name, arg);
                    }
                    type_index += 1;
                }
                GenericParamKind::Const { .. } => {
                    if let Some(arg) = const_args.get(const_index).cloned() {
                        const_substitutions.insert(param.name, arg);
                    }
                    const_index += 1;
                }
            }
        }
        (substitutions, const_substitutions)
    }

    /// Rebuilds substitutions only for a fully applied declaration instance.
    ///
    /// Some call paths intentionally use the partial helper above while they
    /// append inherited owner generics. Exact aliases and declaration-owned
    /// predicates must not do that: accepting one vector with the wrong
    /// type/const split would leave generic parameters silently unsubstituted.
    pub(crate) fn strict_generic_substitutions_and_consts_for_def(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<(SymbolMap<InternedTyId>, SymbolMap<ConstGenericArg>)> {
        let defs = self.defs_for_module(def_id.module_id)?;
        let def = defs.as_ref().defs.get(def_id.def_id)?;
        let expected_type_args = def
            .generic_params
            .iter()
            .filter(|param| matches!(param.kind, GenericParamKind::Type))
            .count();
        let expected_const_args = def
            .generic_params
            .iter()
            .filter(|param| matches!(param.kind, GenericParamKind::Const { .. }))
            .count();
        (args.len() == expected_type_args && const_args.len() == expected_const_args)
            .then(|| self.generic_substitutions_and_consts_for_def(def_id, args, const_args))
    }

    pub(crate) fn nominal_type_generics(&mut self, def_id: GlobalDefId) -> Option<Vec<SymbolId>> {
        if let Some(resolved) = self.resolved_struct_signature(def_id) {
            return Some(resolved.signature.generics);
        }
        if let Some(resolved) = self.resolved_union_signature(def_id) {
            return Some(resolved.signature.generics);
        }
        if let Some(resolved) = self.resolved_enum_signature(def_id) {
            return Some(resolved.signature.generics);
        }
        None
    }

    pub(crate) fn generic_params_for_nominal_def(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<Vec<GenericParamSignature>> {
        // Parameter types belong to the declaration's module. Re-lowering
        // an imported AST TypeRef in this body's context loses its identity;
        // consume the already-resolved signature for local and imported types.
        if let Some(resolved) = self.resolved_struct_signature(def_id) {
            return Some(resolved.signature.generic_params);
        }
        if let Some(resolved) = self.resolved_union_signature(def_id) {
            return Some(resolved.signature.generic_params);
        }
        if let Some(resolved) = self.resolved_enum_signature(def_id) {
            return Some(resolved.signature.generic_params);
        }
        if def_id.module_id == self.defs.module_id
            && let Some(signature) = self.signatures.type_aliases.get(&def_id.def_id)
        {
            return Some(signature.generic_params.clone());
        }
        if let Some(alias) = self.program_signature_scope.type_alias(def_id) {
            return Some(alias.signature.generic_params);
        }
        if let Some(resolved) = self.resolved_trait_signature(def_id) {
            return Some(resolved.generic_params);
        }
        None
    }

    pub(crate) fn expand_type_alias_instance(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<InternedTyId> {
        if def_id.module_id == self.defs.module_id
            && let Some(alias) = self.signatures.type_aliases.get(&def_id.def_id).cloned()
        {
            if alias.generics.len() != args.len() + const_args.len() {
                return Some(self.error());
            }
            let Some((substitutions, const_substitutions)) =
                self.strict_generic_substitutions_and_consts_for_def(def_id, args, const_args)
            else {
                return Some(self.error());
            };
            let target = self.substitute_generics_and_consts(
                alias.target,
                &substitutions,
                &const_substitutions,
            );
            return Some(self.normalize_aliases_in_type(target));
        }
        if let Some(alias) = self.program_signature_scope.type_alias(def_id) {
            if alias.signature.generics.len() != args.len() + const_args.len() {
                return Some(self.error());
            }
            let Some((substitutions, const_substitutions)) =
                self.strict_generic_substitutions_and_consts_for_def(def_id, args, const_args)
            else {
                return Some(self.error());
            };
            let target = alias.signature.target;
            let target =
                self.substitute_generics_and_consts(target, &substitutions, &const_substitutions);
            return Some(self.normalize_aliases_in_type(target));
        }
        None
    }

    /// Normalize types synthesized after the explicit-type normalization pass
    /// with the same recursive normalizer and raw cross-module alias provider.
    pub(crate) fn normalize_aliases_in_type(&mut self, ty: InternedTyId) -> InternedTyId {
        let ty = self.normalize_aliases(ty);
        if let Some(normalized) = self.dynamic_normalized_types.get(&ty) {
            return *normalized;
        }
        let external_alias = |def_id| {
            self.program_signature_scope
                .type_alias(def_id)
                .map(|alias| alias.signature)
        };
        match nia_type_normalize::normalize_module_types(
            nia_type_normalize::TypeNormalizationInput {
                module_id: self.defs.module_id,
                type_store: self.type_store,
                input_ids: &[ty],
                signatures: self.const_signatures,
                external_alias: Some(&external_alias),
            },
        ) {
            Ok(normalization) => {
                let normalization = match normalization.resolve_array_lengths(
                    self.defs.module_id,
                    self.type_store,
                    &[ty],
                    &|id| self.const_eval.symbolic_array_lengths.get(id).cloned(),
                ) {
                    Ok(normalization) => normalization,
                    Err(error) => {
                        self.failure.record(error);
                        return self.error();
                    }
                };
                let normalized = normalization.normalize(ty);
                self.dynamic_normalized_types
                    .extend(normalization.normalized);
                for diagnostic in normalization.diagnostics {
                    if !self.diagnostics.contains(&diagnostic) {
                        self.diagnostics.push(diagnostic);
                    }
                }
                normalized
            }
            Err(error) => {
                self.failure.record(error);
                self.error()
            }
        }
    }

    pub(crate) fn substitute_generics(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> InternedTyId {
        self.substitute_generics_and_consts(ty, substitutions, &SymbolMap::default())
    }

    pub(crate) fn substitute_generics_with_self(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
        self_ty: InternedTyId,
    ) -> InternedTyId {
        self.substitute_generics_and_consts_with_self(
            ty,
            substitutions,
            &SymbolMap::default(),
            self_ty,
        )
    }

    pub(crate) fn substitute_generics_and_consts(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> InternedTyId {
        match nia_ty::substitute_ty(
            self.type_store,
            &self.interner.append,
            ty,
            &|name| substitutions.get(name).copied(),
            &|name| const_substitutions.get(name).cloned(),
            None,
        ) {
            Ok(ty) => ty,
            Err(error) => {
                self.record_internal(error);
                self.interner.error()
            }
        }
    }

    pub(crate) fn substitute_generics_and_consts_with_self(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
        self_ty: InternedTyId,
    ) -> InternedTyId {
        match nia_ty::substitute_ty(
            self.type_store,
            &self.interner.append,
            ty,
            &|name| substitutions.get(name).copied(),
            &|name| const_substitutions.get(name).cloned(),
            Some(self_ty),
        ) {
            Ok(ty) => ty,
            Err(error) => {
                self.record_internal(error);
                self.interner.error()
            }
        }
    }
}
