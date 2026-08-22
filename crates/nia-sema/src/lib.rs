// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared semantic validation primitives for array lengths, arity, and fields.
//!
//! These helpers return structured results and leave diagnostic wording and
//! phase-specific recovery to their callers.
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use nia_span::Span;
use nia_ty::ArrayLenTy;

/// Result of reconciling an array literal length with an expected type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrayLiteralLenCheck {
    /// Length is valid and resolves to the returned type-level length.
    Accepted(ArrayLenTy),
    /// Known expected and actual lengths differ.
    Mismatch {
        /// Required element count.
        expected: u64,
        /// Literal element count.
        actual: u64,
    },
    /// Neither context nor literal provides a concrete length yet.
    Unknown,
}

/// Checks or infers an array literal's type-level length.
pub fn check_array_literal_len(
    expected: Option<ArrayLenTy>,
    expected_value: Option<u64>,
    actual: Option<u64>,
) -> ArrayLiteralLenCheck {
    match (expected, expected_value, actual) {
        (Some(ArrayLenTy::Infer), _, None) | (None, _, None) => ArrayLiteralLenCheck::Unknown,
        (Some(expected), _, None) => ArrayLiteralLenCheck::Accepted(expected),
        (Some(ArrayLenTy::Infer), _, Some(actual)) | (None, _, Some(actual)) => {
            ArrayLiteralLenCheck::Accepted(ArrayLenTy::ConstValue(actual))
        }
        (Some(expected @ ArrayLenTy::ConstValue(expected_value)), _, Some(actual))
            if expected_value == actual =>
        {
            ArrayLiteralLenCheck::Accepted(expected)
        }
        (Some(ArrayLenTy::ConstValue(expected)), _, Some(actual)) => {
            ArrayLiteralLenCheck::Mismatch { expected, actual }
        }
        (Some(_), Some(expected_value), Some(actual)) if expected_value != actual => {
            ArrayLiteralLenCheck::Mismatch {
                expected: expected_value,
                actual,
            }
        }
        (Some(expected), _, Some(_)) => ArrayLiteralLenCheck::Accepted(expected),
    }
}

/// Required relationship between expected and actual argument counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArityRequirement {
    /// Actual count must equal the value.
    Exact(usize),
    /// Actual count must be at least the value.
    AtLeast(usize),
}

/// Result of checking an argument or element count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArityCheck {
    /// Count satisfies the requirement.
    Accepted,
    /// Count does not satisfy the requirement.
    Mismatch {
        /// Requirement that was checked.
        requirement: ArityRequirement,
        /// Actual count observed.
        actual: usize,
    },
}

impl ArityCheck {
    /// Reports whether the checked count was accepted.
    pub fn is_accepted(self) -> bool {
        matches!(self, Self::Accepted)
    }
}

/// Checks an actual count against an arity requirement.
pub fn check_arity(requirement: ArityRequirement, actual: usize) -> ArityCheck {
    let accepted = match requirement {
        ArityRequirement::Exact(expected) => actual == expected,
        ArityRequirement::AtLeast(expected) => actual >= expected,
    };
    if accepted {
        ArityCheck::Accepted
    } else {
        ArityCheck::Mismatch {
            requirement,
            actual,
        }
    }
}

/// Checks that `actual` equals `expected`.
pub fn check_exact_arity(expected: usize, actual: usize) -> ArityCheck {
    check_arity(ArityRequirement::Exact(expected), actual)
}

/// Checks call arguments using exact or variadic minimum arity.
pub fn check_call_arity(
    required_params: usize,
    actual_args: usize,
    is_variadic: bool,
) -> ArityCheck {
    let requirement = if is_variadic {
        ArityRequirement::AtLeast(required_params)
    } else {
        ArityRequirement::Exact(required_params)
    };
    check_arity(requirement, actual_args)
}

/// Field name paired with the source span used for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedField<Key> {
    /// Source span of the field occurrence.
    pub span: Span,
    /// Comparable field identity.
    pub name: Key,
}

impl<Key> NamedField<Key> {
    /// Creates a spanned field identity.
    pub const fn new(span: Span, name: Key) -> Self {
        Self { span, name }
    }
}

/// Duplicate, unknown, and missing results from a field-set check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSetCheck<Key> {
    /// Repeated occurrences after the first field of each name.
    pub duplicate_fields: Vec<NamedField<Key>>,
    /// Supplied fields absent from the expected set.
    pub unknown_fields: Vec<NamedField<Key>>,
    /// Expected fields absent from the supplied set.
    pub missing_fields: Vec<Key>,
}

impl<Key> FieldSetCheck<Key> {
    /// Reports whether all three error collections are empty.
    pub fn is_valid(&self) -> bool {
        self.duplicate_fields.is_empty()
            && self.unknown_fields.is_empty()
            && self.missing_fields.is_empty()
    }
}

