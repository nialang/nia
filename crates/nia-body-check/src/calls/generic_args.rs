// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{BracketArg, ExprKind, TypeKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_item_signatures::{GenericParamSignature, GenericParamSignatureKind};
use nia_sema_ir::GenericInstantiation;
use nia_span::Span;
use nia_ty::{ConstGenericArg, ConstGenericValue, IntConst, TyKind};

#[derive(Debug, Clone, Default)]
pub(crate) struct LoweredGenericArgs {
    pub(crate) type_args: Vec<InternedTyId>,
    pub(crate) const_args: Vec<ConstGenericArg>,
    pub(crate) type_substitutions: std::collections::HashMap<String, InternedTyId>,
    pub(crate) const_substitutions: std::collections::HashMap<String, ConstGenericArg>,
}

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

    pub(super) fn lower_bracket_args_for_generic_params(
        &mut self,
        span: Span,
        params: &[GenericParamSignature],
        args: &[BracketArg],
    ) -> Option<LoweredGenericArgs> {
        if args.len() > params.len() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "generic argument count mismatch: expected {}, got {}",
                    params.len(),
                    args.len()
                ),
            ));
            return None;
        }
        let mut lowered = LoweredGenericArgs::default();
        for (param, arg) in params.iter().zip(args) {
            match &param.kind {
                GenericParamSignatureKind::Type => {
                    if let Some(ty) = &arg.ty {
                        let ty = self.ty_for_type(ty);
                        lowered.type_args.push(ty);
                        lowered.type_substitutions.insert(param.name.clone(), ty);
                    } else {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            arg.span,
                            format!("generic argument `{}` must be a type", param.name),
                        ));
                        return None;
                    }
                }
                GenericParamSignatureKind::Comptime { ty } => {
                    let Some(value) = self.const_generic_value_from_bracket_arg(arg) else {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            arg.span,
                            format!("generic argument `{}` must be a comptime value", param.name),
                        ));
                        return None;
                    };
                    let arg = ConstGenericArg { ty: *ty, value };
                    lowered.const_args.push(arg.clone());
                    lowered.const_substitutions.insert(param.name.clone(), arg);
                }
            }
        }
        Some(lowered)
    }

    fn const_generic_value_from_bracket_arg(&self, arg: &BracketArg) -> Option<ConstGenericValue> {
        if let Some(expr) = &arg.expr {
            return match &expr.kind {
                ExprKind::Integer(text) => nia_literals::eval_int_literal(text)
                    .ok()
                    .map(|value| ConstGenericValue::Int(IntConst::signed(value))),
                ExprKind::Bool(value) => Some(ConstGenericValue::Bool(*value)),
                ExprKind::Ident(name) => Some(ConstGenericValue::GenericParam(name.clone())),
                _ => None,
            };
        }
        let ty = arg.ty.as_ref()?;
        let TypeKind::Path { segments } = &ty.kind else {
            return None;
        };
        if segments.len() != 1 || !segments[0].args.is_empty() {
            return None;
        }
        match segments[0].name.as_str() {
            "true" => Some(ConstGenericValue::Bool(true)),
            "false" => Some(ConstGenericValue::Bool(false)),
            name => Some(ConstGenericValue::GenericParam(name.to_string())),
        }
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

    pub(crate) fn complete_const_instance_args_for_generic_params(
        &mut self,
        span: Span,
        generic_params: &[GenericParamSignature],
        substitutions: &std::collections::HashMap<String, ConstGenericArg>,
    ) -> Option<Vec<ConstGenericArg>> {
        let mut complete = true;
        let mut args = Vec::new();
        for generic in generic_params {
            if !matches!(generic.kind, GenericParamSignatureKind::Comptime { .. }) {
                continue;
            }
            if let Some(arg) = substitutions.get(&generic.name) {
                args.push(arg.clone());
            } else {
                complete = false;
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("cannot infer comptime generic parameter `{}`", generic.name),
                ));
            }
        }
        complete.then_some(args)
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

    pub(crate) fn complete_instance_args_and_const_args_for_def(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        substitutions: &std::collections::HashMap<String, InternedTyId>,
        const_substitutions: &std::collections::HashMap<String, ConstGenericArg>,
    ) -> Option<(Vec<InternedTyId>, Vec<ConstGenericArg>)> {
        let mut complete = true;
        let mut args = Vec::new();
        let mut const_args = Vec::new();
        for generic in self.effective_generics_for_def(def_id) {
            if let Some(arg) = substitutions.get(&generic).copied() {
                args.push(arg);
            } else if let Some(arg) = const_substitutions.get(&generic).cloned() {
                const_args.push(arg);
            } else {
                complete = false;
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("cannot infer generic parameter `{generic}`"),
                ));
            }
        }
        complete.then_some((args, const_args))
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
            const_args: Vec::new(),
            generics: self.effective_generics_for_def(def_id),
            span,
            source_def_id: self.current_def_id,
        };
        self.generic_instantiations.push(instantiation.clone());
        if let Some(facts) = self.current_function_facts() {
            facts.generic_instantiations.push(instantiation);
        }
    }

    pub(crate) fn record_generic_instantiation_with_const_args(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
        span: Span,
    ) {
        let instantiation = GenericInstantiation {
            def_id,
            args: args.to_vec(),
            const_args: const_args.to_vec(),
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
            if generics.is_empty()
                && let Some(def) = self.defs.defs.get(def_id.def_id)
            {
                generics = def
                    .parent
                    .and_then(|parent| self.defs.defs.get(parent))
                    .map(|parent| parent.generics.clone())
                    .unwrap_or_default();
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
            .map(|method| method.effective_generics.clone())
            .or_else(|| {
                self.program_extension_methods
                    .all_methods()
                    .find(|method| method.def_id == def_id)
                    .map(|method| method.effective_generics.clone())
            })
            .unwrap_or_default()
    }
}
