// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ids::{GlobalDefId, InternedTyId, TraitId};
use nia_item_signatures::{FunctionSignature, ParamSignature, TraitImplSignature};
use nia_symbol::{SymbolId, SymbolMap};
use nia_ty::{ArrayLenTy, ConstExprSummary, TyInterner, TyKind, TypeEquivalence, import_type_into};
use nia_type_lower::TypeLowering;

use super::ExtensionModuleInput;

pub(super) fn types_equivalent(
    lowering: &TypeLowering,
    left: nia_ids::InternedTyId,
    right: nia_ids::InternedTyId,
) -> bool {
    types_equivalent_with_const_exprs(
        &lowering.interner,
        &lowering.const_expr_summaries,
        left,
        right,
    )
}

pub(super) fn types_equivalent_in_interner(
    interner: &TyInterner,
    left: nia_ids::InternedTyId,
    right: nia_ids::InternedTyId,
) -> bool {
    types_equivalent_with_const_exprs(interner, &HashMap::new(), left, right)
}

fn types_equivalent_with_const_exprs(
    interner: &TyInterner,
    const_exprs: &HashMap<nia_ids::GlobalConstExprId, ConstExprSummary>,
    left: nia_ids::InternedTyId,
    right: nia_ids::InternedTyId,
) -> bool {
    if left == right {
        return true;
    }
    SignatureTypeEquivalence {
        interner,
        const_exprs,
    }
    .compute_same_type_for_equiv(left, right)
}

struct SignatureTypeEquivalence<'a> {
    interner: &'a TyInterner,
    const_exprs: &'a HashMap<nia_ids::GlobalConstExprId, ConstExprSummary>,
}

impl TypeEquivalence for SignatureTypeEquivalence<'_> {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.interner.get(ty)
    }

    fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
        if left == right {
            return true;
        }
        match (left, right) {
            (ArrayLenTy::Infer, ArrayLenTy::Infer) => true,
            (ArrayLenTy::ConstValue(left), ArrayLenTy::ConstValue(right)) => left == right,
            (
                ArrayLenTy::Builtin {
                    builtin: left_builtin,
                    ty: left_ty,
                },
                ArrayLenTy::Builtin {
                    builtin: right_builtin,
                    ty: right_ty,
                },
            ) => left_builtin == right_builtin && self.same_type_for_equiv(*left_ty, *right_ty),
            _ => self
                .literal_array_len_value(left)
                .zip(self.literal_array_len_value(right))
                .is_some_and(|(left, right)| left == right),
        }
    }

    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        types_equivalent_with_const_exprs(self.interner, self.const_exprs, left, right)
    }
}

impl SignatureTypeEquivalence<'_> {
    fn literal_array_len_value(&self, len: &ArrayLenTy) -> Option<u64> {
        match len {
            ArrayLenTy::ConstValue(value) => Some(*value),
            ArrayLenTy::ConstExpr(id) => self
                .const_exprs
                .get(id)
                .and_then(|summary| summary.literal_array_len),
            _ => None,
        }
    }
}

pub(super) fn lower_trait_method_signature(input: TraitMethodImport<'_>) -> FunctionSignature {
    let substitutions = input
        .trait_generics
        .iter()
        .zip(input.trait_args)
        .map(|(generic, arg)| (*generic, *arg))
        .collect::<SymbolMap<_>>();
    let const_substitutions = const_substitutions_from_self_describing_args(input.trait_const_args);
    let target = TypeSubstitutionTarget {
        projection: Some(ProjectionImplContext {
            trait_id: input.trait_id,
            trait_args: input.trait_args,
            trait_const_args: input.trait_const_args,
            self_ty: input.self_ty,
            associated_types: &input.impl_signature.associated_types,
        }),
        self_ty: Some(input.self_ty),
    };
    let mut signature = input.signature.clone();
    signature.params = signature
        .params
        .iter()
        .map(|param| ParamSignature {
            name: param.name,
            receiver: param.receiver,
            ty: substitute_imported_type(
                input.target_interner,
                input.module,
                input.source_interner,
                param.ty,
                &substitutions,
                &const_substitutions,
                target,
            ),
            span: param.span,
        })
        .collect();
    signature.return_type = substitute_imported_type(
        input.target_interner,
        input.module,
        input.source_interner,
        signature.return_type,
        &substitutions,
        &const_substitutions,
        target,
    );
    signature
}

