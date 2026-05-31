// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    BodyChecker, ProgramFunctionSignature, ProgramTraitSignature, ResolvedFunctionSignature,
};
use nia_ast::Expr;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_item_signatures::{
    AssociatedTypeBindingSignature, FunctionSignature, TraitSignature, WhereBoundSignature,
};
use nia_ty::{TyInterner, import_type_into};

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

    pub(crate) fn resolved_function_signature(
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
        signature.where_predicates = signature
            .where_predicates
            .iter()
            .map(|predicate| nia_item_signatures::WherePredicateSignature {
                ty: self.import_type_from(&program_signature.interner, predicate.ty),
                bounds: predicate
                    .bounds
                    .iter()
                    .map(|bound| WhereBoundSignature {
                        trait_ty: self
                            .import_type_from(&program_signature.interner, bound.trait_ty),
                        associated_type_bindings: bound
                            .associated_type_bindings
                            .iter()
                            .map(|binding| AssociatedTypeBindingSignature {
                                name: binding.name.clone(),
                                ty: self.import_type_from(&program_signature.interner, binding.ty),
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
        signature.supertraits = signature
            .supertraits
            .iter()
            .map(|supertrait| self.import_type_from(&program_signature.interner, *supertrait))
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
        signature.where_predicates = signature
            .where_predicates
            .iter()
            .map(|predicate| nia_item_signatures::WherePredicateSignature {
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
            .collect();
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

    pub(crate) fn import_type_from(
        &mut self,
        source: &TyInterner,
        ty: InternedTyId,
    ) -> InternedTyId {
        import_type_into(&mut self.interner, source, ty)
    }
}
