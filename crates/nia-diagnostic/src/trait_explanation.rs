// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared wording for structured trait-obligation failures.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Semantic reason a trait obligation could not be explained as a type mismatch.
pub enum TraitObligationExplanationKind {
    /// A requirement was rejected without a more specific selection result.
    Unproven,
    /// No applicable implementation exists in the current program.
    NoImplementation,
    /// An implementation exists outside the current visibility scope.
    NotVisible,
    /// Several equally specific implementations apply.
    Ambiguous,
    /// Trait selection succeeded, but a further obligation or shape check failed.
    AdditionalConstraint,
    /// The required implementation cannot be used during constant evaluation.
    ConstUnavailable,
    /// The selected implementation does not define a required associated type.
    AssociatedTypeMissing,
    /// The selected implementation defines an incompatible associated type.
    AssociatedTypeMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Canonical summary, cause, and remediation for a failed trait obligation.
pub struct TraitObligationExplanation {
    /// Compact diagnostic summary.
    pub summary: String,
    /// Explanation of why resolution failed.
    pub note: String,
    /// Actionable next step.
    pub help: String,
}

/// Formats a trait-obligation failure consistently across compiler stages.
pub fn explain_trait_obligation(
    self_type: &str,
    trait_name: &str,
    kind: TraitObligationExplanationKind,
) -> TraitObligationExplanation {
    let summary = format!("trait bound not satisfied: {self_type}: {trait_name}");
    let (note, help) = match kind {
        TraitObligationExplanationKind::Unproven => (
            format!("the required `{trait_name}` obligation for `{self_type}` could not be proven in this context"),
            format!("check the required trait bounds and callable shape, or use a type that satisfies `{trait_name}`"),
        ),
        TraitObligationExplanationKind::NoImplementation => (
            format!("the compiler could not find a visible implementation of `{trait_name}` for `{self_type}`"),
            format!("`{self_type}` does not implement `{trait_name}`; provide an implementation of `{trait_name}` for `{self_type}`, or use a type with a matching implementation"),
        ),
        TraitObligationExplanationKind::NotVisible => (
            format!("an implementation of `{trait_name}` for `{self_type}` exists outside the current visibility scope"),
            format!("make the module that provides `{trait_name}` visible here, or use a type whose implementation is in scope"),
        ),
        TraitObligationExplanationKind::Ambiguous => (
            format!("multiple equally specific implementations of `{trait_name}` apply to `{self_type}`"),
            format!("adjust the implementations or type arguments so exactly one implementation of `{trait_name}` applies"),
        ),
        TraitObligationExplanationKind::AdditionalConstraint => (
            format!("an implementation of `{trait_name}` for `{self_type}` exists, but one of its required bounds is not satisfied"),
            "satisfy the implementation's additional trait bounds, or use a type with a matching implementation".to_string(),
        ),
        TraitObligationExplanationKind::ConstUnavailable => (
            format!("the required `{trait_name}` implementation for `{self_type}` is not available in this constant-evaluation context"),
            format!("use a const-capable implementation of `{trait_name}` or move this operation out of constant evaluation"),
        ),
        TraitObligationExplanationKind::AssociatedTypeMissing => (
            format!("the selected implementation for `{self_type}: {trait_name}` does not provide the required associated type"),
            "define the required associated type in the implementation or correct the associated type name".to_string(),
        ),
        TraitObligationExplanationKind::AssociatedTypeMismatch => (
            format!("the selected implementation for `{self_type}: {trait_name}` provides an incompatible associated type"),
            "make the associated type definition match the required type or update the bound".to_string(),
        ),
    };
    TraitObligationExplanation {
        summary,
        note,
        help,
    }
}

/// Explains a failed associated-type equality, retaining the member name and
/// the concrete types from the solver. `None` means the member was unresolved.
pub fn explain_associated_type_binding(
    trait_name: &str,
    member: &str,
    equality: Option<(&str, &str)>,
) -> TraitObligationExplanation {
    let projection = format!("{trait_name}::{member}");
    match equality {
        Some((expected, actual)) => TraitObligationExplanation {
            summary: format!(
                "associated type binding not satisfied: {projection} expected {expected}, got {actual}"
            ),
            note: format!(
                "the implementation resolves `{projection}` as `{actual}`, but this bound requires `{expected}`"
            ),
            help: format!(
                "change the associated type definition to `{expected}`, or update the bound to match `{actual}`"
            ),
        },
        None => TraitObligationExplanation {
            summary: format!(
                "associated type binding not satisfied: {projection} could not be resolved"
            ),
            note: format!(
                "the implementation for `{trait_name}` does not expose a resolvable associated type named `{member}`"
            ),
            help: format!(
                "define `{member}` in the implementation, use the correct associated type name, or remove the binding"
            ),
        },
    }
}