pub(super) fn normalize_impl_method_signature(
    input: ImplMethodSignatureNormalize<'_>,
) -> FunctionSignature {
    let substitutions = SymbolMap::default();
    let const_substitutions = SymbolMap::default();
    let target = TypeSubstitutionTarget {
        projection: Some(ProjectionImplContext {
            trait_id: input.trait_id,
            trait_args: input.trait_args,
            trait_const_args: input.trait_const_args,
            self_ty: input.self_ty,
            associated_types: &input.impl_signature.associated_types,
        }),
        self_ty: Some(input.self_ty),
    };
    let mut signature = input.signature.clone();
    signature.params = signature
        .params
        .iter()
        .map(|param| ParamSignature {
            name: param.name,
            receiver: param.receiver,
            ty: substitute_imported_type(
                input.target_interner,
                input.module,
                input.source_interner,
                param.ty,
                &substitutions,
                &const_substitutions,
                target,
            ),
            span: param.span,
        })
        .collect();
    signature.return_type = substitute_imported_type(
        input.target_interner,
        input.module,
        input.source_interner,
        signature.return_type,
        &substitutions,
        &const_substitutions,
        target,
    );
    signature
}

pub(super) struct TraitMethodImport<'a> {
    pub(super) target_interner: &'a mut TyInterner,
    pub(super) module: &'a ExtensionModuleInput<'a>,
    pub(super) source_interner: &'a TyInterner,
    pub(super) signature: &'a FunctionSignature,
    // Required trait methods are authored in the trait module/interner but are
    // checked against an impl in the current module/interner. These fields keep
    // the substitution environment and projection-impl context in one place.
    pub(super) trait_generics: &'a [SymbolId],
    pub(super) trait_args: &'a [nia_ids::InternedTyId],
    pub(super) trait_const_args: &'a [nia_ty::ConstGenericArg],
    pub(super) self_ty: nia_ids::InternedTyId,
    pub(super) trait_id: GlobalDefId,
    pub(super) impl_signature: &'a TraitImplSignature,
}

pub(super) struct ImplMethodSignatureNormalize<'a> {
    pub(super) target_interner: &'a mut TyInterner,
    pub(super) module: &'a ExtensionModuleInput<'a>,
    pub(super) source_interner: &'a TyInterner,
    pub(super) signature: &'a FunctionSignature,
    pub(super) trait_args: &'a [nia_ids::InternedTyId],
    pub(super) trait_const_args: &'a [nia_ty::ConstGenericArg],
    pub(super) self_ty: nia_ids::InternedTyId,
    pub(super) trait_id: GlobalDefId,
    pub(super) impl_signature: &'a TraitImplSignature,
}

#[derive(Clone, Copy)]
pub(super) struct ProjectionImplContext<'a> {
    pub(super) trait_id: GlobalDefId,
    pub(super) trait_args: &'a [nia_ids::InternedTyId],
    pub(super) trait_const_args: &'a [nia_ty::ConstGenericArg],
    pub(super) self_ty: nia_ids::InternedTyId,
    pub(super) associated_types: &'a [nia_item_signatures::TraitImplAssociatedTypeSignature],
}

#[derive(Clone, Copy, Default)]
pub(super) struct TypeSubstitutionTarget<'a> {
    pub(super) projection: Option<ProjectionImplContext<'a>>,
    pub(super) self_ty: Option<nia_ids::InternedTyId>,
}

