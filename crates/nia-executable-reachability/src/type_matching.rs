// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug)]
pub(super) struct ReachableExtensionMethodMatch<'a> {
    impl_signature: &'a ProgramTraitImplSignature,
    substitutions: SymbolMap<SubstitutionTy>,
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

pub(super) struct ReachableExtensionMatchInput<'a> {
    pub(super) method: &'a nia_defs::ExtensionMethod,
    pub(super) trait_id: TraitId,
    pub(super) self_ty: InternedTyId,
    pub(super) trait_args: &'a [InternedTyId],
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
        use_module_id,
        type_store,
        extension_index,
        modules_by_id,
    } = input;
    if method.trait_args.len() != trait_args.len() {
        return false;
    }
    let mut matched = false;
    extension_index.with_trait_impl_for_method(method, trait_id, &mut |impl_signature| {
        if impl_signature.trait_args.len() != trait_args.len() {
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
            self_ref,
            trait_args.iter().map(|ty| TypedTyRef {
                store: type_store,
                ty: *ty,
            }),
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
                pointee_ref?,
                trait_args.iter().map(|ty| TypedTyRef {
                    store: type_store,
                    ty: *ty,
                }),
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
    self_ty: TypedTyRef<'a>,
    trait_args: impl IntoIterator<Item = TypedTyRef<'a>>,
) -> Option<SymbolMap<SubstitutionTy>> {
    let mut substitutions = SymbolMap::default();
    if !match_type_pattern(impl_target, self_ty, &mut substitutions) {
        return None;
    }
    let matches_trait_args = impl_trait_args
        .into_iter()
        .zip(trait_args)
        .all(|(pattern, actual)| match_type_pattern(pattern, actual, &mut substitutions));
    matches_trait_args.then_some(substitutions)
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
        let substitutions = TypeSubstitutions::typed_generics(&matched.substitutions);
        let Some(self_ty) = substitute_ty(types, predicate.ty, &substitutions) else {
            continue;
        };
        for bound in &predicate.bounds {
            let Some(trait_ty) = substitute_ty(types, bound.trait_ty, &substitutions) else {
                continue;
            };
            let Some((trait_id, trait_args)) = trait_id_and_args(type_store, trait_ty) else {
                continue;
            };
            if let TraitId::Source(trait_def) = trait_id
                && let Some(trait_signature) = (program_signatures.trait_)(trait_def)
            {
                traits.insert_methods(
                    module_id,
                    trait_id,
                    trait_signature
                        .signature
                        .methods
                        .iter()
                        .map(|method| ReachableTraitMethodName { name: method.name }),
                    self_ty,
                    &trait_args,
                );
                continue;
            }
            traits.insert_method(
                module_id,
                trait_id,
                *fallback_method_name,
                self_ty,
                trait_args,
            );
        }
    }
}

fn match_type_pattern<'a>(
    pattern: TypedTyRef<'a>,
    actual: TypedTyRef<'a>,
    substitutions: &mut SymbolMap<SubstitutionTy>,
) -> bool {
    let Some(pattern_ty) = pattern.kind() else {
        return false;
    };
    match pattern_ty {
        TyKind::GenericParam(name) => {
            if let Some(existing) = substitutions.get(name).copied() {
                substitution_ty_equivalent(existing, actual)
            } else {
                substitutions.insert(*name, SubstitutionTy::Canonical(actual.ty));
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
            }) if matches!(len, nia_ty::ArrayLenTy::GenericParam(_)) || len == actual_len => {
                match_type_pattern(
                    TypedTyRef {
                        store: pattern.store,
                        ty: *elem,
                    },
                    TypedTyRef {
                        store: actual.store,
                        ty: *actual_elem,
                    },
                    substitutions,
                )
            }
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
                && const_args == actual_const_args =>
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
                })
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
        TyKind::Error | TyKind::ConstOnly => true,
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
) -> Option<(TraitId, Vec<InternedTyId>)> {
    match store.get(ty)? {
        TyKind::Nominal { def_id, args, .. } => Some((TraitId::Source(*def_id), args.clone())),
        TyKind::BuiltinTrait { trait_id, args } => {
            Some((TraitId::Builtin(*trait_id), args.clone()))
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub(super) struct TypeSubstitutions<'a> {
    self_ty: Option<InternedTyId>,
    generics: TypeSubstitutionGenerics<'a>,
}

#[derive(Clone, Copy)]
enum TypeSubstitutionGenerics<'a> {
    Local(&'a SymbolMap<InternedTyId>),
    Typed(&'a SymbolMap<SubstitutionTy>),
}

impl<'a> TypeSubstitutions<'a> {
    pub(super) fn local(
        self_ty: Option<InternedTyId>,
        generics: &'a SymbolMap<InternedTyId>,
    ) -> Self {
        Self {
            self_ty,
            generics: TypeSubstitutionGenerics::Local(generics),
        }
    }

    fn typed_generics(generics: &'a SymbolMap<SubstitutionTy>) -> Self {
        Self {
            self_ty: None,
            generics: TypeSubstitutionGenerics::Typed(generics),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ReachabilityTypeCx<'a> {
    pub(super) store: &'a TypeStore,
    pub(super) append: &'a TypeStoreAppend,
}

impl ReachabilityTypeCx<'_> {
    fn get(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.store.get(ty)
    }

    fn intern(&self, kind: TyKind) -> InternedTyId {
        self.append.intern(kind)
    }
}

pub(super) fn substitute_ty(
    types: ReachabilityTypeCx<'_>,
    ty: InternedTyId,
    substitutions: &TypeSubstitutions<'_>,
) -> Option<InternedTyId> {
    let kind = types.get(ty)?.clone();
    match kind {
        TyKind::GenericParam(name) => substitute_generic_ty(&name, substitutions, ty),
        TyKind::SelfParam => substitutions.self_ty.or(Some(ty)),
        TyKind::Opaque => Some(ty),
        TyKind::Tuple(elems) => {
            let elems = elems
                .into_iter()
                .map(|elem| substitute_ty(types, elem, substitutions))
                .collect::<Option<Vec<_>>>()?;
            Some(types.intern(TyKind::Tuple(elems)))
        }
        TyKind::Pointer { is_readonly, elem } => {
            let elem = substitute_ty(types, elem, substitutions)?;
            Some(types.intern(TyKind::Pointer { is_readonly, elem }))
        }
        TyKind::VolatilePointer { is_readonly, elem } => {
            let elem = substitute_ty(types, elem, substitutions)?;
            Some(types.intern(TyKind::VolatilePointer { is_readonly, elem }))
        }
        TyKind::Slice { is_readonly, elem } => {
            let elem = substitute_ty(types, elem, substitutions)?;
            Some(types.intern(TyKind::Slice { is_readonly, elem }))
        }
        TyKind::SlicePointee { elem } => {
            let elem = substitute_ty(types, elem, substitutions)?;
            Some(types.intern(TyKind::SlicePointee { elem }))
        }
        TyKind::Array { len, elem } => {
            let elem = substitute_ty(types, elem, substitutions)?;
            Some(types.intern(TyKind::Array { len, elem }))
        }
        TyKind::Range { kind, bound } => {
            let bound = match bound {
                Some(bound) => Some(substitute_ty(types, bound, substitutions)?),
                None => None,
            };
            Some(types.intern(TyKind::Range { kind, bound }))
        }
        TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        } => {
            let params = params
                .into_iter()
                .map(|param| substitute_ty(types, param, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let return_type = substitute_ty(types, return_type, substitutions)?;
            Some(types.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }))
        }
        TyKind::Callable {
            is_readonly,
            params,
            return_type,
        } => {
            let params = params
                .into_iter()
                .map(|param| substitute_ty(types, param, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let return_type = substitute_ty(types, return_type, substitutions)?;
            Some(types.intern(TyKind::Callable {
                is_readonly,
                params,
                return_type,
            }))
        }
        TyKind::CallablePointee {
            params,
            return_type,
        } => {
            let params = params
                .into_iter()
                .map(|param| substitute_ty(types, param, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let return_type = substitute_ty(types, return_type, substitutions)?;
            Some(types.intern(TyKind::CallablePointee {
                params,
                return_type,
            }))
        }
        TyKind::Optional { elem } => {
            let elem = substitute_ty(types, elem, substitutions)?;
            Some(types.intern(TyKind::Optional { elem }))
        }
        TyKind::ErrorUnion { error, value } => {
            let error = substitute_ty(types, error, substitutions)?;
            let value = substitute_ty(types, value, substitutions)?;
            Some(types.intern(TyKind::ErrorUnion { error, value }))
        }
        TyKind::Nominal {
            def_id,
            args,
            const_args,
        } => {
            let args = args
                .into_iter()
                .map(|arg| substitute_ty(types, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let const_args = const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty(types, arg.ty, substitutions)?;
                    Some(arg)
                })
                .collect::<Option<Vec<_>>>()?;
            Some(types.intern(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }))
        }
        TyKind::BuiltinTrait { trait_id, args } => {
            let args = args
                .into_iter()
                .map(|arg| substitute_ty(types, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            Some(types.intern(TyKind::BuiltinTrait { trait_id, args }))
        }
        TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        } => {
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty(types, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty(types, arg.ty, substitutions)?;
                    Some(arg)
                })
                .collect::<Option<Vec<_>>>()?;
            let associated_type_bindings = substitute_associated_type_bindings(
                types,
                associated_type_bindings,
                substitutions,
            )?;
            Some(types.intern(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }))
        }
        TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        } => {
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty(types, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty(types, arg.ty, substitutions)?;
                    Some(arg)
                })
                .collect::<Option<Vec<_>>>()?;
            let associated_type_bindings = substitute_associated_type_bindings(
                types,
                associated_type_bindings,
                substitutions,
            )?;
            Some(types.intern(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }))
        }
        TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        } => {
            let self_ty = substitute_ty(types, self_ty, substitutions)?;
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty(types, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty(types, arg.ty, substitutions)?;
                    Some(arg)
                })
                .collect::<Option<Vec<_>>>()?;
            Some(types.intern(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }))
        }
        TyKind::Error
        | TyKind::ConstOnly
        | TyKind::Primitive(_)
        | TyKind::BuiltinType(_)
        | TyKind::Vector { .. }
        | TyKind::ClosureState { .. } => Some(ty),
    }
}

