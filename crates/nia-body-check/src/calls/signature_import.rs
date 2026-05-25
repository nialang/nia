// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{BodyChecker, ProgramFunctionSignature, ResolvedFunctionSignature};
use nia_ast::Expr;
use nia_ids::{GlobalDefId, TyId};
use nia_item_signatures::FunctionSignature;
use nia_ty::{TyInterner, TyKind};

impl<'a> BodyChecker<'a> {
    pub(super) fn qualified_callee_signature(
        &mut self,
        callee: &Expr,
    ) -> Option<ResolvedFunctionSignature> {
        let def_id = self.values.qualified_values.get(&callee.span)?;
        let program_signature = self.program_functions.get(def_id)?.clone();
        Some(ResolvedFunctionSignature {
            def_id: *def_id,
            signature: self.import_program_function_signature(&program_signature),
        })
    }

    pub(super) fn resolved_function_signature(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<ResolvedFunctionSignature> {
        if def_id.module_id == self.defs.module_id {
            let signature = self.signatures.functions.get(&def_id.def_id)?.clone();
            return Some(ResolvedFunctionSignature { def_id, signature });
        }
        let program_signature = self.program_functions.get(&def_id)?.clone();
        Some(ResolvedFunctionSignature {
            def_id,
            signature: self.import_program_function_signature(&program_signature),
        })
    }

    pub(crate) fn import_program_function_signature(
        &mut self,
        program_signature: &ProgramFunctionSignature,
    ) -> FunctionSignature {
        let mut signature = program_signature.signature.clone();
        signature.params = signature
            .params
            .iter()
            .map(|param| {
                let mut param = param.clone();
                param.ty = self.import_type_from(&program_signature.interner, param.ty);
                param
            })
            .collect();
        signature.return_type =
            self.import_type_from(&program_signature.interner, signature.return_type);
        signature
    }

    pub(crate) fn import_type_from(&mut self, source: &TyInterner, ty: TyId) -> TyId {
        match source.get(ty) {
            Some(TyKind::Error) | None => self.error(),
            Some(TyKind::Primitive(primitive)) => self.primitive(*primitive),
            Some(TyKind::GenericParam(name)) => {
                self.interner.intern(TyKind::GenericParam(name.clone()))
            }
            Some(TyKind::Pointer { is_const, elem }) => {
                let is_const = *is_const;
                let elem = self.import_type_from(source, *elem);
                self.interner.intern(TyKind::Pointer { is_const, elem })
            }
            Some(TyKind::Slice { is_const, elem }) => {
                let is_const = *is_const;
                let elem = self.import_type_from(source, *elem);
                self.interner.intern(TyKind::Slice { is_const, elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let len = len.clone();
                let elem = self.import_type_from(source, *elem);
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
                    .map(|param| self.import_type_from(source, *param))
                    .collect();
                let return_type = self.import_type_from(source, return_type);
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
                    .map(|arg| self.import_type_from(source, *arg))
                    .collect();
                self.interner.intern(TyKind::Nominal { def_id, args })
            }
        }
    }
}
