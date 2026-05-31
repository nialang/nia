// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::BracketArg;
use nia_body_ir::GenericInstantiation;
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_span::Span;
use nia_ty::TyKind;

impl<'a> BodyChecker<'a> {
    pub(super) fn lower_bracket_type_args(
        &mut self,
        type_args: &[BracketArg],
    ) -> Vec<InternedTyId> {
        let mut lowered = Vec::new();
        for arg in type_args {
            if let Some(ty) = &arg.ty {
                lowered.push(self.ty_for_span(ty.span));
            } else {
                if let Some(expr) = &arg.expr {
                    let expr_ty = self.check_expr(expr);
                    if let Some(TyKind::Error) = self.interner.get(expr_ty) {
                        self.diagnostics.push(Diagnostic::error(
                            arg.span,
                            "generic arguments must be types",
                        ));
                    } else {
                        self.diagnostics.push(Diagnostic::error(
                            arg.span,
                            "generic argument resolved as a value; expected a type",
                        ));
                    }
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        arg.span,
                        "generic arguments must be types",
                    ));
                }
            }
        }
        lowered
    }

    pub(super) fn record_generic_instantiation(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        span: Span,
    ) {
        self.generic_instantiations.push(GenericInstantiation {
            def_id,
            args: args.to_vec(),
            generics: self.effective_generics_for_def(def_id),
            span,
            source_def_id: self.current_def_id,
        });
    }

    pub(super) fn effective_generics_for_def(&self, def_id: GlobalDefId) -> Vec<String> {
        let mut generics = self
            .extensions
            .targets()
            .iter()
            .find(|target| target.methods.iter().any(|method| method.def_id == def_id))
            .map(|target| self.generic_params_in_ty(target.target_ty))
            .unwrap_or_default();
        if def_id.module_id == self.defs.module_id {
            if self
                .defs
                .defs
                .get(def_id.def_id)
                .is_some_and(|def| def.kind == nia_defs::DefKind::TraitMethod)
            {
                generics.push("Self".to_string());
            }
            if generics.is_empty() {
                generics = self
                    .defs
                    .defs
                    .get(def_id.def_id)
                    .and_then(|def| def.parent)
                    .and_then(|parent| self.defs.defs.get(parent))
                    .map(|parent| parent.generics.clone())
                    .unwrap_or_default();
            }
            if let Some(def) = self.defs.defs.get(def_id.def_id) {
                generics.extend(def.generics.clone());
            }
        }
        generics
    }

    pub(crate) fn generic_params_in_ty(&self, ty: InternedTyId) -> Vec<String> {
        let mut generics = Vec::new();
        self.collect_generic_params_in_ty(ty, &mut generics);
        generics
    }

    fn collect_generic_params_in_ty(&self, ty: InternedTyId, generics: &mut Vec<String>) {
        match self.interner.get(ty) {
            Some(TyKind::GenericParam(name)) => {
                if !generics.contains(name) {
                    generics.push(name.clone());
                }
            }
            Some(TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. }) => {
                self.collect_generic_params_in_ty(*elem, generics);
            }
            Some(TyKind::Array { elem, .. }) => {
                self.collect_generic_params_in_ty(*elem, generics);
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.collect_generic_params_in_ty(*param, generics);
                }
                self.collect_generic_params_in_ty(*return_type, generics);
            }
            Some(TyKind::Nominal { args, .. }) => {
                for arg in args {
                    self.collect_generic_params_in_ty(*arg, generics);
                }
            }
            Some(TyKind::Error | TyKind::Primitive(_)) | None => {}
        }
    }
}
