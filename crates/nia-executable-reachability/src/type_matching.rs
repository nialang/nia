// SPDX-License-Identifier: GPL-3.0-or-later
//! Type-pattern recovery for executable trait witnesses.
//!
//! Body checking has already selected a valid extension implementation before
//! this module runs. Reachability must nevertheless replay the implementation
//! target match to recover the concrete type and const arguments used by its
//! where predicates. Losing one of those substitutions can silently omit a
//! method body that the backend will call.
//!
//! Types normally share the compilation session's `TypeStore`. The explicit
//! [`TypedTyRef`] pairing is retained for structural comparisons because query
//! inputs and cached signatures may originate from different stores; raw
//! `InternedTyId` equality is not a cross-store type-equivalence operation.

use super::*;

#[derive(Debug)]
pub(super) struct ReachableExtensionMethodMatch<'a> {
    impl_signature: &'a ProgramTraitImplSignature,
    substitutions: PatternSubstitutions,
}

#[derive(Debug, Clone, Copy)]
struct TypedTyRef<'a> {
    store: &'a TypeStore,
    ty: InternedTyId,
}

impl<'a> TypedTyRef<'a> {
    fn kind(self) -> Option<&'a TyKind> {
        self.store.get(self.ty)
    }
}

#[derive(Debug, Clone, Copy)]
enum SubstitutionTy {
    Canonical(InternedTyId),
}

/// Evidence recovered while matching a generic impl target to a concrete use.
///
/// `const_param_types` preserves the declared type of an impl const parameter.
/// Array lengths carry only a value-level representation, so this table lets a
/// successful array pattern produce a complete `ConstGenericArg` without
/// interning an unrelated type during matching.
#[derive(Debug, Default)]
struct PatternSubstitutions {
    types: SymbolMap<SubstitutionTy>,
    consts: SymbolMap<nia_ty::ConstGenericArg>,
    array_lens: SymbolMap<nia_ty::ArrayLenTy>,
    const_param_types: SymbolMap<InternedTyId>,
}

impl PatternSubstitutions {
    fn for_impl(generic_params: &[nia_item_signatures::GenericParamSignature]) -> Self {
        let const_param_types = generic_params
            .iter()
            .filter_map(|param| match param.kind {
                nia_item_signatures::GenericParamSignatureKind::Const { ty } => {
                    Some((param.name, ty))
                }
                nia_item_signatures::GenericParamSignatureKind::Type => None,
            })
            .collect();
        Self {
            const_param_types,
            ..Self::default()
        }
    }
}

pub(super) struct ReachableExtensionMatchInput<'a> {
    pub(super) method: &'a nia_defs::ExtensionMethod,
    pub(super) trait_id: TraitId,
    pub(super) self_ty: InternedTyId,
    pub(super) trait_args: &'a [InternedTyId],
    /// Const arguments must remain paired with the trait instance: two impls
    /// that differ only in `Trait[true]` versus `Trait[false]` are distinct
    /// executable witnesses and must not share reachability results.
    pub(super) trait_const_args: &'a [nia_ty::ConstGenericArg],
    pub(super) use_module_id: ModuleId,
    pub(super) type_store: &'a TypeStore,
    pub(super) extension_index: &'a dyn ExecutableExtensionLookup,
    pub(super) modules_by_id: &'a HashMap<ModuleId, ReachableModuleInput<'a>>,
}

pub(super) fn with_reachable_extension_method_match(
    input: ReachableExtensionMatchInput<'_>,
    f: &mut dyn FnMut(ReachableExtensionMethodMatch<'_>),
) -> bool {
    let ReachableExtensionMatchInput {
        method,
        trait_id,
        self_ty,
        trait_args,
        trait_const_args,
        use_module_id,
        type_store,
        extension_index,
        modules_by_id,
    } = input;
    if method.trait_args.len() != trait_args.len()
        || method.trait_const_args.len() != trait_const_args.len()
    {
        return false;
    }
    let mut matched = false;
    extension_index.with_trait_impl_for_method(method, trait_id, &mut |impl_signature| {
        if impl_signature.trait_args.len() != trait_args.len()
            || impl_signature.trait_const_args.len() != trait_const_args.len()
        {
            return;
        }
        if !modules_by_id.contains_key(&use_module_id) {
            return;
        }
        let self_ref = TypedTyRef {
            store: type_store,
            ty: self_ty,
        };
        let pointee_ref = typed_pointer_elem_ref(self_ref);
        let direct = match_reachable_extension_impl(
            TypedTyRef {
                store: type_store,
                ty: impl_signature.target_ty,
            },
            impl_signature.trait_args.iter().map(|ty| TypedTyRef {
                store: type_store,
                ty: *ty,
            }),
            &impl_signature.generic_params,
            self_ref,
            trait_args.iter().map(|ty| TypedTyRef {
                store: type_store,
                ty: *ty,
            }),
            &impl_signature.trait_const_args,
            trait_const_args,
        );
        let pointee = direct.is_none().then(|| {
            match_reachable_extension_impl(
                TypedTyRef {
                    store: type_store,
                    ty: impl_signature.target_ty,
                },
                impl_signature.trait_args.iter().map(|ty| TypedTyRef {
                    store: type_store,
                    ty: *ty,
                }),
                &impl_signature.generic_params,
                pointee_ref?,
                trait_args.iter().map(|ty| TypedTyRef {
                    store: type_store,
                    ty: *ty,
                }),
                &impl_signature.trait_const_args,
                trait_const_args,
            )
        });
        let Some(substitutions) = direct.or_else(|| pointee.flatten()) else {
            return;
        };
        matched = true;
        f(ReachableExtensionMethodMatch {
            impl_signature,
            substitutions,
        });
    });
    matched
}

fn match_reachable_extension_impl<'a>(
    impl_target: TypedTyRef<'a>,
    impl_trait_args: impl IntoIterator<Item = TypedTyRef<'a>>,
    impl_generic_params: &[nia_item_signatures::GenericParamSignature],
    self_ty: TypedTyRef<'a>,
    trait_args: impl IntoIterator<Item = TypedTyRef<'a>>,
    impl_trait_const_args: &[nia_ty::ConstGenericArg],
    trait_const_args: &[nia_ty::ConstGenericArg],
) -> Option<PatternSubstitutions> {
    let mut substitutions = PatternSubstitutions::for_impl(impl_generic_params);
    if !match_type_pattern(impl_target, self_ty, &mut substitutions) {
        return None;
    }
    let matches_trait_args = impl_trait_args
        .into_iter()
        .zip(trait_args)
        .all(|(pattern, actual)| match_type_pattern(pattern, actual, &mut substitutions));
    if !matches_trait_args
        || !match_const_generic_arg_patterns(
            impl_target.store,
            impl_trait_const_args,
            self_ty.store,
            trait_const_args,
            &mut substitutions,
        )
    {
        return None;
    }
    Some(substitutions)
}

