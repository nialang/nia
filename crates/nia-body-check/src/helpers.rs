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
        Some(match receiver {
            ReceiverKind::Value => self_nominal,
            ReceiverKind::RefReadOnly => self.interner.intern(TyKind::Pointer {
                is_readonly: true,
                elem: self_nominal,
            }),
            ReceiverKind::Ref => self.interner.intern(TyKind::Pointer {
                is_readonly: false,
                elem: self_nominal,
            }),
        })
    }

    pub(crate) fn method_owner_type(&mut self, def_id: DefId) -> Option<InternedTyId> {
        let method_id = self.global_def_id(def_id);
        for item in self.extensions.targets() {
            if item.methods.iter().any(|method| method.def_id == method_id) {
                return Some(item.target_ty);
            }
        }
        None
    }

    pub(crate) fn receiver_base_type(&self, ty: InternedTyId) -> Option<ReceiverBase> {
        self.receiver_base_type_inner(ty, false, false)
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

    pub(crate) fn struct_generic_substitutions(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> HashMap<String, InternedTyId> {
        self.resolved_struct_signature(def_id)
            .map(|resolved| self.generic_substitutions(&resolved.signature.generics, args))
            .unwrap_or_default()
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
            Some(TyKind::Slice { is_readonly, elem }) => {
                let is_readonly = *is_readonly;
                let elem = *elem;
                let elem = self.substitute_generics(elem, substitutions);
                self.interner.intern(TyKind::Slice { is_readonly, elem })
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
            Some(TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_)) | None => ty,
        }
    }
}