/// Checks supplied fields for duplicates, unknown names, and missing requirements.
pub fn check_required_field_set<Key, FieldName, ExpectedName>(
    fields: impl IntoIterator<Item = FieldName>,
    expected: impl IntoIterator<Item = ExpectedName>,
) -> FieldSetCheck<Key>
where
    Key: Clone + Eq + Hash,
    FieldName: BorrowedNamedField<Key>,
    ExpectedName: Into<Key>,
{
    let mut seen = HashSet::new();
    let mut present = HashSet::new();
    let mut actual = Vec::new();
    let mut duplicate_fields = Vec::new();
    for field in fields {
        let name = field.name().clone();
        if !seen.insert(name.clone()) {
            duplicate_fields.push(NamedField::new(field.span(), name.clone()));
        }
        present.insert(name.clone());
        actual.push(NamedField::new(field.span(), name));
    }

    let mut expected_set = HashSet::new();
    let mut expected_names = Vec::new();
    for name in expected {
        let name = name.into();
        expected_set.insert(name.clone());
        expected_names.push(name);
    }

    let unknown_fields = actual
        .into_iter()
        .filter(|field| !expected_set.contains(&field.name))
        .collect();
    let missing_fields = expected_names
        .into_iter()
        .filter(|name| !present.contains(name))
        .collect();

    FieldSetCheck {
        duplicate_fields,
        unknown_fields,
        missing_fields,
    }
}

/// Returns repeated field occurrences after each name's first occurrence.
pub fn check_unique_field_set<Key, FieldName>(
    fields: impl IntoIterator<Item = FieldName>,
) -> Vec<NamedField<Key>>
where
    Key: Clone + Eq + Hash,
    FieldName: BorrowedNamedField<Key>,
{
    let mut seen = HashSet::new();
    let mut duplicate_fields = Vec::new();
    for field in fields {
        let name = field.name().clone();
        if !seen.insert(name.clone()) {
            duplicate_fields.push(NamedField::new(field.span(), name));
        }
    }
    duplicate_fields
}

/// Checks unspanned field identities against a required field set.
pub fn check_value_field_set<Key, ActualName, ExpectedName>(
    actual: impl IntoIterator<Item = ActualName>,
    expected: impl IntoIterator<Item = ExpectedName>,
) -> FieldSetCheck<Key>
where
    Key: Clone + Eq + Hash,
    ActualName: Into<Key>,
    ExpectedName: Into<Key>,
{
    let actual = actual
        .into_iter()
        .map(|name| NamedField::new(Span::default(), name.into()));
    check_required_field_set(actual, expected)
}

/// Checks the keys of a map against a required field set.
pub fn check_map_field_set<Key, Value>(
    actual: &HashMap<Key, Value>,
    expected: impl IntoIterator<Item = Key>,
) -> FieldSetCheck<Key>
where
    Key: Clone + Eq + Hash,
{
    check_value_field_set(actual.keys().cloned(), expected)
}

/// Borrowed access to a field identity and its diagnostic span.
pub trait BorrowedNamedField<Key> {
    /// Returns the source span of this field occurrence.
    fn span(&self) -> Span;
    /// Returns the comparable field identity.
    fn name(&self) -> &Key;
}

impl<Key> BorrowedNamedField<Key> for NamedField<Key> {
    fn span(&self) -> Span {
        self.span
    }

    fn name(&self) -> &Key {
        &self.name
    }
}

impl<Key> BorrowedNamedField<Key> for &NamedField<Key> {
    fn span(&self) -> Span {
        self.span
    }

    fn name(&self) -> &Key {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_field_set_reports_duplicates_unknown_and_missing() {
        let actual = [
            NamedField::new(Span::new(0, 1), "x"),
            NamedField::new(Span::new(2, 3), "x"),
            NamedField::new(Span::new(4, 5), "z"),
        ];
        let checked = check_required_field_set(actual.iter(), ["x", "y"]);

        assert_eq!(
            checked.duplicate_fields,
            vec![NamedField::new(Span::new(2, 3), "x")]
        );
        assert_eq!(
            checked.unknown_fields,
            vec![NamedField::new(Span::new(4, 5), "z")]
        );
        assert_eq!(checked.missing_fields, vec!["y"]);
    }

    #[test]
    fn array_literal_len_infers_checks_and_reports_mismatch() {
        assert_eq!(
            check_array_literal_len(None, None, Some(3)),
            ArrayLiteralLenCheck::Accepted(ArrayLenTy::ConstValue(3))
        );
        assert_eq!(
            check_array_literal_len(Some(ArrayLenTy::ConstValue(2)), None, Some(3)),
            ArrayLiteralLenCheck::Mismatch {
                expected: 2,
                actual: 3
            }
        );
        assert_eq!(
            check_array_literal_len(Some(ArrayLenTy::Infer), None, None),
            ArrayLiteralLenCheck::Unknown
        );
    }

    #[test]
    fn arity_checks_exact_and_variadic_requirements() {
        assert_eq!(check_exact_arity(2, 2), ArityCheck::Accepted);
        assert_eq!(
            check_exact_arity(2, 3),
            ArityCheck::Mismatch {
                requirement: ArityRequirement::Exact(2),
                actual: 3
            }
        );
        assert_eq!(check_call_arity(2, 3, true), ArityCheck::Accepted);
        assert_eq!(
            check_call_arity(2, 1, true),
            ArityCheck::Mismatch {
                requirement: ArityRequirement::AtLeast(2),
                actual: 1
            }
        );
    }
}
