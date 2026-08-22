// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ids::{GlobalDefId, InternedTyId, TraitId};
use nia_item_signatures::{FunctionSignature, ParamSignature, TraitImplSignature};
use nia_symbol::SymbolMap;
use nia_ty::{ArrayLenTy, ConstExprSummary, TyKind, TypeEquivalence, TypeStore, TypeStoreAppend};
use nia_type_lower::TypeLowering;

use super::ExtensionModuleInput;

pub(super) fn types_equivalent(
    type_store: &TypeStore,
    lowering: &TypeLowering,
    left: nia_ids::InternedTyId,
    right: nia_ids::InternedTyId,
) -> bool {
    types_equivalent_with_const_exprs(type_store, &lowering.const_expr_summaries, left, right)
}

pub(super) fn types_equivalent_in_store(
    type_store: &TypeStore,
    left: nia_ids::InternedTyId,
    right: nia_ids::InternedTyId,
) -> bool {
    types_equivalent_with_const_exprs(type_store, &HashMap::new(), left, right)
}

pub(super) fn type_args_equivalent_in_store(
    type_store: &TypeStore,
    left: &[InternedTyId],
    right: &[InternedTyId],
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| types_equivalent_in_store(type_store, *left, *right))
}

pub(super) fn const_args_equivalent_in_store(
    type_store: &TypeStore,
    left: &[nia_ty::ConstGenericArg],
    right: &[nia_ty::ConstGenericArg],
) -> bool {
    SignatureTypeEquivalence {
        type_store,
        const_exprs: &HashMap::new(),
    }
    .same_const_generic_args_for_equiv(left, right)
}

pub(super) fn projection_context_matches(
    type_store: &TypeStore,
    self_ty: InternedTyId,
    context_self_ty: InternedTyId,
    trait_args: &[InternedTyId],
    context_trait_args: &[InternedTyId],
    trait_const_args: &[nia_ty::ConstGenericArg],
    context_trait_const_args: &[nia_ty::ConstGenericArg],
) -> bool {
    types_equivalent_in_store(type_store, self_ty, context_self_ty)
        && type_args_equivalent_in_store(type_store, trait_args, context_trait_args)
        && const_args_equivalent_in_store(type_store, trait_const_args, context_trait_const_args)
}

fn types_equivalent_with_const_exprs(
    type_store: &TypeStore,
    const_exprs: &HashMap<nia_ids::GlobalConstExprId, ConstExprSummary>,
    left: nia_ids::InternedTyId,
    right: nia_ids::InternedTyId,
) -> bool {
    if left == right {
        return true;
    }
    SignatureTypeEquivalence {
        type_store,
        const_exprs,
    }
    .compute_same_type_for_equiv(left, right)
}

struct SignatureTypeEquivalence<'a> {
    type_store: &'a TypeStore,
    const_exprs: &'a HashMap<nia_ids::GlobalConstExprId, ConstExprSummary>,
}

impl TypeEquivalence for SignatureTypeEquivalence<'_> {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.type_store.get(ty)
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
        types_equivalent_with_const_exprs(self.type_store, self.const_exprs, left, right)
    }

    fn same_const_generic_args_for_equiv(
        &self,
        left: &[nia_ty::ConstGenericArg],
        right: &[nia_ty::ConstGenericArg],
    ) -> bool {
        left.len() == right.len()
            && left.iter().zip(right).all(|(left, right)| {
                self.same_type_for_equiv(left.ty, right.ty)
                    && match (&left.value, &right.value) {
                        (
                            nia_ty::ConstGenericValue::Int(left),
                            nia_ty::ConstGenericValue::Int(right),
                        ) => left.bits() == right.bits(),
                        (left, right) => left == right,
                    }
            })
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

pub(super) fn lower_trait_method_signature(
    input: TraitMethodSubstitution<'_>,
) -> FunctionSignature {
    let (substitutions, const_substitutions) = substitutions_from_generic_params(
        input.trait_generic_params,
        input.trait_args,
        input.trait_const_args,
    );
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
            ty: substitute_type(
                input.append,
                input.module,
                input.type_store,
                param.ty,
                &substitutions,
                &const_substitutions,
                target,
            ),
            span: param.span,
        })
        .collect();
    signature.return_type = substitute_type(
        input.append,
        input.module,
        input.type_store,
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
            ty: substitute_type(
                input.append,
                input.module,
                input.type_store,
                param.ty,
                &substitutions,
                &const_substitutions,
                target,
            ),
            span: param.span,
        })
        .collect();
    signature.return_type = substitute_type(
        input.append,
        input.module,
        input.type_store,
        signature.return_type,
        &substitutions,
        &const_substitutions,
        target,
    );
    signature
}

