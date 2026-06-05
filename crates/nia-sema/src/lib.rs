// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use nia_span::Span;
use nia_ty::ArrayLenTy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrayLiteralLenCheck {
    Accepted(ArrayLenTy),
    Mismatch { expected: u64, actual: u64 },
    Unknown,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArityRequirement {
    Exact(usize),
    AtLeast(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArityCheck {
    Accepted,
    Mismatch {
        requirement: ArityRequirement,
        actual: usize,
    },
}

impl ArityCheck {
    pub fn is_accepted(self) -> bool {
        matches!(self, Self::Accepted)
    }
}

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

pub fn check_exact_arity(expected: usize, actual: usize) -> ArityCheck {
    check_arity(ArityRequirement::Exact(expected), actual)
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedField<Key> {
    pub span: Span,
    pub name: Key,
}

impl<Key> NamedField<Key> {
    pub const fn new(span: Span, name: Key) -> Self {
        Self { span, name }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSetCheck<Key> {
    pub duplicate_fields: Vec<NamedField<Key>>,
    pub unknown_fields: Vec<NamedField<Key>>,
    pub missing_fields: Vec<Key>,
}

impl<Key> FieldSetCheck<Key> {
    pub fn is_valid(&self) -> bool {
        self.duplicate_fields.is_empty()
            && self.unknown_fields.is_empty()
            && self.missing_fields.is_empty()
    }
}

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

pub fn check_map_field_set<Key, Value>(
    actual: &HashMap<Key, Value>,
    expected: impl IntoIterator<Item = Key>,
) -> FieldSetCheck<Key>
where
    Key: Clone + Eq + Hash,
{
    check_value_field_set(actual.keys().cloned(), expected)
}

pub trait BorrowedNamedField<Key> {
    fn span(&self) -> Span;
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