pub(super) fn extend_reachable_trait_methods_from_impl_where_predicates(
    program_signatures: ExecutableSignatureIndex<'_>,
    type_store: &TypeStore,
    matched: &ReachableExtensionMethodMatch,
    fallback_method_name: &SymbolId,
    module_id: ModuleId,
    traits: &mut ReachableTraitRefs,
) {
    let append = type_store.append_for_module(module_id);
    let types = ReachabilityTypeCx {
        store: type_store,
        append: &append,
    };
    for predicate in &matched.impl_signature.where_predicates {
        let substitutions = TypeSubstitutions::typed_generics(
            &matched.substitutions.types,
            &matched.substitutions.consts,
        );
        let Some(self_ty) = substitute_ty(types, predicate.ty, &substitutions) else {
            continue;
        };
        for bound in &predicate.bounds {
            let Some(trait_ty) = substitute_ty(types, bound.trait_ty, &substitutions) else {
                continue;
            };
            let Some((trait_id, trait_args, trait_const_args)) =
                trait_id_and_args(type_store, trait_ty)
            else {
                continue;
            };
            if let TraitId::Source(trait_def) = trait_id
                && let Some(trait_signature) = (program_signatures.trait_)(trait_def)
            {
                traits.insert_methods_with_const_args(
                    module_id,
                    trait_id,
                    trait_signature
                        .signature
                        .methods
                        .iter()
                        .map(|method| ReachableTraitMethodName { name: method.name }),
                    self_ty,
                    &trait_args,
                    &trait_const_args,
                );
                continue;
            }
            traits.insert_method_with_const_args(
                module_id,
                trait_id,
                *fallback_method_name,
                self_ty,
                trait_args,
                trait_const_args,
            );
        }
    }
}

