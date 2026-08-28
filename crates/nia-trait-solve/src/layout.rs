// SPDX-License-Identifier: GPL-3.0-or-later
//! Sizedness checks and equivalence against layout-owned type identities.

use super::*;

impl TraitSolver<'_> {
    pub(crate) fn layout_of(&self, ty: InternedTyId) -> bool {
        let ty = self.normalize(ty);
        if self.intrinsic_sized_shape(ty) {
            return true;
        }
        let Some(layouts) = self.layouts else {
            return false;
        };
        if layouts.types.contains_key(&ty) {
            return true;
        }
        if self.layout_types_contain_equivalent(ty, layouts) {
            return true;
        }
        match self.kind(ty) {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => layouts
                .nominal_type_layout_with_const_args(*def_id, args, const_args)
                .is_some(),
            _ => false,
        }
    }

    pub(crate) fn intrinsic_sized_shape(&self, ty: InternedTyId) -> bool {
        match self.kind(ty) {
            Some(TyKind::Primitive(PrimitiveTy::Never)) => false,
            Some(
                TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::FunctionPointer { .. }
                | TyKind::Callable { .. },
            ) => true,
            Some(TyKind::ClosureState { captures, .. }) => captures
                .iter()
                .all(|capture| self.intrinsic_sized_shape(*capture) || self.layout_of(*capture)),
            Some(
                TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::Slice { .. }
                | TyKind::Range { bound: None, .. },
            ) => true,
            Some(
                TyKind::SlicePointee { .. }
                | TyKind::TraitObjectPointee { .. }
                | TyKind::CallablePointee { .. }
                | TyKind::GenericParam(_),
            ) => false,
            Some(TyKind::Array {
                len: ArrayLenTy::ConstValue(_),
                elem,
            }) => self.intrinsic_sized_shape(*elem) || self.layout_of(*elem),
            Some(TyKind::Array { .. }) => false,
            Some(TyKind::Range {
                bound: Some(bound), ..
            })
            | Some(TyKind::Optional { elem: bound }) => {
                self.intrinsic_sized_shape(*bound) || self.layout_of(*bound)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                (self.intrinsic_sized_shape(*error) || self.layout_of(*error))
                    && (self.intrinsic_sized_shape(*value) || self.layout_of(*value))
            }
            _ => false,
        }
    }

    pub(crate) fn layout_types_contain_equivalent(
        &self,
        ty: InternedTyId,
        layouts: &Layouts,
    ) -> bool {
        layouts.types.keys().any(|layout_ty| {
            self.types_equivalent_in_layout_interner(ty, *layout_ty, layouts, &mut HashSet::new())
        })
    }

    pub(crate) fn types_equivalent_in_layout_interner(
        &self,
        left: InternedTyId,
        right: InternedTyId,
        layouts: &Layouts,
        seen: &mut HashSet<(InternedTyId, InternedTyId)>,
    ) -> bool {
        let left = self.normalize(left);
        // `right` may originate in the layout store's interner, so numeric IDs cannot establish
        // equality. Compare structure and memoize pairs to terminate on shared recursive shapes.
        if !seen.insert((left, right)) {
            return true;
        }
        match (self.interner.get(left), self.interner.get(right)) {
            (Some(TyKind::Error), Some(TyKind::Error)) => true,
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            (Some(TyKind::Tuple(left)), Some(TyKind::Tuple(right))) => {
                self.type_slices_equivalent_in_layout_interner(left, right, layouts, seen)
            }
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
                    && self.types_equivalent_in_layout_interner(
                        *left_elem,
                        *right_elem,
                        layouts,
                        seen,
                    )
            }
            (
                Some(TyKind::SlicePointee { elem: left_elem }),
                Some(TyKind::SlicePointee { elem: right_elem }),
            ) => self.types_equivalent_in_layout_interner(*left_elem, *right_elem, layouts, seen),
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
                self.array_lens_equivalent_in_layout_interner(left_len, right_len, layouts, seen)
                    && self.types_equivalent_in_layout_interner(
                        *left_elem,
                        *right_elem,
                        layouts,
                        seen,
                    )
            }
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
                    && match (left_bound, right_bound) {
                        (Some(left), Some(right)) => {
                            self.types_equivalent_in_layout_interner(*left, *right, layouts, seen)
                        }
                        (None, None) => true,
                        _ => false,
                    }
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
                    && self.type_slices_equivalent_in_layout_interner(
                        left_params,
                        right_params,
                        layouts,
                        seen,
                    )
                    && self.types_equivalent_in_layout_interner(
                        *left_return,
                        *right_return,
                        layouts,
                        seen,
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
                    && self.type_slices_equivalent_in_layout_interner(
                        left_params,
                        right_params,
                        layouts,
                        seen,
                    )
                    && self.types_equivalent_in_layout_interner(
                        *left_return,
                        *right_return,
                        layouts,
                        seen,
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
                self.type_slices_equivalent_in_layout_interner(
                    left_params,
                    right_params,
                    layouts,
                    seen,
                ) && self.types_equivalent_in_layout_interner(
                    *left_return,
                    *right_return,
                    layouts,
                    seen,
                )
            }
            (
                Some(TyKind::ClosureState {
                    closure_id: left_id,
                    captures: left_captures,
                    params: left_params,
                    return_type: left_return,
                }),
                Some(TyKind::ClosureState {
                    closure_id: right_id,
                    captures: right_captures,
                    params: right_params,
                    return_type: right_return,
                }),
            ) => {
                left_id == right_id
                    && self.type_slices_equivalent_in_layout_interner(
                        left_captures,
                        right_captures,
                        layouts,
                        seen,
                    )
                    && self.type_slices_equivalent_in_layout_interner(
                        left_params,
                        right_params,
                        layouts,
                        seen,
                    )
                    && self.types_equivalent_in_layout_interner(
                        *left_return,
                        *right_return,
                        layouts,
                        seen,
                    )
            }
            (Some(TyKind::Optional { elem: left }), Some(TyKind::Optional { elem: right })) => {
                self.types_equivalent_in_layout_interner(*left, *right, layouts, seen)
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
                self.types_equivalent_in_layout_interner(*left_error, *right_error, layouts, seen)
                    && self.types_equivalent_in_layout_interner(
                        *left_value,
                        *right_value,
                        layouts,
                        seen,
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
                    && self.const_args_equivalent_in_layout_interner(
                        left_const_args,
                        right_const_args,
                        layouts,
                        seen,
                    )
                    && self.type_slices_equivalent_in_layout_interner(
                        left_args, right_args, layouts, seen,
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
                    && self.type_slices_equivalent_in_layout_interner(
                        left_args, right_args, layouts, seen,
                    )
            }
            _ => false,
        }
    }

    pub(crate) fn type_slices_equivalent_in_layout_interner(
        &self,
        left: &[InternedTyId],
        right: &[InternedTyId],
        layouts: &Layouts,
        seen: &mut HashSet<(InternedTyId, InternedTyId)>,
    ) -> bool {
        left.len() == right.len()
            && left.iter().zip(right).all(|(left, right)| {
                self.types_equivalent_in_layout_interner(*left, *right, layouts, seen)
            })
    }

    pub(crate) fn array_lens_equivalent_in_layout_interner(
        &self,
        left: &ArrayLenTy,
        right: &ArrayLenTy,
        layouts: &Layouts,
        seen: &mut HashSet<(InternedTyId, InternedTyId)>,
    ) -> bool {
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
            ) => {
                left_builtin == right_builtin
                    && self.types_equivalent_in_layout_interner(*left_ty, *right_ty, layouts, seen)
            }
            _ => match (
                self.const_arg_from_array_len(left),
                self.const_arg_from_array_len(right),
            ) {
                (Some(left), Some(right)) => {
                    self.types_equivalent_in_layout_interner(left.ty, right.ty, layouts, seen)
                        && self.const_generic_values_equivalent(left.ty, &left.value, &right.value)
                }
                _ => false,
            },
        }
    }

    fn const_args_equivalent_in_layout_interner(
        &self,
        left: &[nia_ty::ConstGenericArg],
        right: &[nia_ty::ConstGenericArg],
        layouts: &Layouts,
        seen: &mut HashSet<(InternedTyId, InternedTyId)>,
    ) -> bool {
        left.len() == right.len()
            && left.iter().zip(right).all(|(left_arg, right_arg)| {
                self.types_equivalent_in_layout_interner(left_arg.ty, right_arg.ty, layouts, seen)
                    && match (&left_arg.value, &right_arg.value) {
                        (
                            nia_ty::ConstGenericValue::GenericParam(left),
                            nia_ty::ConstGenericValue::GenericParam(right),
                        ) => left == right,
                        (
                            nia_ty::ConstGenericValue::Int(left),
                            nia_ty::ConstGenericValue::Int(right),
                        ) => left.bits() == right.bits(),
                        (
                            nia_ty::ConstGenericValue::Bool(left),
                            nia_ty::ConstGenericValue::Bool(right),
                        ) => left == right,
                        (
                            nia_ty::ConstGenericValue::Char(left),
                            nia_ty::ConstGenericValue::Char(right),
                        ) => left == right,
                        (left, right) => {
                            self.const_generic_values_equivalent(left_arg.ty, left, right)
                        }
                    }
            })
    }

    pub(crate) fn is_generic_param(&self, ty: InternedTyId) -> bool {
        matches!(self.kind(ty), Some(TyKind::GenericParam(_)))
    }

    pub(crate) fn is_unsized_pointee(&self, ty: InternedTyId) -> bool {
        matches!(
            self.kind(ty),
            Some(
                TyKind::SlicePointee { .. }
                    | TyKind::TraitObjectPointee { .. }
                    | TyKind::CallablePointee { .. }
            )
        )
    }

    pub(crate) fn bool(&self) -> InternedTyId {
        self.interner.primitive(PrimitiveTy::Bool)
    }

    pub(crate) fn usize(&self) -> InternedTyId {
        self.interner.primitive(PrimitiveTy::Usize)
    }

    pub(crate) fn is_unit(&self, ty: InternedTyId) -> bool {
        self.kind(ty).is_some_and(TyKind::is_unit)
    }

    pub(crate) fn is_numeric(&self, ty: InternedTyId) -> bool {
        match self.kind(ty) {
            Some(TyKind::Primitive(primitive))
            | Some(TyKind::Vector {
                elem: primitive, ..
            }) => primitive.is_integer() || primitive.is_float(),
            _ => false,
        }
    }

    pub(crate) fn intrinsic_shift_impl_exists(
        &mut self,
        self_ty: InternedTyId,
        rhs_ty: InternedTyId,
    ) -> bool {
        match self.kind(self_ty) {
            Some(TyKind::Primitive(primitive)) if primitive.is_integer() => {
                matches!(self.kind(rhs_ty), Some(TyKind::Primitive(rhs)) if rhs.is_integer())
            }
            Some(TyKind::Vector { elem, .. }) if elem.is_integer() => {
                self.types_equivalent(self_ty, rhs_ty)
            }
            _ => false,
        }
    }

    pub(crate) fn is_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.kind(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
            )) | Some(TyKind::Vector {
                elem: PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
                    | PrimitiveTy::Bool,
                ..
            })
        )
    }

    pub(crate) fn is_char(&self, ty: InternedTyId) -> bool {
        matches!(self.kind(ty), Some(TyKind::Primitive(PrimitiveTy::Char)))
    }

    pub(crate) fn is_pointer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.kind(ty),
            Some(
                TyKind::Pointer { .. }
                    | TyKind::VolatilePointer { .. }
                    | TyKind::FunctionPointer { .. }
            )
        )
    }
}

impl TypeEquivalence for TraitSolver<'_> {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.interner.get(ty)
    }

    fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
        if left == right {
            return true;
        }
        match (left, right) {
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
                    && self.structural_types_equivalent(*left_ty, *right_ty)
            }
            _ => match (
                self.const_arg_from_array_len(left),
                self.const_arg_from_array_len(right),
            ) {
                (Some(left), Some(right)) => {
                    self.structural_types_equivalent(left.ty, right.ty)
                        && self.const_generic_values_equivalent(left.ty, &left.value, &right.value)
                }
                _ => false,
            },
        }
    }

    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        self.structural_types_equivalent(left, right)
    }

    fn same_const_generic_args_for_equiv(
        &self,
        left: &[ConstGenericArg],
        right: &[ConstGenericArg],
    ) -> bool {
        left.len() == right.len()
            && left.iter().zip(right).all(|(left, right)| {
                self.structural_types_equivalent(left.ty, right.ty)
                    && match (&left.value, &right.value) {
                        (ConstGenericValue::Int(left), ConstGenericValue::Int(right)) => {
                            left.bits() == right.bits()
                        }
                        (left, right) => left == right,
                    }
            })
    }
}
