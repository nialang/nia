// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{BodyChecker, ProgramFunctionSignature, ResolvedFunctionSignature};
use nia_ast::Expr;
use nia_ids::GlobalDefId;
use nia_item_signatures::{
    AssociatedTypeBindingSignature, FunctionSignature, TraitSignature, WhereBoundSignature,
    WherePredicateSignature,
};

impl<'a> BodyChecker<'a> {
    pub(crate) fn qualified_callee_signature(
        &mut self,
        callee: &Expr,
    ) -> Option<ResolvedFunctionSignature> {
        let def_id = self.qualified_value(callee)?;
        let program_signature = self.program_signature_scope.function(def_id)?;
        Some(ResolvedFunctionSignature {
            def_id,
            signature: self.program_function_signature(&program_signature),
        })
    }

    pub(crate) fn resolved_function_signature(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<ResolvedFunctionSignature> {
        if def_id.module_id == self.defs.module_id {
            if let Some(program_signature) = self.program_signature_scope.function(def_id) {
                return Some(ResolvedFunctionSignature {
                    def_id,
                    signature: self.program_function_signature(&program_signature),
                });
            }
            let signature = self.signatures.functions.get(&def_id.def_id)?.clone();
            return Some(ResolvedFunctionSignature {
                def_id,
                signature: self.local_function_signature(&signature),
            });
        }
        let program_signature = self.program_signature_scope.function(def_id)?;
        Some(ResolvedFunctionSignature {
            def_id,
            signature: self.program_function_signature(&program_signature),
        })
    }

    pub(crate) fn local_function_signature(
        &mut self,
        signature: &FunctionSignature,
    ) -> FunctionSignature {
        self.normalize_function_signature_aliases(signature.clone())
    }

    pub(crate) fn program_function_signature(
        &mut self,
        program_signature: &ProgramFunctionSignature,
    ) -> FunctionSignature {
        self.normalize_function_signature_aliases(program_signature.signature.clone())
    }

    pub(crate) fn resolved_trait_signature(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<TraitSignature> {
        if def_id.module_id == self.defs.module_id {
            let signature = self.signatures.traits.get(&def_id.def_id)?.clone();
            return Some(signature);
        }
        let program_signature = self.program_signature_scope.trait_(def_id)?;
        Some(program_signature.signature.clone())
    }

    fn normalize_function_signature_aliases(
        &mut self,
        mut signature: FunctionSignature,
    ) -> FunctionSignature {
        signature.where_predicates = signature
            .where_predicates
            .into_iter()
            .map(|predicate| WherePredicateSignature {
                ty: self.normalize_aliases_in_type(predicate.ty),
                bounds: predicate
                    .bounds
                    .into_iter()
                    .map(|bound| WhereBoundSignature {
                        trait_ty: self.normalize_aliases_in_type(bound.trait_ty),
                        associated_type_bindings: bound
                            .associated_type_bindings
                            .into_iter()
                            .map(|binding| AssociatedTypeBindingSignature {
                                name: binding.name,
                                ty: self.normalize_aliases_in_type(binding.ty),
                                span: binding.span,
                            })
                            .collect(),
                        span: bound.span,
                    })
                    .collect(),
                span: predicate.span,
            })
            .collect();
        signature.params = signature
            .params
            .into_iter()
            .map(|mut param| {
                param.ty = self.normalize_aliases_in_type(param.ty);
                param
            })
            .collect();
        signature.return_type = self.normalize_aliases_in_type(signature.return_type);
        signature
    }
}