fn match_type_pattern<'a>(
    pattern: TypedTyRef<'a>,
    actual: TypedTyRef<'a>,
    substitutions: &mut PatternSubstitutions,
) -> bool {
    let Some(pattern_ty) = pattern.kind() else {
        return false;
    };
    match pattern_ty {
        TyKind::GenericParam(name) => {
            if let Some(existing) = substitutions.types.get(name).copied() {
                substitution_ty_equivalent(existing, actual)
            } else {
                substitutions
                    .types
                    .insert(*name, SubstitutionTy::Canonical(actual.ty));
                true
            }
        }
        TyKind::SelfParam => matches!(actual.kind(), Some(TyKind::SelfParam)),
        TyKind::Opaque => matches!(actual.kind(), Some(TyKind::Opaque)),
        TyKind::Tuple(pattern_elems) => match actual.kind() {
            Some(TyKind::Tuple(actual_elems)) if pattern_elems.len() == actual_elems.len() => {
                pattern_elems
                    .iter()
                    .zip(actual_elems)
                    .all(|(pattern_elem, actual_elem)| {
                        match_type_pattern(
                            TypedTyRef {
                                store: pattern.store,
                                ty: *pattern_elem,
                            },
                            TypedTyRef {
                                store: actual.store,
                                ty: *actual_elem,
                            },
                            substitutions,
                        )
                    })
            }
            _ => false,
        },
        TyKind::ClosureState {
            closure_id,
            captures,
            params,
            return_type,
        } => match actual.kind() {
            Some(TyKind::ClosureState {
                closure_id: actual_id,
                captures: actual_captures,
                params: actual_params,
                return_type: actual_return,
            }) if closure_id == actual_id
                && captures.len() == actual_captures.len()
                && params.len() == actual_params.len() =>
            {
                captures
                    .iter()
                    .zip(actual_captures)
                    .all(|(pattern_ty, actual_ty)| {
                        match_type_pattern(
                            TypedTyRef {
                                store: pattern.store,
                                ty: *pattern_ty,
                            },
                            TypedTyRef {
                                store: actual.store,
                                ty: *actual_ty,
                            },
                            substitutions,
                        )
                    })
                    && params
                        .iter()
                        .zip(actual_params)
                        .all(|(pattern_ty, actual_ty)| {
                            match_type_pattern(
                                TypedTyRef {
                                    store: pattern.store,
                                    ty: *pattern_ty,
                                },
                                TypedTyRef {
                                    store: actual.store,
                                    ty: *actual_ty,
                                },
                                substitutions,
                            )
                        })
                    && match_type_pattern(
                        TypedTyRef {
                            store: pattern.store,
                            ty: *return_type,
                        },
                        TypedTyRef {
                            store: actual.store,
                            ty: *actual_return,
                        },
                        substitutions,
                    )
            }
            _ => false,
        },
        TyKind::Primitive(pattern_primitive) => {
            matches!(actual.kind(), Some(TyKind::Primitive(actual_primitive)) if pattern_primitive == actual_primitive)
        }
        TyKind::BuiltinType(pattern_builtin) => {
            matches!(actual.kind(), Some(TyKind::BuiltinType(actual_builtin)) if pattern_builtin == actual_builtin)
        }
        TyKind::Vector {
            elem: pattern_elem,
            lanes: pattern_lanes,
        } => {
            matches!(actual.kind(), Some(TyKind::Vector { elem, lanes }) if elem == pattern_elem && lanes == pattern_lanes)
        }
        TyKind::Pointer { is_readonly, elem } => match actual.kind() {
            Some(TyKind::Pointer {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => match_type_pattern(
                TypedTyRef {
                    store: pattern.store,
                    ty: *elem,
                },
                TypedTyRef {
                    store: actual.store,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::VolatilePointer { is_readonly, elem } => match actual.kind() {
            Some(TyKind::VolatilePointer {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => match_type_pattern(
                TypedTyRef {
                    store: pattern.store,
                    ty: *elem,
                },
                TypedTyRef {
                    store: actual.store,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::Slice { is_readonly, elem } => match actual.kind() {
            Some(TyKind::Slice {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => match_type_pattern(
                TypedTyRef {
                    store: pattern.store,
                    ty: *elem,
                },
                TypedTyRef {
                    store: actual.store,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::SlicePointee { elem } => match actual.kind() {
            Some(TyKind::SlicePointee { elem: actual_elem }) => match_type_pattern(
                TypedTyRef {
                    store: pattern.store,
                    ty: *elem,
                },
                TypedTyRef {
                    store: actual.store,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::Array { len, elem } => match actual.kind() {
            Some(TyKind::Array {
                len: actual_len,
                elem: actual_elem,
            }) if match_array_len_pattern(len, actual_len, substitutions) => match_type_pattern(
                TypedTyRef {
                    store: pattern.store,
                    ty: *elem,
                },
                TypedTyRef {
                    store: actual.store,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::Range { kind, bound } => match actual.kind() {
            Some(TyKind::Range {
                kind: actual_kind,
                bound: actual_bound,
            }) if kind == actual_kind => match (bound, actual_bound) {
                (Some(bound), Some(actual_bound)) => match_type_pattern(
                    TypedTyRef {
                        store: pattern.store,
                        ty: *bound,
                    },
                    TypedTyRef {
                        store: actual.store,
                        ty: *actual_bound,
                    },
                    substitutions,
                ),
                (None, None) => true,
                _ => false,
            },
            _ => false,
        },
        TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        } => match actual.kind() {
            Some(TyKind::FunctionPointer {
                params: actual_params,
                return_type: actual_return,
                is_variadic: actual_variadic,
            }) if is_variadic == actual_variadic && params.len() == actual_params.len() => {
                params
                    .iter()
                    .zip(actual_params)
                    .all(|(param, actual_param)| {
                        match_type_pattern(
                            TypedTyRef {
                                store: pattern.store,
                                ty: *param,
                            },
                            TypedTyRef {
                                store: actual.store,
                                ty: *actual_param,
                            },
                            substitutions,
                        )
                    })
                    && match_type_pattern(
                        TypedTyRef {
                            store: pattern.store,
                            ty: *return_type,
                        },
                        TypedTyRef {
                            store: actual.store,
                            ty: *actual_return,
                        },
                        substitutions,
                    )
            }
            _ => false,
        },
        TyKind::Callable {
            is_readonly,
            params,
            return_type,
        } => match actual.kind() {
            Some(TyKind::Callable {
                is_readonly: actual_readonly,
                params: actual_params,
                return_type: actual_return,
            }) if is_readonly == actual_readonly && params.len() == actual_params.len() => {
                params
                    .iter()
                    .zip(actual_params)
                    .all(|(param, actual_param)| {
                        match_type_pattern(
                            TypedTyRef {
                                store: pattern.store,
                                ty: *param,
                            },
                            TypedTyRef {
                                store: actual.store,
                                ty: *actual_param,
                            },
                            substitutions,
                        )
                    })
                    && match_type_pattern(
                        TypedTyRef {
                            store: pattern.store,
                            ty: *return_type,
                        },
                        TypedTyRef {
                            store: actual.store,
                            ty: *actual_return,
                        },
                        substitutions,
                    )
            }
            _ => false,
        },
        TyKind::CallablePointee {
            params,
            return_type,
        } => match actual.kind() {
            Some(TyKind::CallablePointee {
                params: actual_params,
                return_type: actual_return,
            }) if params.len() == actual_params.len() => {
                params
                    .iter()
                    .zip(actual_params)
                    .all(|(param, actual_param)| {
                        match_type_pattern(
                            TypedTyRef {
                                store: pattern.store,
                                ty: *param,
                            },
                            TypedTyRef {
                                store: actual.store,
                                ty: *actual_param,
                            },
                            substitutions,
                        )
                    })
                    && match_type_pattern(
                        TypedTyRef {
                            store: pattern.store,
                            ty: *return_type,
                        },
                        TypedTyRef {
                            store: actual.store,
                            ty: *actual_return,
                        },
                        substitutions,
                    )
            }
            _ => false,
        },
        TyKind::Optional { elem } => match actual.kind() {
            Some(TyKind::Optional { elem: actual_elem }) => match_type_pattern(
                TypedTyRef {
                    store: pattern.store,
                    ty: *elem,
                },
                TypedTyRef {
                    store: actual.store,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::ErrorUnion { error, value } => match actual.kind() {
            Some(TyKind::ErrorUnion {
                error: actual_error,
                value: actual_value,
            }) => {
                match_type_pattern(
                    TypedTyRef {
                        store: pattern.store,
                        ty: *error,
                    },
                    TypedTyRef {
                        store: actual.store,
                        ty: *actual_error,
                    },
                    substitutions,
                ) && match_type_pattern(
                    TypedTyRef {
                        store: pattern.store,
                        ty: *value,
                    },
                    TypedTyRef {
                        store: actual.store,
                        ty: *actual_value,
                    },
                    substitutions,
                )
            }
            _ => false,
        },
        TyKind::Nominal {
            def_id,
            args,
            const_args,
        } => match actual.kind() {
            Some(TyKind::Nominal {
                def_id: actual_def_id,
                args: actual_args,
                const_args: actual_const_args,
            }) if def_id == actual_def_id
                && args.len() == actual_args.len()
                && const_args.len() == actual_const_args.len() =>
            {
                args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                    match_type_pattern(
                        TypedTyRef {
                            store: pattern.store,
                            ty: *arg,
                        },
                        TypedTyRef {
                            store: actual.store,
                            ty: *actual_arg,
                        },
                        substitutions,
                    )
                }) && match_const_generic_arg_patterns(
                    pattern.store,
                    const_args,
                    actual.store,
                    actual_const_args,
                    substitutions,
                )
            }
            _ => false,
        },
        TyKind::BuiltinTrait { trait_id, args } => match actual.kind() {
            Some(TyKind::BuiltinTrait {
                trait_id: actual_trait_id,
                args: actual_args,
            }) if trait_id == actual_trait_id && args.len() == actual_args.len() => {
                args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                    match_type_pattern(
                        TypedTyRef {
                            store: pattern.store,
                            ty: *arg,
                        },
                        TypedTyRef {
                            store: actual.store,
                            ty: *actual_arg,
                        },
                        substitutions,
                    )
                })
            }
            _ => false,
        },
        TyKind::TraitObject { .. }
        | TyKind::TraitObjectPointee { .. }
        | TyKind::Projection { .. } => typed_refs_equivalent(pattern, actual),
        TyKind::Error => matches!(actual.kind(), Some(TyKind::Error)),
        TyKind::ConstOnly => matches!(actual.kind(), Some(TyKind::ConstOnly)),
    }
}

fn match_array_len_pattern(
    pattern: &nia_ty::ArrayLenTy,
    actual: &nia_ty::ArrayLenTy,
    substitutions: &mut PatternSubstitutions,
) -> bool {
    if pattern == actual {
        return true;
    }
    let nia_ty::ArrayLenTy::GenericParam(name) = pattern else {
        return false;
    };
    if let Some(existing) = substitutions.array_lens.get(name) {
        return existing == actual;
    }
    substitutions.array_lens.insert(*name, actual.clone());

    let Some(value) = const_value_from_array_len(actual) else {
        // Layout builtins are valid concrete lengths, but this phase has no
        // evaluator with which to encode their result as a const-generic
        // value. Retaining the length still enforces repeated-N consistency
        // and keeps the matching implementation executable-reachable.
        return matches!(actual, nia_ty::ArrayLenTy::Builtin { .. });
    };
    let Some(ty) = substitutions.const_param_types.get(name).copied() else {
        return false;
    };
    record_const_substitution(*name, nia_ty::ConstGenericArg { ty, value }, substitutions)
}

fn match_const_generic_arg_patterns(
    pattern_store: &TypeStore,
    patterns: &[nia_ty::ConstGenericArg],
    actual_store: &TypeStore,
    actuals: &[nia_ty::ConstGenericArg],
    substitutions: &mut PatternSubstitutions,
) -> bool {
    patterns.iter().zip(actuals).all(|(pattern, actual)| {
        if let nia_ty::ConstGenericValue::GenericParam(name) = pattern.value {
            return typed_refs_equivalent(
                TypedTyRef {
                    store: pattern_store,
                    ty: pattern.ty,
                },
                TypedTyRef {
                    store: actual_store,
                    ty: actual.ty,
                },
            ) && record_const_substitution(
                name,
                nia_ty::ConstGenericArg {
                    ty: pattern.ty,
                    value: actual.value.clone(),
                },
                substitutions,
            );
        }
        pattern.value == actual.value
            && typed_refs_equivalent(
                TypedTyRef {
                    store: pattern_store,
                    ty: pattern.ty,
                },
                TypedTyRef {
                    store: actual_store,
                    ty: actual.ty,
                },
            )
    })
}

fn record_const_substitution(
    name: SymbolId,
    arg: nia_ty::ConstGenericArg,
    substitutions: &mut PatternSubstitutions,
) -> bool {
    if let Some(existing) = substitutions.consts.get(&name) {
        existing == &arg
    } else {
        substitutions.consts.insert(name, arg);
        true
    }
}

fn const_value_from_array_len(len: &nia_ty::ArrayLenTy) -> Option<nia_ty::ConstGenericValue> {
    match len {
        nia_ty::ArrayLenTy::GenericParam(name) => {
            Some(nia_ty::ConstGenericValue::GenericParam(*name))
        }
        nia_ty::ArrayLenTy::ConstValue(value) => Some(nia_ty::ConstGenericValue::Int(
            nia_ty::IntConst::unsigned((*value).into()),
        )),
        nia_ty::ArrayLenTy::ConstExpr(id) => Some(nia_ty::ConstGenericValue::ConstExpr(*id)),
        nia_ty::ArrayLenTy::Infer | nia_ty::ArrayLenTy::Builtin { .. } => None,
    }
}

fn substitution_ty_equivalent(existing: SubstitutionTy, actual: TypedTyRef<'_>) -> bool {
    match existing {
        SubstitutionTy::Canonical(existing) => typed_refs_equivalent(
            TypedTyRef {
                store: actual.store,
                ty: existing,
            },
            actual,
        ),
    }
}

fn typed_pointer_elem_ref(ty: TypedTyRef<'_>) -> Option<TypedTyRef<'_>> {
    match ty.kind() {
        Some(TyKind::Pointer { elem, .. }) => Some(TypedTyRef {
            store: ty.store,
            ty: *elem,
        }),
        _ => None,
    }
}

fn typed_refs_equivalent(left: TypedTyRef<'_>, right: TypedTyRef<'_>) -> bool {
    if left.ty == right.ty {
        return true;
    }
    typed_refs_structurally_equivalent(left, right)
}

fn typed_refs_structurally_equivalent(left: TypedTyRef<'_>, right: TypedTyRef<'_>) -> bool {
    match (left.kind(), right.kind()) {
        (Some(TyKind::Error), Some(TyKind::Error)) => true,
        (Some(TyKind::ConstOnly), Some(TyKind::ConstOnly)) => true,
        (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
        (Some(TyKind::BuiltinType(left)), Some(TyKind::BuiltinType(right))) => left == right,
        (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
        (Some(TyKind::SelfParam), Some(TyKind::SelfParam)) => true,
        (
            Some(TyKind::Pointer {
                is_readonly: left_readonly,
                elem: left_elem,
            }),
            Some(TyKind::Pointer {
                is_readonly: right_readonly,
                elem: right_elem,
            }),
        )
        | (
            Some(TyKind::VolatilePointer {
                is_readonly: left_readonly,
                elem: left_elem,
            }),
            Some(TyKind::VolatilePointer {
                is_readonly: right_readonly,
                elem: right_elem,
            }),
        )
        | (
            Some(TyKind::Slice {
                is_readonly: left_readonly,
                elem: left_elem,
            }),
            Some(TyKind::Slice {
                is_readonly: right_readonly,
                elem: right_elem,
            }),
        ) => {
            left_readonly == right_readonly
                && typed_refs_equivalent(
                    TypedTyRef {
                        store: left.store,
                        ty: *left_elem,
                    },
                    TypedTyRef {
                        store: right.store,
                        ty: *right_elem,
                    },
                )
        }
        (
            Some(TyKind::SlicePointee { elem: left_elem }),
            Some(TyKind::SlicePointee { elem: right_elem }),
        )
        | (
            Some(TyKind::Optional { elem: left_elem }),
            Some(TyKind::Optional { elem: right_elem }),
        ) => typed_refs_equivalent(
            TypedTyRef {
                store: left.store,
                ty: *left_elem,
            },
            TypedTyRef {
                store: right.store,
                ty: *right_elem,
            },
        ),
        (
            Some(TyKind::Array {
                len: left_len,
                elem: left_elem,
            }),
            Some(TyKind::Array {
                len: right_len,
                elem: right_elem,
            }),
        ) => {
            array_lens_equivalent(left.store, left_len, right.store, right_len)
                && typed_refs_equivalent(
                    TypedTyRef {
                        store: left.store,
                        ty: *left_elem,
                    },
                    TypedTyRef {
                        store: right.store,
                        ty: *right_elem,
                    },
                )
        }
        (
            Some(TyKind::Vector {
                elem: left_elem,
                lanes: left_lanes,
            }),
            Some(TyKind::Vector {
                elem: right_elem,
                lanes: right_lanes,
            }),
        ) => left_elem == right_elem && left_lanes == right_lanes,
        (
            Some(TyKind::Range {
                kind: left_kind,
                bound: left_bound,
            }),
            Some(TyKind::Range {
                kind: right_kind,
                bound: right_bound,
            }),
        ) => {
            left_kind == right_kind
                && optional_typed_refs_equivalent(
                    left.store,
                    *left_bound,
                    right.store,
                    *right_bound,
                )
        }
        (
            Some(TyKind::FunctionPointer {
                params: left_params,
                return_type: left_return,
                is_variadic: left_variadic,
            }),
            Some(TyKind::FunctionPointer {
                params: right_params,
                return_type: right_return,
                is_variadic: right_variadic,
            }),
        ) => {
            left_variadic == right_variadic
                && typed_ref_slices_equivalent(left.store, left_params, right.store, right_params)
                && typed_refs_equivalent(
                    TypedTyRef {
                        store: left.store,
                        ty: *left_return,
                    },
                    TypedTyRef {
                        store: right.store,
                        ty: *right_return,
                    },
                )
        }
        (
            Some(TyKind::Callable {
                is_readonly: left_readonly,
                params: left_params,
                return_type: left_return,
            }),
            Some(TyKind::Callable {
                is_readonly: right_readonly,
                params: right_params,
                return_type: right_return,
            }),
        ) => {
            left_readonly == right_readonly
                && typed_ref_slices_equivalent(left.store, left_params, right.store, right_params)
                && typed_refs_equivalent(
                    TypedTyRef {
                        store: left.store,
                        ty: *left_return,
                    },
                    TypedTyRef {
                        store: right.store,
                        ty: *right_return,
                    },
                )
        }
        (
            Some(TyKind::CallablePointee {
                params: left_params,
                return_type: left_return,
            }),
            Some(TyKind::CallablePointee {
                params: right_params,
                return_type: right_return,
            }),
        ) => {
            typed_ref_slices_equivalent(left.store, left_params, right.store, right_params)
                && typed_refs_equivalent(
                    TypedTyRef {
                        store: left.store,
                        ty: *left_return,
                    },
                    TypedTyRef {
                        store: right.store,
                        ty: *right_return,
                    },
                )
        }
        (
            Some(TyKind::ErrorUnion {
                error: left_error,
                value: left_value,
            }),
            Some(TyKind::ErrorUnion {
                error: right_error,
                value: right_value,
            }),
        ) => {
            typed_refs_equivalent(
                TypedTyRef {
                    store: left.store,
                    ty: *left_error,
                },
                TypedTyRef {
                    store: right.store,
                    ty: *right_error,
                },
            ) && typed_refs_equivalent(
                TypedTyRef {
                    store: left.store,
                    ty: *left_value,
                },
                TypedTyRef {
                    store: right.store,
                    ty: *right_value,
                },
            )
        }
        (
            Some(TyKind::Nominal {
                def_id: left_def,
                args: left_args,
                const_args: left_const_args,
            }),
            Some(TyKind::Nominal {
                def_id: right_def,
                args: right_args,
                const_args: right_const_args,
            }),
        ) => {
            left_def == right_def
                && typed_ref_slices_equivalent(left.store, left_args, right.store, right_args)
                && const_generic_args_equivalent(
                    left.store,
                    left_const_args,
                    right.store,
                    right_const_args,
                )
        }
        (
            Some(TyKind::BuiltinTrait {
                trait_id: left_trait,
                args: left_args,
            }),
            Some(TyKind::BuiltinTrait {
                trait_id: right_trait,
                args: right_args,
            }),
        ) => {
            left_trait == right_trait
                && typed_ref_slices_equivalent(left.store, left_args, right.store, right_args)
        }
        (
            Some(TyKind::TraitObject {
                is_readonly: left_readonly,
                trait_id: left_trait,
                trait_args: left_args,
                trait_const_args: left_const_args,
                associated_type_bindings: left_bindings,
            }),
            Some(TyKind::TraitObject {
                is_readonly: right_readonly,
                trait_id: right_trait,
                trait_args: right_args,
                trait_const_args: right_const_args,
                associated_type_bindings: right_bindings,
            }),
        ) => {
            left_readonly == right_readonly
                && trait_object_parts_equivalent(
                    TraitObjectParts {
                        store: left.store,
                        trait_id: *left_trait,
                        args: left_args,
                        const_args: left_const_args,
                        bindings: left_bindings,
                    },
                    TraitObjectParts {
                        store: right.store,
                        trait_id: *right_trait,
                        args: right_args,
                        const_args: right_const_args,
                        bindings: right_bindings,
                    },
                )
        }
        (
            Some(TyKind::TraitObjectPointee {
                trait_id: left_trait,
                trait_args: left_args,
                trait_const_args: left_const_args,
                associated_type_bindings: left_bindings,
            }),
            Some(TyKind::TraitObjectPointee {
                trait_id: right_trait,
                trait_args: right_args,
                trait_const_args: right_const_args,
                associated_type_bindings: right_bindings,
            }),
        ) => trait_object_parts_equivalent(
            TraitObjectParts {
                store: left.store,
                trait_id: *left_trait,
                args: left_args,
                const_args: left_const_args,
                bindings: left_bindings,
            },
            TraitObjectParts {
                store: right.store,
                trait_id: *right_trait,
                args: right_args,
                const_args: right_const_args,
                bindings: right_bindings,
            },
        ),
        (
            Some(TyKind::Projection {
                self_ty: left_self,
                trait_id: left_trait,
                trait_args: left_args,
                trait_const_args: left_const_args,
                name: left_name,
            }),
            Some(TyKind::Projection {
                self_ty: right_self,
                trait_id: right_trait,
                trait_args: right_args,
                trait_const_args: right_const_args,
                name: right_name,
            }),
        ) => {
            left_trait == right_trait
                && left_name == right_name
                && typed_refs_equivalent(
                    TypedTyRef {
                        store: left.store,
                        ty: *left_self,
                    },
                    TypedTyRef {
                        store: right.store,
                        ty: *right_self,
                    },
                )
                && typed_ref_slices_equivalent(left.store, left_args, right.store, right_args)
                && const_generic_args_equivalent(
                    left.store,
                    left_const_args,
                    right.store,
                    right_const_args,
                )
        }
        _ => false,
    }
}

fn optional_typed_refs_equivalent(
    left_store: &TypeStore,
    left: Option<InternedTyId>,
    right_store: &TypeStore,
    right: Option<InternedTyId>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => typed_refs_equivalent(
            TypedTyRef {
                store: left_store,
                ty: left,
            },
            TypedTyRef {
                store: right_store,
                ty: right,
            },
        ),
        (None, None) => true,
        _ => false,
    }
}

fn typed_ref_slices_equivalent(
    left_store: &TypeStore,
    left: &[InternedTyId],
    right_store: &TypeStore,
    right: &[InternedTyId],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            typed_refs_equivalent(
                TypedTyRef {
                    store: left_store,
                    ty: *left,
                },
                TypedTyRef {
                    store: right_store,
                    ty: *right,
                },
            )
        })
}

fn array_lens_equivalent(
    left_store: &TypeStore,
    left: &nia_ty::ArrayLenTy,
    right_store: &TypeStore,
    right: &nia_ty::ArrayLenTy,
) -> bool {
    use nia_ty::ArrayLenTy;
    match (left, right) {
        (ArrayLenTy::Infer, ArrayLenTy::Infer) => true,
        (ArrayLenTy::GenericParam(left), ArrayLenTy::GenericParam(right)) => left == right,
        (ArrayLenTy::ConstValue(left), ArrayLenTy::ConstValue(right)) => left == right,
        (ArrayLenTy::ConstExpr(left), ArrayLenTy::ConstExpr(right)) => left == right,
        (
            ArrayLenTy::Builtin {
                builtin: left_builtin,
                ty: left_ty,
            },
            ArrayLenTy::Builtin {
                builtin: right_builtin,
                ty: right_ty,
            },
        ) => {
            left_builtin == right_builtin
                && typed_refs_equivalent(
                    TypedTyRef {
                        store: left_store,
                        ty: *left_ty,
                    },
                    TypedTyRef {
                        store: right_store,
                        ty: *right_ty,
                    },
                )
        }
        _ => false,
    }
}

fn const_generic_args_equivalent(
    left_store: &TypeStore,
    left: &[nia_ty::ConstGenericArg],
    right_store: &TypeStore,
    right: &[nia_ty::ConstGenericArg],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.value == right.value
                && typed_refs_equivalent(
                    TypedTyRef {
                        store: left_store,
                        ty: left.ty,
                    },
                    TypedTyRef {
                        store: right_store,
                        ty: right.ty,
                    },
                )
        })
}

#[derive(Clone, Copy)]
struct TraitObjectParts<'a> {
    store: &'a TypeStore,
    trait_id: TraitId,
    args: &'a [InternedTyId],
    const_args: &'a [nia_ty::ConstGenericArg],
    bindings: &'a [AssociatedTypeBindingTy],
}

fn trait_object_parts_equivalent(left: TraitObjectParts<'_>, right: TraitObjectParts<'_>) -> bool {
    left.trait_id == right.trait_id
        && typed_ref_slices_equivalent(left.store, left.args, right.store, right.args)
        && const_generic_args_equivalent(left.store, left.const_args, right.store, right.const_args)
        && associated_type_bindings_equivalent(
            left.store,
            left.bindings,
            right.store,
            right.bindings,
        )
}

fn associated_type_bindings_equivalent(
    left_store: &TypeStore,
    left: &[AssociatedTypeBindingTy],
    right_store: &TypeStore,
    right: &[AssociatedTypeBindingTy],
) -> bool {
    left.len() == right.len()
        && left.iter().all(|left_binding| {
            right
                .iter()
                .find(|right_binding| {
                    associated_type_binding_keys_equivalent(
                        left_store,
                        left_binding,
                        right_store,
                        right_binding,
                    )
                })
                .is_some_and(|right_binding| {
                    typed_refs_equivalent(
                        TypedTyRef {
                            store: left_store,
                            ty: left_binding.ty,
                        },
                        TypedTyRef {
                            store: right_store,
                            ty: right_binding.ty,
                        },
                    )
                })
        })
}

fn associated_type_binding_keys_equivalent(
    left_store: &TypeStore,
    left: &AssociatedTypeBindingTy,
    right_store: &TypeStore,
    right: &AssociatedTypeBindingTy,
) -> bool {
    left.name == right.name
        && left.trait_id == right.trait_id
        && typed_ref_slices_equivalent(left_store, &left.trait_args, right_store, &right.trait_args)
        && const_generic_args_equivalent(
            left_store,
            &left.trait_const_args,
            right_store,
            &right.trait_const_args,
        )
}

pub(super) fn trait_id_and_args(
    store: &TypeStore,
    ty: InternedTyId,
) -> Option<(TraitId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
    match store.get(ty)? {
        TyKind::Nominal {
            def_id,
            args,
            const_args,
        } => Some((TraitId::Source(*def_id), args.clone(), const_args.clone())),
        TyKind::BuiltinTrait { trait_id, args } => {
            Some((TraitId::Builtin(*trait_id), args.clone(), Vec::new()))
        }
        _ => None,
    }
}

/// Type and const arguments applied while following reachability facts.
#[derive(Clone, Copy)]
pub(super) struct TypeSubstitutions<'a> {
    self_ty: Option<InternedTyId>,
    generics: TypeSubstitutionGenerics<'a>,
    consts: Option<&'a SymbolMap<nia_ty::ConstGenericArg>>,
}

#[derive(Clone, Copy)]
enum TypeSubstitutionGenerics<'a> {
    Local(&'a SymbolMap<InternedTyId>),
    Typed(&'a SymbolMap<SubstitutionTy>),
}

impl<'a> TypeSubstitutions<'a> {
    pub(super) fn local_with_consts(
        self_ty: Option<InternedTyId>,
        generics: &'a SymbolMap<InternedTyId>,
        consts: &'a SymbolMap<nia_ty::ConstGenericArg>,
    ) -> Self {
        Self {
            self_ty,
            generics: TypeSubstitutionGenerics::Local(generics),
            consts: Some(consts),
        }
    }

    fn typed_generics(
        generics: &'a SymbolMap<SubstitutionTy>,
        consts: &'a SymbolMap<nia_ty::ConstGenericArg>,
    ) -> Self {
        Self {
            self_ty: None,
            generics: TypeSubstitutionGenerics::Typed(generics),
            consts: Some(consts),
        }
    }

    fn type_arg(&self, name: &SymbolId) -> Option<InternedTyId> {
        match self.generics {
            TypeSubstitutionGenerics::Local(generics) => generics.get(name).copied(),
            TypeSubstitutionGenerics::Typed(generics) => {
                generics.get(name).copied().map(|ty| match ty {
                    SubstitutionTy::Canonical(ty) => ty,
                })
            }
        }
    }

    fn const_arg(&self, name: &SymbolId) -> Option<nia_ty::ConstGenericArg> {
        self.consts.and_then(|consts| consts.get(name)).cloned()
    }
}

/// Session type-store access used by reachability substitution.
#[derive(Clone, Copy)]
pub(super) struct ReachabilityTypeCx<'a> {
    pub(super) store: &'a TypeStore,
    pub(super) append: &'a TypeStoreAppend,
}

impl ReachabilityTypeCx<'_> {
    fn get(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.store.get(ty)
    }
}

/// Applies reachability substitutions with the canonical `nia-ty` walker.
///
/// Keeping the traversal in `nia-ty` is important: closure signatures, array
/// lengths, const-generic arguments, projections, and associated bindings must
/// all evolve together when `TyKind` gains a new nested type position.
pub(super) fn substitute_ty(
    types: ReachabilityTypeCx<'_>,
    ty: InternedTyId,
    substitutions: &TypeSubstitutions<'_>,
) -> Option<InternedTyId> {
    types.get(ty)?;
    Some(nia_ty::substitute_ty(
        types.store,
        types.append,
        ty,
        &|name| substitutions.type_arg(name),
        &|name| substitutions.const_arg(name),
        substitutions.self_ty,
    ))
}

/// Substitutes a standalone const argument carried by a generic-instantiation
/// fact. These arguments live outside a `TyKind`, so the canonical type walker
/// cannot reach them through [`substitute_ty`].
pub(super) fn substitute_const_arg(
    types: ReachabilityTypeCx<'_>,
    arg: &nia_ty::ConstGenericArg,
    substitutions: &TypeSubstitutions<'_>,
) -> Option<nia_ty::ConstGenericArg> {
    let mut substituted = match &arg.value {
        nia_ty::ConstGenericValue::GenericParam(name) => {
            substitutions.const_arg(name).unwrap_or_else(|| arg.clone())
        }
        nia_ty::ConstGenericValue::ConstExpr(_)
        | nia_ty::ConstGenericValue::Int(_)
        | nia_ty::ConstGenericValue::Bool(_)
        | nia_ty::ConstGenericValue::Char(_) => arg.clone(),
    };
    substituted.ty = substitute_ty(types, substituted.ty, substitutions)?;
    Some(substituted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::{ClosureId, DefId, ModuleIdAllocator};
    use nia_symbol::stable_hash;
    use nia_ty::{ArrayLenTy, ConstGenericArg, ConstGenericValue, PrimitiveTy};

    fn symbol(name: &str) -> SymbolId {
        SymbolId::from_stable_hash(stable_hash(name))
    }

    fn module() -> ModuleId {
        ModuleIdAllocator::new().allocate()
    }

    #[test]
    fn nominal_pattern_recovers_type_and_const_arguments() {
        let module_id = module();
        let store = TypeStore::new();
        let append = store.append_for_module(module_id);
        let usize_ty = append.primitive(PrimitiveTy::Usize);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let type_name = symbol("T");
        let const_name = symbol("N");
        let type_param = append.intern(TyKind::GenericParam(type_name));
        let def_id = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let pattern = append.intern(TyKind::Nominal {
            def_id,
            args: vec![type_param],
            const_args: vec![ConstGenericArg {
                ty: usize_ty,
                value: ConstGenericValue::GenericParam(const_name),
            }],
        });
        let actual_const = ConstGenericArg {
            ty: usize_ty,
            value: ConstGenericValue::Int(nia_ty::IntConst::unsigned(3)),
        };
        let actual = append.intern(TyKind::Nominal {
            def_id,
            args: vec![i32_ty],
            const_args: vec![actual_const.clone()],
        });
        let generic_params = vec![
            nia_item_signatures::GenericParamSignature {
                name: type_name,
                kind: nia_item_signatures::GenericParamSignatureKind::Type,
            },
            nia_item_signatures::GenericParamSignature {
                name: const_name,
                kind: nia_item_signatures::GenericParamSignatureKind::Const { ty: usize_ty },
            },
        ];

        let matched = match_reachable_extension_impl(
            TypedTyRef {
                store: &store,
                ty: pattern,
            },
            std::iter::empty(),
            &generic_params,
            TypedTyRef {
                store: &store,
                ty: actual,
            },
            std::iter::empty(),
            &[],
            &[],
        )
        .expect("generic nominal target should match its concrete instance");

        assert!(matches!(
            matched.types.get(&type_name),
            Some(SubstitutionTy::Canonical(ty)) if *ty == i32_ty
        ));
        assert_eq!(matched.consts.get(&const_name), Some(&actual_const));
    }

    #[test]
    fn repeated_const_pattern_rejects_conflicting_values() {
        let module_id = module();
        let store = TypeStore::new();
        let append = store.append_for_module(module_id);
        let usize_ty = append.primitive(PrimitiveTy::Usize);
        let const_name = symbol("N");
        let def_id = GlobalDefId {
            module_id,
            def_id: DefId(2),
        };
        let param = || ConstGenericArg {
            ty: usize_ty,
            value: ConstGenericValue::GenericParam(const_name),
        };
        let value = |bits| ConstGenericArg {
            ty: usize_ty,
            value: ConstGenericValue::Int(nia_ty::IntConst::unsigned(bits)),
        };
        let pattern = append.intern(TyKind::Nominal {
            def_id,
            args: Vec::new(),
            const_args: vec![param(), param()],
        });
        let actual = append.intern(TyKind::Nominal {
            def_id,
            args: Vec::new(),
            const_args: vec![value(2), value(3)],
        });
        let generic_params = [nia_item_signatures::GenericParamSignature {
            name: const_name,
            kind: nia_item_signatures::GenericParamSignatureKind::Const { ty: usize_ty },
        }];

        assert!(
            match_reachable_extension_impl(
                TypedTyRef {
                    store: &store,
                    ty: pattern,
                },
                std::iter::empty(),
                &generic_params,
                TypedTyRef {
                    store: &store,
                    ty: actual,
                },
                std::iter::empty(),
                &[],
                &[],
            )
            .is_none()
        );
    }

    #[test]
    fn array_pattern_accepts_builtin_length_without_fabricating_const_value() {
        let module_id = module();
        let store = TypeStore::new();
        let append = store.append_for_module(module_id);
        let usize_ty = append.primitive(PrimitiveTy::Usize);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let const_name = symbol("N");
        let pattern = append.intern(TyKind::Array {
            len: ArrayLenTy::GenericParam(const_name),
            elem: i32_ty,
        });
        let builtin_len = ArrayLenTy::Builtin {
            builtin: nia_ids::LayoutBuiltin::Size,
            ty: i32_ty,
        };
        let actual = append.intern(TyKind::Array {
            len: builtin_len.clone(),
            elem: i32_ty,
        });
        let generic_params = [nia_item_signatures::GenericParamSignature {
            name: const_name,
            kind: nia_item_signatures::GenericParamSignatureKind::Const { ty: usize_ty },
        }];

        let matched = match_reachable_extension_impl(
            TypedTyRef {
                store: &store,
                ty: pattern,
            },
            std::iter::empty(),
            &generic_params,
            TypedTyRef {
                store: &store,
                ty: actual,
            },
            std::iter::empty(),
            &[],
            &[],
        )
        .expect("layout builtin is a valid concrete array length");

        assert_eq!(matched.array_lens.get(&const_name), Some(&builtin_len));
        assert!(!matched.consts.contains_key(&const_name));
    }

    #[test]
    fn recovery_types_only_match_the_same_recovery_kind() {
        let module_id = module();
        let store = TypeStore::new();
        let append = store.append_for_module(module_id);
        let error = append.error();
        let const_only = append.intern(TyKind::ConstOnly);
        let i32_ty = append.primitive(PrimitiveTy::I32);

        for recovery in [error, const_only] {
            let mut substitutions = PatternSubstitutions::default();
            assert!(!match_type_pattern(
                TypedTyRef {
                    store: &store,
                    ty: recovery,
                },
                TypedTyRef {
                    store: &store,
                    ty: i32_ty,
                },
                &mut substitutions,
            ));
            assert!(match_type_pattern(
                TypedTyRef {
                    store: &store,
                    ty: recovery,
                },
                TypedTyRef {
                    store: &store,
                    ty: recovery,
                },
                &mut substitutions,
            ));
        }
    }

    #[test]
    fn substitution_descends_into_closures_and_array_lengths() {
        let module_id = module();
        let store = TypeStore::new();
        let append = store.append_for_module(module_id);
        let usize_ty = append.primitive(PrimitiveTy::Usize);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let type_name = symbol("T");
        let const_name = symbol("N");
        let type_param = append.intern(TyKind::GenericParam(type_name));
        let array = append.intern(TyKind::Array {
            len: ArrayLenTy::GenericParam(const_name),
            elem: type_param,
        });
        let closure = append.intern(TyKind::ClosureState {
            closure_id: ClosureId {
                owner: GlobalDefId {
                    module_id,
                    def_id: DefId(3),
                },
                ordinal: 0,
            },
            captures: vec![array],
            params: vec![array],
            return_type: array,
        });
        let type_substitutions = SymbolMap::from_iter([(type_name, i32_ty)]);
        let const_substitutions = SymbolMap::from_iter([(
            const_name,
            ConstGenericArg {
                ty: usize_ty,
                value: ConstGenericValue::Int(nia_ty::IntConst::unsigned(4)),
            },
        )]);
        let substitutions =
            TypeSubstitutions::local_with_consts(None, &type_substitutions, &const_substitutions);
        let substituted = substitute_ty(
            ReachabilityTypeCx {
                store: &store,
                append: &append,
            },
            closure,
            &substitutions,
        )
        .expect("closure type belongs to the reachability store");

        let Some(TyKind::ClosureState {
            captures,
            params,
            return_type,
            ..
        }) = store.get(substituted)
        else {
            panic!("substitution should preserve the closure constructor");
        };
        for array in captures.iter().chain(params).chain([return_type]) {
            assert!(matches!(
                store.get(*array),
                Some(TyKind::Array {
                    len: ArrayLenTy::ConstValue(4),
                    elem,
                }) if *elem == i32_ty
            ));
        }
    }
}