pub(super) struct TraitMethodSubstitution<'a> {
    pub(super) append: &'a TypeStoreAppend,
    pub(super) module: &'a ExtensionModuleInput<'a>,
    pub(super) type_store: &'a TypeStore,
    pub(super) signature: &'a FunctionSignature,
    // Required trait methods are authored in the trait module but are checked
    // against an impl in the current module. These fields keep
    // the substitution environment and projection-impl context in one place.
    pub(super) trait_generic_params: &'a [nia_item_signatures::GenericParamSignature],
    pub(super) trait_args: &'a [nia_ids::InternedTyId],
    pub(super) trait_const_args: &'a [nia_ty::ConstGenericArg],
    pub(super) self_ty: nia_ids::InternedTyId,
    pub(super) trait_id: GlobalDefId,
    pub(super) impl_signature: &'a TraitImplSignature,
}

pub(super) struct ImplMethodSignatureNormalize<'a> {
    pub(super) append: &'a TypeStoreAppend,
    pub(super) module: &'a ExtensionModuleInput<'a>,
    pub(super) type_store: &'a TypeStore,
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

pub(super) fn substitute_type(
    append: &TypeStoreAppend,
    module: &ExtensionModuleInput<'_>,
    type_store: &TypeStore,
    ty: nia_ids::InternedTyId,
    substitutions: &SymbolMap<nia_ids::InternedTyId>,
    const_substitutions: &SymbolMap<nia_ty::ConstGenericArg>,
    target: TypeSubstitutionTarget<'_>,
) -> nia_ids::InternedTyId {
    let TypeSubstitutionTarget {
        projection: projection_context,
        self_ty: self_substitution,
    } = target;
    match type_store.get(ty) {
        Some(TyKind::GenericParam(name)) => substitutions.get(name).copied().unwrap_or(ty),
        Some(TyKind::SelfParam) => self_substitution.unwrap_or(ty),
        Some(TyKind::Opaque) => ty,
        Some(TyKind::Tuple(elems)) => {
            let elems = elems
                .iter()
                .map(|elem| {
                    substitute_type(
                        append,
                        module,
                        type_store,
                        *elem,
                        substitutions,
                        const_substitutions,
                        TypeSubstitutionTarget {
                            projection: projection_context,
                            self_ty: self_substitution,
                        },
                    )
                })
                .collect();
            append.intern(TyKind::Tuple(elems))
        }
        Some(TyKind::ClosureState {
            closure_id,
            captures,
            params,
            return_type,
        }) => {
            let substitute = |ty| {
                substitute_type(
                    append,
                    module,
                    type_store,
                    ty,
                    substitutions,
                    const_substitutions,
                    TypeSubstitutionTarget {
                        projection: projection_context,
                        self_ty: self_substitution,
                    },
                )
            };
            append.intern(TyKind::ClosureState {
                closure_id: *closure_id,
                captures: captures.iter().copied().map(substitute).collect(),
                params: params.iter().copied().map(substitute).collect(),
                return_type: substitute(*return_type),
            })
        }
        Some(TyKind::Pointer { is_readonly, elem }) => {
            let is_readonly = *is_readonly;
            let elem = substitute_type(
                append,
                module,
                type_store,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            append.intern(TyKind::Pointer { is_readonly, elem })
        }
        Some(TyKind::VolatilePointer { is_readonly, elem }) => {
            let is_readonly = *is_readonly;
            let elem = substitute_type(
                append,
                module,
                type_store,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            append.intern(TyKind::VolatilePointer { is_readonly, elem })
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let is_readonly = *is_readonly;
            let elem = substitute_type(
                append,
                module,
                type_store,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            append.intern(TyKind::Slice { is_readonly, elem })
        }
        Some(TyKind::SlicePointee { elem }) => {
            let elem = substitute_type(
                append,
                module,
                type_store,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            append.intern(TyKind::SlicePointee { elem })
        }
        Some(TyKind::Array { len, elem }) => {
            let len = substitute_array_len(len.clone(), const_substitutions);
            let elem = substitute_type(
                append,
                module,
                type_store,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            append.intern(TyKind::Array { len, elem })
        }
        Some(TyKind::Range { kind, bound }) => {
            let bound = bound.map(|bound| {
                substitute_type(
                    append,
                    module,
                    type_store,
                    bound,
                    substitutions,
                    const_substitutions,
                    TypeSubstitutionTarget {
                        projection: projection_context,
                        self_ty: self_substitution,
                    },
                )
            });
            append.intern(TyKind::Range { kind: *kind, bound })
        }
        Some(TyKind::Optional { elem }) => {
            let elem = substitute_type(
                append,
                module,
                type_store,
                *elem,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            append.intern(TyKind::Optional { elem })
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            let error = substitute_type(
                append,
                module,
                type_store,
                *error,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            let value = substitute_type(
                append,
                module,
                type_store,
                *value,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            append.intern(TyKind::ErrorUnion { error, value })
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => {
            let params = params
                .iter()
                .map(|param| {
                    substitute_type(
                        append,
                        module,
                        type_store,
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
            let return_type = substitute_type(
                append,
                module,
                type_store,
                *return_type,
                substitutions,
                const_substitutions,
                TypeSubstitutionTarget {
                    projection: projection_context,
                    self_ty: self_substitution,
                },
            );
            append.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: *is_variadic,
            })
        }
        Some(TyKind::Callable {
            is_readonly,
            params,
            return_type,
        }) => {
            let substitute = |ty| {
                substitute_type(
                    append,
                    module,
                    type_store,
                    ty,
                    substitutions,
                    const_substitutions,
                    TypeSubstitutionTarget {
                        projection: projection_context,
                        self_ty: self_substitution,
                    },
                )
            };
            append.intern(TyKind::Callable {
                is_readonly: *is_readonly,
                params: params.iter().copied().map(substitute).collect(),
                return_type: substitute(*return_type),
            })
        }
        Some(TyKind::CallablePointee {
            params,
            return_type,
        }) => {
            let substitute = |ty| {
                substitute_type(
                    append,
                    module,
                    type_store,
                    ty,
                    substitutions,
                    const_substitutions,
                    TypeSubstitutionTarget {
                        projection: projection_context,
                        self_ty: self_substitution,
                    },
                )
            };
            append.intern(TyKind::CallablePointee {
                params: params.iter().copied().map(substitute).collect(),
                return_type: substitute(*return_type),
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
                    substitute_type(
                        append,
                        module,
                        type_store,
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
                    substitute_const_arg(
                        append,
                        module,
                        type_store,
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
            append.intern(TyKind::Nominal {
                def_id: *def_id,
                args,
                const_args,
            })
        }
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            let args = args
                .iter()
                .map(|arg| {
                    substitute_type(
                        append,
                        module,
                        type_store,
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
            append.intern(TyKind::BuiltinTrait {
                trait_id: *trait_id,
                args,
            })
        }
        Some(TyKind::BuiltinType(builtin)) => append.intern(TyKind::BuiltinType(*builtin)),
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
                    substitute_type(
                        append,
                        module,
                        type_store,
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
                    substitute_const_arg(
                        append,
                        module,
                        type_store,
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
                            substitute_type(
                                append,
                                module,
                                type_store,
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
                            substitute_const_arg(
                                append,
                                module,
                                type_store,
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
                    ty: substitute_type(
                        append,
                        module,
                        type_store,
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
            append.intern(TyKind::TraitObject {
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
                    substitute_type(
                        append,
                        module,
                        type_store,
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
                    substitute_const_arg(
                        append,
                        module,
                        type_store,
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
                            substitute_type(
                                append,
                                module,
                                type_store,
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
                            substitute_const_arg(
                                append,
                                module,
                                type_store,
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
                    ty: substitute_type(
                        append,
                        module,
                        type_store,
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
            append.intern(TyKind::TraitObjectPointee {
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
            let self_ty = substitute_type(
                append,
                module,
                type_store,
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
                    substitute_type(
                        append,
                        module,
                        type_store,
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
                    substitute_const_arg(
                        append,
                        module,
                        type_store,
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
                && projection_context_matches(
                    type_store,
                    self_ty,
                    context.self_ty,
                    &trait_args,
                    context.trait_args,
                    &trait_const_args,
                    context.trait_const_args,
                )
                && let Some(associated_type) = context
                    .associated_types
                    .iter()
                    .find(|associated_type| associated_type.name == *name)
            {
                let ty = module.normalization.normalize(associated_type.ty);
                return ty;
            }
            append.intern(TyKind::Projection {
                self_ty,
                trait_id: *trait_id,
                trait_args,
                trait_const_args,
                name: *name,
            })
        }
        Some(TyKind::Error | TyKind::ConstOnly | TyKind::Primitive(_) | TyKind::Vector { .. })
        | None => ty,
    }
}

pub(super) fn substitutions_from_generic_params(
    params: &[nia_item_signatures::GenericParamSignature],
    type_args: &[nia_ids::InternedTyId],
    const_args: &[nia_ty::ConstGenericArg],
) -> (
    SymbolMap<nia_ids::InternedTyId>,
    SymbolMap<nia_ty::ConstGenericArg>,
) {
    // Type and const arguments are stored in separate lists on applied types.
    // Walking the declaration-order parameter list with one cursor per kind
    // reconstructs the original mixed parameter list without assuming that
    // const arguments are self-describing or grouped in the declaration.
    let mut type_args = type_args.iter().copied();
    let mut const_args = const_args.iter().cloned();
    let mut substitutions = SymbolMap::default();
    let mut const_substitutions = SymbolMap::default();
    for param in params {
        match param.kind {
            nia_item_signatures::GenericParamSignatureKind::Type => {
                if let Some(arg) = type_args.next() {
                    substitutions.insert(param.name, arg);
                }
            }
            nia_item_signatures::GenericParamSignatureKind::Const { .. } => {
                if let Some(arg) = const_args.next() {
                    const_substitutions.insert(param.name, arg);
                }
            }
        }
    }
    (substitutions, const_substitutions)
}

fn substitute_const_arg(
    append: &TypeStoreAppend,
    module: &ExtensionModuleInput<'_>,
    type_store: &TypeStore,
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
        ty: substitute_type(
            append,
            module,
            type_store,
            arg.ty,
            substitutions,
            const_substitutions,
            target,
        ),
        value: arg.value.clone(),
    }
}

fn substitute_array_len(
    len: nia_ty::ArrayLenTy,
    const_substitutions: &SymbolMap<nia_ty::ConstGenericArg>,
) -> nia_ty::ArrayLenTy {
    match len {
        nia_ty::ArrayLenTy::GenericParam(name) => const_substitutions
            .get(&name)
            .and_then(nia_ty::array_len_from_const_arg)
            .unwrap_or(nia_ty::ArrayLenTy::GenericParam(name)),
        len => len,
    }
}
