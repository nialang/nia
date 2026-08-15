// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

use nia_pattern_analysis::{Constructor as AnalysisConstructor, Domain as AnalysisDomain};
use nia_pattern_analysis::{Pattern as AnalysisPattern, missing_witness, useful_witness};

mod coverage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConstPatternConstructor {
    Tuple,
    Pointer { is_readonly: bool },
    OptionalSome,
    OptionalNull,
    ErrorOk,
    ErrorErr,
    Struct(GlobalDefId),
    EnumVariant(GlobalDefId),
}

impl Analyzer<'_> {
    // Pattern analysis has three distinct phases. The mismatch probe reports
    // only contradictions it can prove, structural checking decides whether
    // an arm is typeable, and binding runs after that check to populate locals.
    pub(super) fn resolved_const_switch_arm_binds_pattern_locals(
        &self,
        arm: &nia_const_ir::ResolvedConstSwitchArm,
    ) -> bool {
        arm.patterns()
            .iter()
            .any(|pattern| resolved_pattern_local_id(pattern).is_some())
    }

    pub(super) fn resolved_const_switch_has_definite_pattern_mismatch(
        &mut self,
        switch: &ResolvedConstSwitch,
    ) -> bool {
        let Some(target_ty) = self.resolved_const_arg_runtime_type(switch.target(), None) else {
            return false;
        };
        switch.arms().iter().any(|arm| {
            self.resolved_const_patterns_have_definite_mismatch(arm.patterns(), target_ty)
        })
    }

    pub(super) fn resolved_const_patterns_have_definite_mismatch(
        &mut self,
        patterns: &[ResolvedConstPattern],
        target_ty: InternedTyId,
    ) -> bool {
        patterns
            .iter()
            .any(|pattern| self.resolved_const_pattern_has_definite_mismatch(pattern, target_ty))
    }

    fn resolved_const_pattern_has_definite_mismatch(
        &mut self,
        pattern: &ResolvedConstPattern,
        target_ty: InternedTyId,
    ) -> bool {
        match pattern.kind() {
            ResolvedConstPatternKind::Wildcard { .. } | ResolvedConstPatternKind::Bind { .. } => {
                false
            }
            ResolvedConstPatternKind::Pointer { pattern, .. }
            | ResolvedConstPatternKind::MutPointer { pattern, .. } => {
                let Some(TyKind::Pointer { elem, .. }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_const_pattern_has_definite_mismatch(pattern, elem)
            }
            ResolvedConstPatternKind::Expr(expr) => {
                let target_ty = ConstValueType::Runtime(target_ty);
                self.resolved_const_expr_type(expr, target_ty.runtime())
                    .or_else(|| self.resolved_const_expr_type(expr, None))
                    .is_some_and(|pattern_ty| {
                        pattern_ty != target_ty
                            && !self.const_equality_types_are_compatible(&target_ty, &pattern_ty)
                    })
            }
            ResolvedConstPatternKind::Range { start, end, .. } => {
                if !self.is_integer_runtime_type(target_ty) {
                    return true;
                }
                let start_ty = self.resolved_const_arg_runtime_type(start, Some(target_ty));
                let end_ty = self.resolved_const_arg_runtime_type(end, Some(target_ty));
                matches!(
                    (start_ty, end_ty),
                    (Some(start_ty), Some(end_ty))
                        if start_ty != target_ty || end_ty != target_ty
                )
            }
            ResolvedConstPatternKind::OptionalSome { pattern, .. } => {
                let Some(TyKind::Optional { elem }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_const_pattern_has_definite_mismatch(pattern, elem)
            }
            ResolvedConstPatternKind::OptionalNull { .. } => {
                !matches!(self.ty_kind(target_ty), Some(TyKind::Optional { .. }))
            }
            ResolvedConstPatternKind::ErrorOk { pattern, .. } => {
                let Some(TyKind::ErrorUnion { value, .. }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_const_pattern_has_definite_mismatch(pattern, value)
            }
            ResolvedConstPatternKind::ErrorErr { pattern, .. } => {
                let Some(TyKind::ErrorUnion { error, .. }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_const_pattern_has_definite_mismatch(pattern, error)
            }
            ResolvedConstPatternKind::Tuple { patterns, .. } => {
                let Some(TyKind::Tuple(elems)) = self.ty_kind(target_ty) else {
                    return true;
                };
                patterns.len() != elems.len()
                    || patterns.iter().zip(elems).any(|(pattern, elem)| {
                        self.resolved_const_pattern_has_definite_mismatch(pattern, elem)
                    })
            }
            ResolvedConstPatternKind::EnumVariant {
                variant, fields, ..
            } => {
                let Some(fields) =
                    self.resolved_const_enum_pattern_fields(variant, fields, target_ty)
                else {
                    return true;
                };
                fields.into_iter().any(|(pattern, ty)| {
                    self.resolved_const_pattern_has_definite_mismatch(pattern, ty)
                })
            }
            ResolvedConstPatternKind::Struct {
                def_id,
                fields,
                rest,
                ..
            } => {
                let Some(fields) =
                    self.resolved_const_struct_pattern_fields(*def_id, fields, *rest, target_ty)
                else {
                    return true;
                };
                fields.into_iter().any(|(pattern, ty)| {
                    self.resolved_const_pattern_has_definite_mismatch(pattern, ty)
                })
            }
        }
    }

    pub(super) fn check_resolved_const_patterns(
        &mut self,
        patterns: &[ResolvedConstPattern],
        target_ty: InternedTyId,
    ) -> Option<()> {
        for pattern in patterns {
            match pattern.kind() {
                ResolvedConstPatternKind::Wildcard { .. }
                | ResolvedConstPatternKind::Bind { .. } => {}
                ResolvedConstPatternKind::Pointer { pattern, .. }
                | ResolvedConstPatternKind::MutPointer { pattern, .. } => {
                    let Some(TyKind::Pointer { elem, .. }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_const_patterns(std::slice::from_ref(pattern), elem)?;
                }
                ResolvedConstPatternKind::Expr(expr) => {
                    let target_ty = ConstValueType::Runtime(target_ty);
                    let pattern_ty = self
                        .resolved_const_expr_type(expr, Some(target_ty.runtime()?))
                        .or_else(|| self.resolved_const_expr_type(expr, None))?;
                    if pattern_ty != target_ty
                        && !self.const_equality_types_are_compatible(&target_ty, &pattern_ty)
                    {
                        return None;
                    }
                }
                ResolvedConstPatternKind::Range { start, end, .. } => {
                    if !self.is_integer_runtime_type(target_ty) {
                        return None;
                    }
                    let start_ty = self.resolved_const_arg_runtime_type(start, Some(target_ty))?;
                    let end_ty = self.resolved_const_arg_runtime_type(end, Some(target_ty))?;
                    if start_ty != target_ty || end_ty != target_ty {
                        return None;
                    }
                }
                ResolvedConstPatternKind::OptionalSome { pattern, .. } => {
                    let Some(TyKind::Optional { elem }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_const_patterns(std::slice::from_ref(pattern), elem)?;
                }
                ResolvedConstPatternKind::OptionalNull { .. } => {
                    if !matches!(self.ty_kind(target_ty), Some(TyKind::Optional { .. })) {
                        return None;
                    }
                }
                ResolvedConstPatternKind::ErrorOk { pattern, .. } => {
                    let Some(TyKind::ErrorUnion { value, .. }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_const_patterns(std::slice::from_ref(pattern), value)?;
                }
                ResolvedConstPatternKind::ErrorErr { pattern, .. } => {
                    let Some(TyKind::ErrorUnion { error, .. }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_const_patterns(std::slice::from_ref(pattern), error)?;
                }
                ResolvedConstPatternKind::Tuple { patterns, .. } => {
                    let Some(TyKind::Tuple(elems)) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    if patterns.len() != elems.len() {
                        return None;
                    }
                    for (pattern, elem) in patterns.iter().zip(elems) {
                        self.check_resolved_const_patterns(std::slice::from_ref(pattern), elem)?;
                    }
                }
                ResolvedConstPatternKind::EnumVariant {
                    variant, fields, ..
                } => {
                    for (pattern, ty) in
                        self.resolved_const_enum_pattern_fields(variant, fields, target_ty)?
                    {
                        self.check_resolved_const_patterns(std::slice::from_ref(pattern), ty)?;
                    }
                }
                ResolvedConstPatternKind::Struct {
                    def_id,
                    fields,
                    rest,
                    ..
                } => {
                    for (pattern, ty) in self
                        .resolved_const_struct_pattern_fields(*def_id, fields, *rest, target_ty)?
                    {
                        self.check_resolved_const_patterns(std::slice::from_ref(pattern), ty)?;
                    }
                }
            }
        }
        Some(())
    }

    pub(super) fn bind_typed_resolved_const_patterns(
        &mut self,
        patterns: &[ResolvedConstPattern],
        target_ty: InternedTyId,
    ) -> Option<()> {
        for pattern in patterns {
            self.bind_typed_resolved_const_pattern(pattern, target_ty, false)?;
        }
        Some(())
    }

    pub(super) fn bind_typed_resolved_const_pattern(
        &mut self,
        pattern: &ResolvedConstPattern,
        target_ty: InternedTyId,
        is_mutable: bool,
    ) -> Option<()> {
        match pattern.kind() {
            ResolvedConstPatternKind::Bind { local_id, .. } => {
                self.bind_const_local_type(
                    *local_id,
                    ConstValueType::Runtime(target_ty),
                    is_mutable,
                );
            }
            ResolvedConstPatternKind::Pointer { pattern, .. }
            | ResolvedConstPatternKind::MutPointer { pattern, .. } => {
                let Some(TyKind::Pointer { elem, .. }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_const_pattern(pattern, elem, is_mutable)?;
            }
            ResolvedConstPatternKind::OptionalSome { pattern, .. } => {
                let Some(TyKind::Optional { elem }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_const_pattern(pattern, elem, is_mutable)?;
            }
            ResolvedConstPatternKind::ErrorOk { pattern, .. } => {
                let Some(TyKind::ErrorUnion { value, .. }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_const_pattern(pattern, value, is_mutable)?;
            }
            ResolvedConstPatternKind::ErrorErr { pattern, .. } => {
                let Some(TyKind::ErrorUnion { error, .. }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_const_pattern(pattern, error, is_mutable)?;
            }
            ResolvedConstPatternKind::Tuple { patterns, .. } => {
                let Some(TyKind::Tuple(elems)) = self.ty_kind(target_ty) else {
                    return None;
                };
                if patterns.len() != elems.len() {
                    return None;
                }
                for (pattern, elem) in patterns.iter().zip(elems) {
                    self.bind_typed_resolved_const_pattern(pattern, elem, is_mutable)?;
                }
            }
            ResolvedConstPatternKind::EnumVariant {
                variant, fields, ..
            } => {
                for (pattern, ty) in
                    self.resolved_const_enum_pattern_fields(variant, fields, target_ty)?
                {
                    self.bind_typed_resolved_const_pattern(pattern, ty, is_mutable)?;
                }
            }
            ResolvedConstPatternKind::Struct {
                def_id,
                fields,
                rest,
                ..
            } => {
                for (pattern, ty) in
                    self.resolved_const_struct_pattern_fields(*def_id, fields, *rest, target_ty)?
                {
                    self.bind_typed_resolved_const_pattern(pattern, ty, is_mutable)?;
                }
            }
            ResolvedConstPatternKind::Wildcard { .. }
            | ResolvedConstPatternKind::OptionalNull { .. }
            | ResolvedConstPatternKind::Expr(_)
            | ResolvedConstPatternKind::Range { .. } => {}
        }
        Some(())
    }
}