pub(super) fn substitute_imported_type(
    target_interner: &mut TyInterner,
    module: &ExtensionModuleInput<'_>,
    source_interner: &TyInterner,
    ty: nia_ids::InternedTyId,
    substitutions: &SymbolMap<nia_ids::InternedTyId>,
    const_substitutions: &SymbolMap<nia_ty::ConstGenericArg>,
    target: TypeSubstitutionTarget<'_>,
) -> nia_ids::InternedTyId {
    let TypeSubstitutionTarget {
        projection: projection_context,
        self_ty: self_substitution,
    } = target;
    match source_interner.get(ty) {
        Some(TyKind::GenericParam(name)) => substitutions
            .get(name)
            .copied()
            .unwrap_or_else(|| import_type_into(target_interner, source_interner, ty)),
        Some(TyKind::SelfParam) => self_substitution
            .unwrap_or_else(|| import_type_into(target_interner, source_interner, ty)),
        Some(TyKind::Pointer { is_readonly, elem }) => {
            let is_readonly = *is_readonly;
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            target_interner.intern(TyKind::Pointer { is_readonly, elem })
        }
        Some(TyKind::VolatilePointer { is_readonly, elem }) => {
            let is_readonly = *is_readonly;
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            target_interner.intern(TyKind::VolatilePointer { is_readonly, elem })
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let is_readonly = *is_readonly;
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            target_interner.intern(TyKind::Slice { is_readonly, elem })
        }
        Some(TyKind::SlicePointee { elem }) => {
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            target_interner.intern(TyKind::SlicePointee { elem })
        }
        Some(TyKind::Array { len, elem }) => {
            let len = substitute_imported_array_len(len.clone(), const_substitutions);
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            target_interner.intern(TyKind::Array { len, elem })
        }
        Some(TyKind::Range { kind, bound }) => {
            let bound = bound.map(|bound| {
                substitute_imported_type(
                    target_interner,
                    module,
                    source_interner,
                    bound,
                    substitutions,
                    const_substitutions,
                    TypeSubstitutionTarget {
                        projection: projection_context,
                        self_ty: self_substitution,
                    },
                )
            });
            target_interner.intern(TyKind::Range { kind: *kind, bound })
        }
        Some(TyKind::Optional { elem }) => {
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            target_interner.intern(TyKind::Optional { elem })
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            let error = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *error,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            let value = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *value,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            target_interner.intern(TyKind::ErrorUnion { error, value })
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => {
            let params = params
                .iter()
                .map(|param| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *param,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    )
                })
                .collect();
            let return_type = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *return_type,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            target_interner.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: *is_variadic,
            })
        }
        Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) => {
            let args = args
                .iter()
                .map(|arg| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *arg,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    )
                })
                .collect();
            let const_args = const_args
                .iter()
                .map(|arg| {
                    substitute_imported_const_arg(
                        target_interner,
                        module,
                        source_interner,
                        arg,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    )
                })
                .collect();
            target_interner.intern(TyKind::Nominal {
                def_id: *def_id,
                args,
                const_args,
            })
        }
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            let args = args
                .iter()
                .map(|arg| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *arg,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    )
                })
                .collect();
            target_interner.intern(TyKind::BuiltinTrait {
                trait_id: *trait_id,
                args,
            })
        }
        Some(TyKind::BuiltinType(builtin)) => target_interner.intern(TyKind::BuiltinType(*builtin)),
        Some(TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => {
            let trait_args = trait_args
                .iter()
                .map(|arg| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *arg,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    )
                })
                .collect();
            let trait_const_args = trait_const_args
                .iter()
                .map(|arg| {
                    substitute_imported_const_arg(
                        target_interner,
                        module,
                        source_interner,
                        arg,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    )
                })
                .collect();
            let associated_type_bindings = associated_type_bindings
                .iter()
                .map(|binding| nia_ty::AssociatedTypeBindingTy {
                    trait_id: binding.trait_id,
                    trait_args: binding
                        .trait_args
                        .iter()
                        .map(|arg| {
                            substitute_imported_type(
                                target_interner,
                                module,
                                source_interner,
                                *arg,
                                substitutions,
                                const_substitutions,
                                TypeSubstitutionTarget {
                                    projection: projection_context,
                                    self_ty: self_substitution,
                                },
                            )
                        })
                        .collect(),
                    trait_const_args: binding
                        .trait_const_args
                        .iter()
                        .map(|arg| {
                            substitute_imported_const_arg(
                                target_interner,
                                module,
                                source_interner,
                                arg,
                                substitutions,
                                const_substitutions,
                                TypeSubstitutionTarget {
                                    projection: projection_context,
                                    self_ty: self_substitution,
                                },
                            )
                        })
                        .collect(),
                    name: binding.name,
                    ty: substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        binding.ty,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    ),
                })
                .collect();
            target_interner.intern(TyKind::TraitObject {
                is_readonly: *is_readonly,
                trait_id: *trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            })
        }
        Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => {
            let trait_args = trait_args
                .iter()
                .map(|arg| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *arg,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    )
                })
                .collect();
            let trait_const_args = trait_const_args
                .iter()
                .map(|arg| {
                    substitute_imported_const_arg(
                        target_interner,
                        module,
                        source_interner,
                        arg,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    )
                })
                .collect();
            let associated_type_bindings = associated_type_bindings
                .iter()
                .map(|binding| nia_ty::AssociatedTypeBindingTy {
                    trait_id: binding.trait_id,
                    trait_args: binding
                        .trait_args
                        .iter()
                        .map(|arg| {
                            substitute_imported_type(
                                target_interner,
                                module,
                                source_interner,
                                *arg,
                                substitutions,
                                const_substitutions,
                                TypeSubstitutionTarget {
                                    projection: projection_context,
                                    self_ty: self_substitution,
                                },
                            )
                        })
                        .collect(),
                    trait_const_args: binding
                        .trait_const_args
                        .iter()
                        .map(|arg| {
                            substitute_imported_const_arg(
                                target_interner,
                                module,
                                source_interner,
                                arg,
                                substitutions,
                                const_substitutions,
                                TypeSubstitutionTarget {
                                    projection: projection_context,
                                    self_ty: self_substitution,
                                },
                            )
                        })
                        .collect(),
                    name: binding.name,
                    ty: substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        binding.ty,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    ),
                })
                .collect();
            target_interner.intern(TyKind::TraitObjectPointee {
                trait_id: *trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            })
        }
        Some(TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        }) => {
            let self_ty = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *self_ty,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            let trait_args = trait_args
                .iter()
                .map(|arg| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *arg,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    )
                })
                .collect::<Vec<_>>();
            let trait_const_args = trait_const_args
                .iter()
                .map(|arg| {
                    substitute_imported_const_arg(
                        target_interner,
                        module,
                        source_interner,
                        arg,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    )
                })
                .collect::<Vec<_>>();
            if let Some(context) = projection_context
                && *trait_id == TraitId::Source(context.trait_id)
                && self_ty == context.self_ty
                && trait_args == context.trait_args
                && trait_const_args == context.trait_const_args
                && let Some(associated_type) = context
                    .associated_types
                    .iter()
                    .find(|associated_type| associated_type.name == *name)
            {
                let ty = module.normalization.normalize(associated_type.ty);
                return import_type_into(target_interner, &module.normalization.interner, ty);
            }
            target_interner.intern(TyKind::Projection {
                self_ty,
                trait_id: *trait_id,
                trait_args,
                trait_const_args,
                name: *name,
            })
        }
        Some(
            TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_) | TyKind::Vector { .. },
        )
        | None => import_type_into(target_interner, source_interner, ty),
    }
}

