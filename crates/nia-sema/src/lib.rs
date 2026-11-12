// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared semantic validation primitives for value capabilities and type shapes.
//!
//! These helpers return structured results and leave diagnostic wording and
//! phase-specific recovery to their callers.
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use nia_ids::InternedTyId;
use nia_span::Span;
use nia_ty::{ArrayLenTy, TyKind};

mod const_capability;
pub use const_capability::{ConstValueRestriction, const_value_restriction};

/// Type-shape result for implicit address formation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplicitAddressPlan {
    /// The value is addressed directly as the expected pointer target.
    Direct {
        /// Whether the generated pointer forbids mutation.
        pointer_is_readonly: bool,
    },
    /// The value is addressed as an array and then viewed as a slice.
    ArrayToSlice {
        /// Mutability of the temporary pointer to the array.
        pointer_is_readonly: bool,
        /// Mutability of the resulting slice view.
        slice_is_readonly: bool,
    },
}

/// Classifies the shared type rule for implicitly addressing a value.
///
/// Callers provide normalized type shapes and their phase-specific type
/// equivalence predicate. Expression-place checks, adjustment recording, and
/// const-value materialization deliberately remain outside this helper.
pub fn classify_implicit_address<F>(
    expected: &TyKind,
    expected_elem_kind: Option<&TyKind>,
    actual_ty: InternedTyId,
    actual: &TyKind,
    mut types_match: F,
) -> Option<ImplicitAddressPlan>
where
    F: FnMut(InternedTyId, InternedTyId) -> bool,
{
    match expected {
        TyKind::Pointer {
            is_readonly,
            elem: expected_elem,
        } if types_match(actual_ty, *expected_elem) => Some(ImplicitAddressPlan::Direct {
            pointer_is_readonly: *is_readonly,
        }),
        TyKind::Pointer {
            is_readonly: pointer_is_readonly,
            ..
        }
        | TyKind::Slice {
            is_readonly: pointer_is_readonly,
            ..
        } => {
            let TyKind::Array {
                elem: actual_array_elem,
                ..
            } = actual
            else {
                return None;
            };
            let (slice_is_readonly, expected_slice_elem) = match expected {
                TyKind::Pointer { .. } => match expected_elem_kind? {
                    TyKind::Slice { is_readonly, elem } => (*is_readonly, *elem),
                    _ => return None,
                },
                TyKind::Slice { is_readonly, elem } => (*is_readonly, *elem),
                _ => unreachable!(),
            };
            if !types_match(*actual_array_elem, expected_slice_elem)
                || (!*pointer_is_readonly && slice_is_readonly)
            {
                return None;
            }
            Some(ImplicitAddressPlan::ArrayToSlice {
                pointer_is_readonly: *pointer_is_readonly,
                slice_is_readonly,
            })
        }
        _ => None,
    }
}

/// Returns the element type for a slice-method receiver, including arrays
/// that can be addressed and array pointers that can be viewed as slices.
/// Callers supply normalized shapes and check expression addressability.
pub fn slice_method_receiver_element(
    receiver: &TyKind,
    pointer_element: Option<&TyKind>,
    slice_is_readonly: bool,
) -> Option<InternedTyId> {
    match receiver {
        TyKind::Array { elem, .. } => Some(*elem),
        TyKind::Slice { is_readonly, elem } if slice_is_readonly || !is_readonly => Some(*elem),
        TyKind::Pointer { is_readonly, .. } if slice_is_readonly || !is_readonly => {
            match pointer_element? {
                TyKind::Array { elem, .. } => Some(*elem),
                _ => None,
            }
        }
        _ => None,
    }
}

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

/// How a constructor spells the type it builds.
///
/// Constructors take their type's generic arguments from what they write, not
/// from inference: `Pair[i32, bool] { .. }`, `Wrapper[i32](1)` and
/// `Choice[i32]::Some(1)` name their instance, and only the omitted forms
/// `.{ .. }` and `.Some(1)` take it from the expected type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructorSpelling {
    /// `.Some(1)`, `.None`, `.{ .. }`: the type is the expected one.
    Omitted,
    /// `E::B`, `E[i32]::A(1)`, `T2(1)`, `Alias::A`: a prefix names the type,
    /// with `written` generic arguments after it. An alias prefix counts its
    /// own parameters, not its target's.
    Prefix {
        /// Generic arguments written after the prefix.
        written: usize,
    },
}

/// Where a constructor's generic arguments come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructorGenericArgs {
    /// The prefix names the whole instance: its arguments are written, or
    /// the type has none.
    Written,
    /// The omitted form takes the instance from the expected type.
    FromExpected,
    /// The prefix does not write the arguments its type declares.
    Mismatch {
        /// Parameters the named type declares.
        expected: usize,
        /// Arguments written after the prefix.
        actual: usize,
    },
}

