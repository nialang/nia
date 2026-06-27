// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::BracketArg;
use nia_diagnostic::{Diagnostic, codes};
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
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            arg.span,
                            "generic arguments must be types",
                        ));
                    } else {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            arg.span,
                            "generic argument resolved as a value; expected a type",
                        ));
                    }
                } else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        arg.span,
                        "generic arguments must be types",
                    ));
                }
            }
        }
        lowered
    }

    pub(crate) fn complete_instance_args_for_generics(
        &mut self,
        span: Span,
        generics: &[String],
        substitutions: &std::collections::HashMap<String, InternedTyId>,
    ) -> Option<Vec<InternedTyId>> {
        let mut complete = true;
        for generic in generics {
            if !substitutions.contains_key(generic) {
                complete = false;
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("cannot infer generic parameter `{generic}`"),
                ));
            }
        }
        if !complete {
            return None;
        }
        Some(
            generics
                .iter()
                .filter_map(|generic| substitutions.get(generic).copied())
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn complete_instance_args_for_def(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        substitutions: &std::collections::HashMap<String, InternedTyId>,
    ) -> Option<Vec<InternedTyId>> {
        let generics = self.effective_generics_for_def(def_id);
        self.complete_instance_args_for_generics(span, &generics, substitutions)
    }

    pub(crate) fn record_generic_instantiation(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        span: Span,
    ) {
        let instantiation = GenericInstantiation {
            def_id,
            args: args.to_vec(),
            generics: self.effective_generics_for_def(def_id),
            span,
            source_def_id: self.current_def_id,
        };
        self.generic_instantiations.push(instantiation.clone());
        if let Some(facts) = self.current_function_facts() {
            facts.generic_instantiations.push(instantiation);
        }
    }

    pub(crate) fn effective_generics_for_def(&self, def_id: GlobalDefId) -> Vec<String> {
        if let Some(generics) = self.trait_method_effective_generics(def_id) {
            return generics;
        }
        let mut generics = self.extension_method_effective_generics(def_id);
        if def_id.module_id == self.defs.module_id {
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

    fn trait_method_effective_generics(&self, def_id: GlobalDefId) -> Option<Vec<String>> {
        if def_id.module_id == self.defs.module_id
            && let Some(def) = self.defs.defs.get(def_id.def_id)
            && def.kind == nia_defs::DefKind::TraitMethod
        {
            let mut generics = vec!["Self".to_string()];
            generics.extend(
                def.parent
                    .and_then(|parent| self.defs.defs.get(parent))
                    .map(|parent| parent.generics.clone())
                    .unwrap_or_default(),
            );
            generics.extend(def.generics.clone());
            return Some(generics);
        }
        self.program_traits
            .iter()
            .find_map(|(trait_id, signature)| {
                signature
                    .signature
                    .methods
                    .iter()
                    .find(|method| {
                        GlobalDefId {
                            module_id: trait_id.module_id,
                            def_id: method.def_id,
                        } == def_id
                    })
                    .map(|method| {
                        let mut generics = vec!["Self".to_string()];
                        generics.extend(signature.signature.generics.iter().cloned());
                        generics.extend(method.signature.generics.iter().cloned());
                        generics
                    })
            })
    }

    fn extension_method_effective_generics(&self, def_id: GlobalDefId) -> Vec<String> {
        self.extensions
            .targets()
            .iter()
            .flat_map(|target| target.methods.iter())
            .find(|method| method.def_id == def_id)
            .map(|method| method.impl_generics.clone())
            .or_else(|| {
                self.program_extension_methods
                    .all_methods()
                    .find(|method| method.def_id == def_id)
                    .map(|method| method.impl_generics.clone())
            })
            .unwrap_or_default()
    }
}
