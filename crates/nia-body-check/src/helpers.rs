// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::{BodyChecker, ReceiverBase};
use nia_ast::ReceiverKind;
use nia_defs::{DefId, DefKind};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_item_signatures::FunctionSignature;
use nia_ty::TyKind;

impl<'a> BodyChecker<'a> {
    pub(crate) fn method_self_type(
        &mut self,
        def_id: DefId,
        signature: &FunctionSignature,
    ) -> Option<InternedTyId> {
        let method = self.defs.defs.get(def_id)?;
        if !matches!(method.kind, DefKind::Method | DefKind::TraitMethod) {
            return None;
        }
        if method.kind == DefKind::TraitMethod {
            let self_nominal = self
                .interner
                .intern(TyKind::GenericParam("Self".to_string()));
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
        let self_nominal = self.method_owner_type(def_id)?;
        let receiver = signature.params.first()?.receiver?;
        if self.method_owner_trait_object_type(def_id).is_some() {
            let self_generic = self
                .interner
                .intern(TyKind::GenericParam("Self".to_string()));
            return Some(self.receiver_ty_for_target(self_generic, receiver));
        }
        Some(self.receiver_ty_for_target(self_nominal, receiver))
    }

    pub(crate) fn method_owner_type(&mut self, def_id: DefId) -> Option<InternedTyId> {
        let method_id = self.global_def_id(def_id);
        self.extension_methods_by_id
            .get(&method_id)
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
                    associated_type_bindings,
                }),
                ReceiverKind::Ref => self.interner.intern(TyKind::TraitObject {
                    is_readonly: false,
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                }),
            };
        }
        if let Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            associated_type_bindings,
        }) = self.interner.get(target_ty).cloned()
        {
            return match receiver {
                ReceiverKind::Value => target_ty,
                ReceiverKind::RefReadOnly => self.interner.intern(TyKind::TraitObject {
                    is_readonly: true,
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                }),
                ReceiverKind::Ref => self.interner.intern(TyKind::TraitObject {
                    is_readonly: false,
                    trait_id,
                    trait_args,
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
            Some(TyKind::Nominal { def_id, args }) => Some(ReceiverBase {
                def_id: *def_id,
                args: args.clone(),
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
        generics: &[String],
        args: &[InternedTyId],
    ) -> HashMap<String, InternedTyId> {
        generics
            .iter()
            .zip(args)
            .map(|(name, ty)| (name.clone(), *ty))
            .collect()
    }

    pub(crate) fn nominal_type_generics(&mut self, def_id: GlobalDefId) -> Option<Vec<String>> {
        if let Some(resolved) = self.resolved_struct_signature(def_id) {
            return Some(resolved.signature.generics);
        }
        if let Some(resolved) = self.resolved_union_signature(def_id) {
            return Some(resolved.signature.generics);
        }
        if self.resolved_enum_signature(def_id).is_some() {
            return Some(Vec::new());
        }
        None
    }

    pub(crate) fn nominal_type_generic_substitutions(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> HashMap<String, InternedTyId> {
        self.nominal_type_generics(def_id)
            .map(|generics| self.generic_substitutions(&generics, args))
            .unwrap_or_default()
    }

    pub(crate) fn expand_type_alias_instance(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> Option<InternedTyId> {
        if def_id.module_id == self.defs.module_id
            && let Some(alias) = self.signatures.type_aliases.get(&def_id.def_id).cloned()
        {
            if alias.generics.len() != args.len() {
                return Some(self.error());
            }
            let substitutions = self.generic_substitutions(&alias.generics, args);
            let target = self.substitute_generics(alias.target, &substitutions);
            return Some(self.normalize_aliases_in_type(target));
        }
        if let Some(alias) = self.program_type_aliases.get(&def_id).cloned() {
            if alias.signature.generics.len() != args.len() {
                return Some(self.error());
            }
            let substitutions = self.generic_substitutions(&alias.signature.generics, args);
            let target = self.import_type_from(&alias.interner, alias.signature.target);
            let target = self.substitute_generics(target, &substitutions);
            return Some(self.normalize_aliases_in_type(target));
        }
        None
    }

    pub(crate) fn normalize_aliases_in_type(&mut self, ty: InternedTyId) -> InternedTyId {
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Nominal { def_id, args }) => {
                self.expand_type_alias_instance(def_id, &args).unwrap_or(ty)
            }
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.normalize_aliases_in_type(elem);
                self.interner.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let elem = self.normalize_aliases_in_type(elem);
                self.interner
                    .intern(TyKind::VolatilePointer { is_readonly, elem })
            }
            _ => ty,
        }
    }

    pub(crate) fn substitute_generics(
        &mut self,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> InternedTyId {
        match self.interner.get(ty) {
            Some(TyKind::GenericParam(name)) => substitutions.get(name).copied().unwrap_or(ty),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let is_readonly = *is_readonly;
                let elem = *elem;
                let elem = self.substitute_generics(elem, substitutions);
                self.interner.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let is_readonly = *is_readonly;
                let elem = *elem;
                let elem = self.substitute_generics(elem, substitutions);
                self.interner
                    .intern(TyKind::VolatilePointer { is_readonly, elem })
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let is_readonly = *is_readonly;
                let elem = *elem;
                let elem = self.substitute_generics(elem, substitutions);
                self.interner.intern(TyKind::Slice { is_readonly, elem })
            }
            Some(TyKind::SlicePointee { elem }) => {
                let elem = *elem;
                let elem = self.substitute_generics(elem, substitutions);
                self.interner.intern(TyKind::SlicePointee { elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let len = len.clone();
                let elem = *elem;
                let elem = self.substitute_generics(elem, substitutions);
                self.interner.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::Range { kind, bound }) => {
                let kind = *kind;
                let bound = bound.map(|bound| self.substitute_generics(bound, substitutions));
                self.interner.intern(TyKind::Range { kind, bound })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params.clone();
                let return_type = *return_type;
                let is_variadic = *is_variadic;
                let params = params
                    .iter()
                    .map(|param| self.substitute_generics(*param, substitutions))
                    .collect();
                let return_type = self.substitute_generics(return_type, substitutions);
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Optional { elem }) => {
                let elem = *elem;
                let elem = self.substitute_generics(elem, substitutions);
                self.interner.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = *error;
                let value = *value;
                let error = self.substitute_generics(error, substitutions);
                let value = self.substitute_generics(value, substitutions);
                self.interner.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal { def_id, args }) => {
                let def_id = *def_id;
                let args = args.clone();
                let args = args
                    .iter()
                    .map(|arg| self.substitute_generics(*arg, substitutions))
                    .collect();
                self.interner.intern(TyKind::Nominal { def_id, args })
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let trait_id = *trait_id;
                let args = args.clone();
                let args = args
                    .iter()
                    .map(|arg| self.substitute_generics(*arg, substitutions))
                    .collect();
                self.interner
                    .intern(TyKind::BuiltinTrait { trait_id, args })
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => {
                let is_readonly = *is_readonly;
                let trait_id = *trait_id;
                let trait_args = trait_args.clone();
                let associated_type_bindings = associated_type_bindings.clone();
                let trait_args = trait_args
                    .iter()
                    .map(|arg| self.substitute_generics(*arg, substitutions))
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .iter()
                            .map(|arg| self.substitute_generics(*arg, substitutions))
                            .collect(),
                        name: binding.name.clone(),
                        ty: self.substitute_generics(binding.ty, substitutions),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => {
                let trait_id = *trait_id;
                let trait_args = trait_args.clone();
                let associated_type_bindings = associated_type_bindings.clone();
                let trait_args = trait_args
                    .iter()
                    .map(|arg| self.substitute_generics(*arg, substitutions))
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .iter()
                            .map(|arg| self.substitute_generics(*arg, substitutions))
                            .collect(),
                        name: binding.name.clone(),
                        ty: self.substitute_generics(binding.ty, substitutions),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = *self_ty;
                let trait_id = *trait_id;
                let trait_args = trait_args.clone();
                let name = name.clone();
                let self_ty = self.substitute_generics(self_ty, substitutions);
                let trait_args = trait_args
                    .iter()
                    .map(|arg| self.substitute_generics(*arg, substitutions))
                    .collect();
                self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    name,
                })
            }
            Some(
                TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_) | TyKind::Vector { .. },
            )
            | None => ty,
        }
    }
}