/// Classifies a constructor's generic arguments. This is the one statement
/// of the rule; every phase that types or reports a constructor asks it, so
/// const and runtime code cannot disagree on which spellings infer.
pub fn constructor_generic_args(
    spelling: ConstructorSpelling,
    declared: usize,
) -> ConstructorGenericArgs {
    match spelling {
        ConstructorSpelling::Omitted => ConstructorGenericArgs::FromExpected,
        ConstructorSpelling::Prefix { written } if written == declared => {
            ConstructorGenericArgs::Written
        }
        ConstructorSpelling::Prefix { written } => ConstructorGenericArgs::Mismatch {
            expected: declared,
            actual: written,
        },
    }
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
    fn constructors_infer_only_in_omitted_form() {
        use ConstructorGenericArgs::{FromExpected, Mismatch, Written};
        use ConstructorSpelling::{Omitted, Prefix};
        assert_eq!(constructor_generic_args(Omitted, 1), FromExpected);
        assert_eq!(constructor_generic_args(Omitted, 0), FromExpected);
        assert_eq!(constructor_generic_args(Prefix { written: 0 }, 0), Written);
        assert_eq!(constructor_generic_args(Prefix { written: 2 }, 2), Written);
        assert_eq!(
            constructor_generic_args(Prefix { written: 0 }, 1),
            Mismatch {
                expected: 1,
                actual: 0
            }
        );
        assert_eq!(
            constructor_generic_args(Prefix { written: 3 }, 2),
            Mismatch {
                expected: 2,
                actual: 3
            }
        );
    }

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
        let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
        let module_id = module_ids.allocate().expect("allocate module ID");
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
        assert_eq!(
            check_array_literal_len(Some(ArrayLenTy::ConstValue(4)), None, None),
            ArrayLiteralLenCheck::Accepted(ArrayLenTy::ConstValue(4))
        );
        assert_eq!(
            check_array_literal_len(Some(ArrayLenTy::Infer), None, Some(4)),
            ArrayLiteralLenCheck::Accepted(ArrayLenTy::ConstValue(4))
        );
        assert_eq!(
            check_array_literal_len(Some(ArrayLenTy::ConstValue(4)), Some(4), Some(4)),
            ArrayLiteralLenCheck::Accepted(ArrayLenTy::ConstValue(4))
        );
        assert_eq!(
            check_array_literal_len(
                Some(ArrayLenTy::ConstExpr(nia_ids::GlobalConstExprId {
                    module_id,
                    const_expr_id: nia_ids::ConstExprId::synthetic(0),
                })),
                Some(9),
                Some(4),
            ),
            ArrayLiteralLenCheck::Mismatch {
                expected: 9,
                actual: 4
            }
        );
    }

    #[test]
    fn implicit_address_classification_shares_pointer_and_slice_shapes() {
        let store = nia_ty::TypeStore::new().expect("create type store");
        let append = store.append_for_module(module_id_for_test());
        let usize_ty = append
            .primitive(nia_ty::PrimitiveTy::Usize)
            .expect("usize type");
        let array_ty = append
            .intern(TyKind::Array {
                len: ArrayLenTy::ConstValue(2),
                elem: usize_ty,
            })
            .expect("array type");
        let pointer_ty = append
            .intern(TyKind::Pointer {
                is_readonly: true,
                elem: array_ty,
            })
            .expect("pointer type");
        let slice_ty = append
            .intern(TyKind::Slice {
                is_readonly: true,
                elem: usize_ty,
            })
            .expect("slice type");

        assert_eq!(
            classify_implicit_address(
                store.get(pointer_ty).unwrap(),
                Some(store.get(array_ty).unwrap()),
                array_ty,
                store.get(array_ty).unwrap(),
                |left, right| left == right,
            ),
            Some(ImplicitAddressPlan::Direct {
                pointer_is_readonly: true,
            })
        );
        assert_eq!(
            classify_implicit_address(
                store.get(slice_ty).unwrap(),
                None,
                array_ty,
                store.get(array_ty).unwrap(),
                |left, right| left == right,
            ),
            Some(ImplicitAddressPlan::ArrayToSlice {
                pointer_is_readonly: true,
                slice_is_readonly: true,
            })
        );
    }

    fn module_id_for_test() -> nia_ids::ModuleId {
        nia_ids::ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID")
    }

    #[test]
    fn arity_checks_exact_and_variadic_requirements() {
        assert!(check_exact_arity(2, 2).is_accepted());
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
        assert!(check_arity(ArityRequirement::AtLeast(0), 0).is_accepted());
        assert!(!check_arity(ArityRequirement::Exact(0), 1).is_accepted());
    }

    #[test]
    fn value_map_and_unique_field_helpers_preserve_set_contracts() {
        let actual = ["a", "a", "c"];
        let required: FieldSetCheck<&str> = check_value_field_set(actual, ["a", "b"]);
        assert_eq!(required.duplicate_fields.len(), 1);
        assert_eq!(required.duplicate_fields[0].span, Span::default());
        assert_eq!(required.unknown_fields.len(), 1);
        assert_eq!(required.unknown_fields[0].name, "c");
        assert_eq!(required.missing_fields, vec!["b"]);
        assert!(!required.is_valid());

        let mut values = HashMap::new();
        values.insert("a", 1u8);
        values.insert("b", 2u8);
        let from_map = check_map_field_set(&values, ["a", "b"]);
        assert!(from_map.is_valid());

        let unique = check_unique_field_set([
            NamedField::new(Span::new(4, 5), "x"),
            NamedField::new(Span::new(8, 9), "x"),
            NamedField::new(Span::new(10, 11), "x"),
        ]);
        assert_eq!(
            unique,
            vec![
                NamedField::new(Span::new(8, 9), "x"),
                NamedField::new(Span::new(10, 11), "x"),
            ]
        );
    }
}
