// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use nia_span::Span;

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
}
