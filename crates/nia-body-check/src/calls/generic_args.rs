// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::BracketArg;
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_sema_ir::GenericInstantiation;
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
                lowered.push(self.ty_for_type(ty));
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

    pub(crate) fn record_generic_instantiation(
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
            .flat_map(|target| target.methods.iter())
            .find(|method| method.def_id == def_id)
            .map(|method| method.impl_generics.clone())
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
}
