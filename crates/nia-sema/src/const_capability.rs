// SPDX-License-Identifier: GPL-3.0-or-later
//! Value capabilities required by compile-time execution.

use nia_ty::TyKind;

/// A value representation that compile-time execution cannot materialize.
///
/// This is independent of ordinary typing and callable coercion: a well-typed
/// runtime value can still require a representation the const engine lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstValueRestriction {
    /// Function items, closure state, thin pointers, and callable views.
    Callable,
}

impl ConstValueRestriction {
    /// The diagnostic shared by checking and execution of this capability.
    pub const fn message(self) -> &'static str {
        match self {
            Self::Callable => "callable values are not available during const evaluation",
        }
    }
}

/// Classifies a normalized value type after ordinary typing and adjustment.
///
/// Do not inspect a callable's signature or recursively reject its containing
/// types: `null` optional callbacks and empty callback arrays materialize no
/// callable value. Their populated forms check the actual payload expressions.
pub fn const_value_restriction(ty: &TyKind) -> Option<ConstValueRestriction> {
    match ty {
        TyKind::FunctionItem { .. }
        | TyKind::ClosureState { .. }
        | TyKind::FunctionPointer { .. }
        | TyKind::Callable { .. }
        | TyKind::CallablePointee { .. } => Some(ConstValueRestriction::Callable),
        _ => None,
    }
}
