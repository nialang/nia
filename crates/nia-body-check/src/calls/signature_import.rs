// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    BodyChecker, ProgramFunctionSignature, ProgramTraitSignature, ResolvedFunctionSignature,
};
use nia_ast::Expr;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_item_signatures::{
    AssociatedTypeBindingSignature, FunctionSignature, TraitSignature, WhereBoundSignature,
    WherePredicateSignature,
};
use nia_ty::{TyInterner, import_type_into};

impl<'a> BodyChecker<'a> {
    pub(crate) fn qualified_callee_signature(
        &mut self,
        callee: &Expr,
    ) -> Option<ResolvedFunctionSignature> {
        let def_id = self.qualified_value(callee)?;
        let program_signature = self.function_signature_scope.program_signature(&def_id)?;
        Some(ResolvedFunctionSignature {
            def_id,
            signature: self.import_program_function_signature(&program_signature),
        })
    }

    pub(crate) fn resolved_function_signature(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<ResolvedFunctionSignature> {
        if def_id.module_id == self.defs.module_id {
            if let Some(program_signature) =
                self.function_signature_scope.program_signature(&def_id)
            {
                return Some(ResolvedFunctionSignature {
                    def_id,
                    signature: self.import_program_function_signature(&program_signature),
                });
            }
            let signature = self.signatures.functions.get(&def_id.def_id)?.clone();
            return Some(ResolvedFunctionSignature {
                def_id,
                signature: self.import_local_function_signature(&signature),
            });
        }
        let program_signature = self.function_signature_scope.program_signature(&def_id)?;
        Some(ResolvedFunctionSignature {
            def_id,
            signature: self.import_program_function_signature(&program_signature),
        })
    }

    pub(crate) fn import_local_function_signature(
        &mut self,
        signature: &FunctionSignature,
    ) -> FunctionSignature {
        let source = self.type_lowering.interner.clone();
        let signature = self.import_function_signature_from(&source, signature);
        self.normalize_function_signature_aliases(signature)
    }

    pub(crate) fn import_program_function_signature(
        &mut self,
        program_signature: &ProgramFunctionSignature,
    ) -> FunctionSignature {
        let mut signature = program_signature.signature.clone();
        signature.where_predicates = self
            .import_where_predicates_from(&program_signature.interner, &signature.where_predicates);
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
        self.normalize_function_signature_aliases(signature)
    }

    pub(crate) fn resolved_trait_signature(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<TraitSignature> {
        if def_id.module_id == self.defs.module_id {
            let signature = self.signatures.traits.get(&def_id.def_id)?.clone();
            return Some(signature);
        }
        let program_signature = self.program_traits.get(&def_id)?.clone();
        Some(self.import_program_trait_signature(&program_signature))
    }

    pub(crate) fn import_program_trait_signature(
        &mut self,
        program_signature: &ProgramTraitSignature,
    ) -> TraitSignature {
        let mut signature = program_signature.signature.clone();
        signature.where_predicates = self
            .import_where_predicates_from(&program_signature.interner, &signature.where_predicates);
        signature.supertraits = signature
            .supertraits
            .iter()
            .map(|supertrait| nia_item_signatures::TraitSupertraitSignature {
                ty: self.import_type_from(&program_signature.interner, supertrait.ty),
                span: supertrait.span,
            })
            .collect();
        signature.associated_values = signature
            .associated_values
            .iter()
            .map(
                |associated_value| nia_item_signatures::TraitAssociatedValueSignature {
                    def_id: associated_value.def_id,
                    name: associated_value.name.clone(),
                    ty: self.import_type_from(&program_signature.interner, associated_value.ty),
                    span: associated_value.span,
                },
            )
            .collect();
        signature.methods = signature
            .methods
            .iter()
            .map(|method| {
                let mut method = method.clone();
                method.signature = self
                    .import_function_signature_from(&program_signature.interner, &method.signature);
                method
            })
            .collect();
        signature
    }

    pub(crate) fn import_function_signature_from(
        &mut self,
        source: &TyInterner,
        signature: &FunctionSignature,
    ) -> FunctionSignature {
        let mut signature = signature.clone();
        signature.where_predicates =
            self.import_where_predicates_from(source, &signature.where_predicates);
        signature.params = signature
            .params
            .iter()
            .map(|param| {
                let mut param = param.clone();
                param.ty = self.import_type_from(source, param.ty);
                param
            })
            .collect();
        signature.return_type = self.import_type_from(source, signature.return_type);
        signature
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

    pub(crate) fn import_where_predicates_from(
        &mut self,
        source: &TyInterner,
        predicates: &[WherePredicateSignature],
    ) -> Vec<WherePredicateSignature> {
        predicates
            .iter()
            .map(|predicate| WherePredicateSignature {
                ty: self.import_type_from(source, predicate.ty),
                bounds: predicate
                    .bounds
                    .iter()
                    .map(|bound| WhereBoundSignature {
                        trait_ty: self.import_type_from(source, bound.trait_ty),
                        associated_type_bindings: bound
                            .associated_type_bindings
                            .iter()
                            .map(|binding| AssociatedTypeBindingSignature {
                                name: binding.name.clone(),
                                ty: self.import_type_from(source, binding.ty),
                                span: binding.span,
                            })
                            .collect(),
                        span: bound.span,
                    })
                    .collect(),
                span: predicate.span,
            })
            .collect()
    }

    pub(crate) fn import_type_from(
        &mut self,
        source: &TyInterner,
        ty: InternedTyId,
    ) -> InternedTyId {
        import_type_into(&mut self.interner, source, ty)
    }
}