pub(super) fn const_substitutions_from_self_describing_args(
    const_args: &[nia_ty::ConstGenericArg],
) -> SymbolMap<nia_ty::ConstGenericArg> {
    const_args
        .iter()
        .filter_map(|arg| match &arg.value {
            nia_ty::ConstGenericValue::GenericParam(name) => Some((*name, arg.clone())),
            _ => None,
        })
        .collect()
}

fn substitute_imported_const_arg(
    target_interner: &mut TyInterner,
    module: &ExtensionModuleInput<'_>,
    source_interner: &TyInterner,
    arg: &nia_ty::ConstGenericArg,
    substitutions: &SymbolMap<nia_ids::InternedTyId>,
    const_substitutions: &SymbolMap<nia_ty::ConstGenericArg>,
    target: TypeSubstitutionTarget<'_>,
) -> nia_ty::ConstGenericArg {
    if let nia_ty::ConstGenericValue::GenericParam(name) = &arg.value
        && let Some(substituted) = const_substitutions.get(name)
    {
        return substituted.clone();
    }
    nia_ty::ConstGenericArg {
        ty: substitute_imported_type(
            target_interner,
            module,
            source_interner,
            arg.ty,
            substitutions,
            const_substitutions,
            target,
        ),
        value: arg.value.clone(),
    }
}

fn substitute_imported_array_len(
    len: nia_ty::ArrayLenTy,
    const_substitutions: &SymbolMap<nia_ty::ConstGenericArg>,
) -> nia_ty::ArrayLenTy {
    match len {
        nia_ty::ArrayLenTy::GenericParam(name) => const_substitutions
            .get(&name)
            .and_then(array_len_from_const_arg)
            .unwrap_or(nia_ty::ArrayLenTy::GenericParam(name)),
        len => len,
    }
}

fn array_len_from_const_arg(arg: &nia_ty::ConstGenericArg) -> Option<nia_ty::ArrayLenTy> {
    match &arg.value {
        nia_ty::ConstGenericValue::Int(value) => value
            .bits()
            .try_into()
            .ok()
            .map(nia_ty::ArrayLenTy::ConstValue),
        nia_ty::ConstGenericValue::GenericParam(name) => {
            Some(nia_ty::ArrayLenTy::GenericParam(*name))
        }
        nia_ty::ConstGenericValue::ConstExpr(id) => Some(nia_ty::ArrayLenTy::ConstExpr(*id)),
        nia_ty::ConstGenericValue::Bool(_) | nia_ty::ConstGenericValue::Char(_) => None,
    }
}