fn substitute_associated_type_bindings(
    types: ReachabilityTypeCx<'_>,
    bindings: Vec<AssociatedTypeBindingTy>,
    substitutions: &TypeSubstitutions<'_>,
) -> Option<Vec<AssociatedTypeBindingTy>> {
    bindings
        .into_iter()
        .map(|binding| {
            let trait_args = binding
                .trait_args
                .into_iter()
                .map(|arg| substitute_ty(types, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let trait_const_args = binding
                .trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty(types, arg.ty, substitutions)?;
                    Some(arg)
                })
                .collect::<Option<Vec<_>>>()?;
            let ty = substitute_ty(types, binding.ty, substitutions)?;
            Some(AssociatedTypeBindingTy {
                trait_id: binding.trait_id,
                trait_args,
                trait_const_args,
                name: binding.name,
                ty,
            })
        })
        .collect()
}

fn substitute_generic_ty(
    name: &SymbolId,
    substitutions: &TypeSubstitutions<'_>,
    fallback: InternedTyId,
) -> Option<InternedTyId> {
    match substitutions.generics {
        TypeSubstitutionGenerics::Local(generics) => generics.get(name).copied().or(Some(fallback)),
        TypeSubstitutionGenerics::Typed(generics) => generics
            .get(name)
            .copied()
            .map(|ty| match ty {
                SubstitutionTy::Canonical(ty) => ty,
            })
            .or(Some(fallback)),
    }
}
