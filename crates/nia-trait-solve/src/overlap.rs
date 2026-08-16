// SPDX-License-Identifier: GPL-3.0-or-later
//! Conservative overlap checks for compiler-provided trait implementations.
//!
//! These predicates answer whether an unresolved type pattern *could* match a
//! builtin implementation. They intentionally over-approximate generic cases;
//! rejecting a possible overlap here would incorrectly accept an ambiguous program.

use super::*;

/// Conservative classifier used to reject source impls that may overlap an
/// implementation supplied by the compiler.
pub struct IntrinsicOverlap<'a, F>
where
    F: Fn(InternedTyId) -> bool,
{
    /// Canonical store containing the impl's type pattern.
    pub type_store: &'a TypeStore,
    /// Normalization product applied before structural classification.
    pub normalization: &'a TypeNormalization,
    /// Program-wide nominal enum classifier.
    pub is_enum: F,
}

impl<'a, F> IntrinsicOverlap<'a, F>
where
    F: Fn(InternedTyId) -> bool,
{
    /// Returns whether a source pattern could match the builtin trait's domain.
    ///
    /// Generic parameters are treated as possible matches. A `true` result is
    /// intentionally conservative and does not claim that a concrete witness
    /// exists.
    pub fn overlaps_builtin_trait(
        &self,
        self_ty: InternedTyId,
        trait_id: BuiltinTrait,
        trait_args: &[InternedTyId],
    ) -> bool {
        let self_ty = self.normalize(self_ty);
        match trait_id {
            BuiltinTrait::Add
            | BuiltinTrait::Sub
            | BuiltinTrait::Mul
            | BuiltinTrait::Div
            | BuiltinTrait::Rem => {
                let Some(rhs_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_numeric(self_ty) && self.patterns_can_match(self_ty, rhs_ty)
            }
            BuiltinTrait::BitAnd | BuiltinTrait::BitOr | BuiltinTrait::BitXor => {
                let Some(rhs_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_integer(self_ty) && self.patterns_can_match(self_ty, rhs_ty)
            }
            BuiltinTrait::Shl | BuiltinTrait::Shr => {
                let Some(rhs_ty) = trait_args.first().copied() else {
                    return false;
                };
                match self.type_store.get(self.normalize(self_ty)) {
                    Some(TyKind::GenericParam(_)) => self.can_be_integer(rhs_ty),
                    Some(TyKind::Primitive(primitive)) if primitive.is_integer() => {
                        match self.type_store.get(self.normalize(rhs_ty)) {
                            Some(TyKind::GenericParam(_)) => true,
                            Some(TyKind::Primitive(rhs)) => rhs.is_integer(),
                            _ => false,
                        }
                    }
                    Some(TyKind::Vector { elem, .. }) if elem.is_integer() => {
                        self.patterns_can_match(self_ty, rhs_ty)
                    }
                    _ => false,
                }
            }
            BuiltinTrait::Neg => self.can_be_numeric(self_ty),
            BuiltinTrait::BitNot => self.can_be_integer(self_ty),
            BuiltinTrait::Not => self.can_be_bool(self_ty),
            BuiltinTrait::Eq => {
                let Some(rhs_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.patterns_can_match(self_ty, rhs_ty)
                    && (self.can_be_numeric(self_ty)
                        || self.can_be_bool(self_ty)
                        || self.can_be_char(self_ty)
                        || self.can_be_pointer(self_ty)
                        || self.can_be_enum(self_ty))
            }
            BuiltinTrait::Ord => {
                let Some(rhs_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.patterns_can_match(self_ty, rhs_ty)
                    && (self.can_be_numeric(self_ty) || self.can_be_char(self_ty))
            }
            BuiltinTrait::Sized => self.can_have_known_layout(self_ty),
            BuiltinTrait::Unsized => self.can_be_compiler_classified_type(self_ty),
            BuiltinTrait::Deref => self.can_be_non_unit_pointer(self_ty, false),
            BuiltinTrait::DerefMut => self.can_be_non_unit_pointer(self_ty, true),
            BuiltinTrait::Index => {
                let Some(index_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, false) && self.can_be_integer(index_ty)
            }
            BuiltinTrait::IndexMut => {
                let Some(index_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, true) && self.can_be_integer(index_ty)
            }
            BuiltinTrait::Slice => {
                let Some(range_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, false)
                    && self.can_be_usize_range(range_ty)
            }
            BuiltinTrait::SliceMut => {
                let Some(range_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, true)
                    && self.can_be_usize_range(range_ty)
            }
            BuiltinTrait::Iterable => false,
            BuiltinTrait::Iterator => false,
            BuiltinTrait::Simd => self.can_be_simd(self_ty),
            BuiltinTrait::SimdMask => self.can_be_simd_mask(self_ty),
        }
    }

    fn patterns_can_match(&self, left: InternedTyId, right: InternedTyId) -> bool {
        let left = self.normalize(left);
        let right = self.normalize(right);
        if self.types_equivalent(left, right) {
            return true;
        }
        match (self.type_store.get(left), self.type_store.get(right)) {
            (Some(TyKind::GenericParam(_)), _) | (_, Some(TyKind::GenericParam(_))) => true,
            (
                Some(TyKind::Pointer {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Slice {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            ) => left_const == right_const && self.patterns_can_match(*left_elem, *right_elem),
            (
                Some(TyKind::SlicePointee { elem: left_elem }),
                Some(TyKind::SlicePointee { elem: right_elem }),
            ) => self.patterns_can_match(*left_elem, *right_elem),
            (
                Some(TyKind::Array {
                    elem: left_elem, ..
                }),
                Some(TyKind::Array {
                    elem: right_elem, ..
                }),
            ) => self.patterns_can_match(*left_elem, *right_elem),
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
                        (Some(left_bound), Some(right_bound)) => {
                            self.patterns_can_match(*left_bound, *right_bound)
                        }
                        (None, None) => true,
                        _ => false,
                    }
            }
            _ => false,
        }
    }

    fn types_equivalent(&self, left: InternedTyId, right: InternedTyId) -> bool {
        let left = self.normalize(left);
        let right = self.normalize(right);
        if left == right {
            return true;
        }
        match (self.type_store.get(left), self.type_store.get(right)) {
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            _ => false,
        }
    }

    fn can_be_numeric(&self, ty: InternedTyId) -> bool {
        match self.type_store.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Primitive(primitive))
            | Some(TyKind::Vector {
                elem: primitive, ..
            }) => primitive.is_integer() || primitive.is_float(),
            _ => false,
        }
    }

    fn can_be_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_))
                | Some(TyKind::Primitive(
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
                ))
                | Some(TyKind::Vector {
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

    fn can_be_bool(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_)) | Some(TyKind::Primitive(PrimitiveTy::Bool))
        )
    }

    fn can_be_char(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_)) | Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    fn can_be_simd(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_)) | Some(TyKind::Vector { .. })
        )
    }

    fn can_be_simd_mask(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_))
                | Some(TyKind::Vector {
                    elem: PrimitiveTy::Bool,
                    lanes: 0..=64
                })
        )
    }

    fn can_be_pointer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_))
                | Some(
                    TyKind::Pointer { .. }
                        | TyKind::VolatilePointer { .. }
                        | TyKind::FunctionPointer { .. },
                )
        )
    }

    fn can_be_enum(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_))
        ) || (self.is_enum)(ty)
    }

    fn can_have_known_layout(&self, ty: InternedTyId) -> bool {
        !matches!(
            self.kind(ty),
            Some(TyKind::Error | TyKind::Primitive(PrimitiveTy::Never))
        )
    }

    fn can_be_compiler_classified_type(&self, ty: InternedTyId) -> bool {
        !matches!(self.kind(ty), Some(TyKind::Error))
    }

    fn can_be_non_unit_pointer(&self, ty: InternedTyId, mutable: bool) -> bool {
        match self.type_store.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Pointer { is_readonly, elem })
            | Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                (!mutable || !*is_readonly)
                    && !self
                        .type_store
                        .get(self.normalize(*elem))
                        .is_some_and(TyKind::is_unit)
            }
            _ => false,
        }
    }

    fn can_be_array_pointer_or_slice(&self, ty: InternedTyId, mutable: bool) -> bool {
        match self.type_store.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) | Some(TyKind::Array { .. }) => true,
            Some(
                TyKind::Pointer { is_readonly, .. }
                | TyKind::VolatilePointer { is_readonly, .. }
                | TyKind::Slice { is_readonly, .. },
            ) => !mutable || !*is_readonly,
            _ => false,
        }
    }

    fn can_be_usize_range(&self, ty: InternedTyId) -> bool {
        match self.type_store.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Range { bound: None, .. }) => true,
            Some(TyKind::Range {
                bound: Some(bound), ..
            }) => matches!(
                self.type_store.get(self.normalize(*bound)),
                Some(TyKind::GenericParam(_)) | Some(TyKind::Primitive(PrimitiveTy::Usize))
            ),
            _ => false,
        }
    }

    fn normalize(&self, ty: InternedTyId) -> InternedTyId {
        self.normalization.normalize(ty)
    }

    fn kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        let ty = self.normalize(ty);
        self.type_store.get(ty)
    }
}
