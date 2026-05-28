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
        if method.kind != DefKind::Method {
            return None;
        }
        let self_nominal = self.method_owner_type(def_id)?;
        let receiver = signature.params.first()?.receiver?;
        Some(match receiver {
            ReceiverKind::Value => self_nominal,
            ReceiverKind::RefConst => self.interner.intern(TyKind::Pointer {
                is_const: true,
                elem: self_nominal,
            }),
            ReceiverKind::Ref => self.interner.intern(TyKind::Pointer {
                is_const: false,
                elem: self_nominal,
            }),
        })
    }

    fn method_owner_type(&mut self, def_id: DefId) -> Option<InternedTyId> {
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
            Some(TyKind::Pointer { is_const, elem }) => {
                self.receiver_base_type_inner(*elem, true, has_readonly_pointer || *is_const)
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
            Some(TyKind::Pointer { is_const, elem }) => {
                let is_const = *is_const;
                let elem = *elem;
                let elem = self.substitute_generics(elem, substitutions);
                self.interner.intern(TyKind::Pointer { is_const, elem })
            }
            Some(TyKind::Slice { is_const, elem }) => {
                let is_const = *is_const;
                let elem = *elem;
                let elem = self.substitute_generics(elem, substitutions);
                self.interner.intern(TyKind::Slice { is_const, elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let len = len.clone();
                let elem = *elem;
                let elem = self.substitute_generics(elem, substitutions);
                self.interner.intern(TyKind::Array { len, elem })
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
            Some(TyKind::Nominal { def_id, args }) => {
                let def_id = *def_id;
                let args = args.clone();
                let args = args
                    .iter()
                    .map(|arg| self.substitute_generics(*arg, substitutions))
                    .collect();
                self.interner.intern(TyKind::Nominal { def_id, args })
            }
            Some(TyKind::Error | TyKind::Primitive(_)) | None => ty,
        }
    }
}
