// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure usefulness and exhaustiveness analysis for typed patterns.
//!
//! Frontends keep ownership of type resolution and diagnostics. This crate only
//! operates on canonical constructor identities, field types, and scalar bounds.

use std::fmt;

/// A pattern after names, aliases, field order, and constants have been resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern<C> {
    Wildcard,
    Constructor {
        id: C,
        fields: Vec<Self>,
    },
    ScalarRange {
        start: i128,
        end: i128,
    },
    /// A valid pattern whose matched values cannot be characterized statically.
    ///
    /// Opaque patterns are useful unless shadowed by a wildcard, but never make a
    /// switch exhaustive. This is the conservative rule needed for runtime
    /// equality expressions and other user-defined matching behavior.
    Opaque,
}

/// One constructor in a type's canonical constructor universe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constructor<T, C> {
    pub id: C,
    pub fields: Vec<T>,
}

/// The values which can inhabit one pattern column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Domain<T, C> {
    /// Every constructor is known. An empty universe represents an uninhabited type.
    Finite(Vec<Constructor<T, C>>),
    /// Known constructors plus values introduced outside the current compilation unit.
    Open(Vec<Constructor<T, C>>),
    /// A finite scalar domain. Pattern endpoints partition it without enumerating values.
    Scalar {
        min: i128,
        max: i128,
        /// Whether `[min, max]` is the type's entire scalar domain.
        complete: bool,
    },
    /// No sound finite constructor interpretation is available.
    Opaque,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisError {
    MatrixWidth { expected: usize, found: usize },
    QueryWidth { expected: usize, found: usize },
    UnknownConstructor,
    ConstructorArity { expected: usize, found: usize },
    ScalarPatternOutsideScalarDomain,
}

impl fmt::Display for AnalysisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MatrixWidth { expected, found } => {
                write!(
                    formatter,
                    "pattern matrix row has width {found}, expected {expected}"
                )
            }
            Self::QueryWidth { expected, found } => {
                write!(
                    formatter,
                    "pattern query has width {found}, expected {expected}"
                )
            }
            Self::UnknownConstructor => write!(formatter, "pattern uses an unknown constructor"),
            Self::ConstructorArity { expected, found } => write!(
                formatter,
                "pattern constructor has arity {found}, expected {expected}"
            ),
            Self::ScalarPatternOutsideScalarDomain => {
                write!(formatter, "scalar pattern is used for a non-scalar type")
            }
        }
    }
}

impl std::error::Error for AnalysisError {}

/// Returns a concrete sub-pattern of `query` not matched by any matrix row.
///
/// `domain` must return constructor fields in the same canonical order used by
/// lowering. That correspondence is the central soundness invariant: matrix
/// specialization and runtime destructuring must interpret every constructor
/// identically.
pub fn useful_witness<T, C, F>(
    matrix: &[Vec<Pattern<C>>],
    query: &[Pattern<C>],
    types: &[T],
    mut domain: F,
) -> Result<Option<Vec<Pattern<C>>>, AnalysisError>
where
    T: Clone,
    C: Clone + Eq,
    F: FnMut(&T) -> Domain<T, C>,
{
    validate_widths(matrix, query, types.len())?;
    useful_inner(matrix, query, types, &mut domain)
}

/// Returns one value not covered by `matrix`, or `None` when it is exhaustive.
pub fn missing_witness<T, C, F>(
    matrix: &[Vec<Pattern<C>>],
    ty: T,
    domain: F,
) -> Result<Option<Pattern<C>>, AnalysisError>
where
    T: Clone,
    C: Clone + Eq,
    F: FnMut(&T) -> Domain<T, C>,
{
    Ok(useful_witness(matrix, &[Pattern::Wildcard], &[ty], domain)?
        .and_then(|mut witness| witness.pop()))
}

fn validate_widths<C>(
    matrix: &[Vec<Pattern<C>>],
    query: &[Pattern<C>],
    width: usize,
) -> Result<(), AnalysisError> {
    if query.len() != width {
        return Err(AnalysisError::QueryWidth {
            expected: width,
            found: query.len(),
        });
    }
    if let Some(row) = matrix.iter().find(|row| row.len() != width) {
        return Err(AnalysisError::MatrixWidth {
            expected: width,
            found: row.len(),
        });
    }
    Ok(())
}

fn useful_inner<T, C, F>(
    matrix: &[Vec<Pattern<C>>],
    query: &[Pattern<C>],
    types: &[T],
    domain: &mut F,
) -> Result<Option<Vec<Pattern<C>>>, AnalysisError>
where
    T: Clone,
    C: Clone + Eq,
    F: FnMut(&T) -> Domain<T, C>,
{
    if matrix.is_empty() {
        return Ok(Some(query.to_vec()));
    }
    if query.is_empty() {
        return Ok(None);
    }
    // This shortcut is important for recursive types: a wildcard row covers the
    // remaining product directly, without recursively expanding its constructors.
    if matrix.iter().any(|row| {
        row.iter()
            .all(|pattern| matches!(pattern, Pattern::Wildcard))
    }) {
        return Ok(None);
    }

    let head_ty = &types[0];
    let tail_types = &types[1..];
    let head = &query[0];
    let tail = &query[1..];
    let head_domain = domain(head_ty);

    match head {
        Pattern::Constructor { id, fields } => {
            let constructor = find_constructor(&head_domain, id)?;
            check_arity(fields.len(), constructor.fields.len())?;
            let specialized = specialize_constructor(matrix, id, constructor.fields.len())?;
            let mut specialized_query = fields.clone();
            specialized_query.extend_from_slice(tail);
            let mut specialized_types = constructor.fields.clone();
            specialized_types.extend_from_slice(tail_types);
            let Some(mut witness) =
                useful_inner(&specialized, &specialized_query, &specialized_types, domain)?
            else {
                return Ok(None);
            };
            let remaining = witness.split_off(constructor.fields.len());
            Ok(Some(
                std::iter::once(Pattern::Constructor {
                    id: id.clone(),
                    fields: witness,
                })
                .chain(remaining)
                .collect(),
            ))
        }
        Pattern::ScalarRange { start, end } => {
            let Domain::Scalar { min, max, .. } = head_domain else {
                return Err(AnalysisError::ScalarPatternOutsideScalarDomain);
            };
            let clipped = clip_range(*start, *end, min, max);
            let Some((start, end)) = clipped else {
                return Ok(None);
            };
            for (part_start, part_end) in scalar_partitions(matrix, Some((start, end)), min, max) {
                if part_start < start || part_end > end {
                    continue;
                }
                let specialized = specialize_scalar(matrix, part_start, part_end);
                if let Some(witness) = useful_inner(&specialized, tail, tail_types, domain)? {
                    return Ok(Some(
                        std::iter::once(Pattern::ScalarRange {
                            start: part_start,
                            end: part_start,
                        })
                        .chain(witness)
                        .collect(),
                    ));
                }
            }
            Ok(None)
        }
        Pattern::Wildcard => useful_wildcard(matrix, tail, tail_types, head_domain, domain),
        Pattern::Opaque => {
            let default = default_matrix(matrix);
            Ok(useful_inner(&default, tail, tail_types, domain)?
                .map(|witness| std::iter::once(Pattern::Opaque).chain(witness).collect()))
        }
    }
}

fn useful_wildcard<T, C, F>(
    matrix: &[Vec<Pattern<C>>],
    tail: &[Pattern<C>],
    tail_types: &[T],
    head_domain: Domain<T, C>,
    domain: &mut F,
) -> Result<Option<Vec<Pattern<C>>>, AnalysisError>
where
    T: Clone,
    C: Clone + Eq,
    F: FnMut(&T) -> Domain<T, C>,
{
    let is_open = matches!(&head_domain, Domain::Open(_));
    match head_domain {
        Domain::Finite(constructors) | Domain::Open(constructors) => {
            for constructor in constructors {
                let specialized =
                    specialize_constructor(matrix, &constructor.id, constructor.fields.len())?;
                let mut specialized_query = vec![Pattern::Wildcard; constructor.fields.len()];
                specialized_query.extend_from_slice(tail);
                let mut specialized_types = constructor.fields.clone();
                specialized_types.extend_from_slice(tail_types);
                if let Some(mut witness) =
                    useful_inner(&specialized, &specialized_query, &specialized_types, domain)?
                {
                    let remaining = witness.split_off(constructor.fields.len());
                    return Ok(Some(
                        std::iter::once(Pattern::Constructor {
                            id: constructor.id,
                            fields: witness,
                        })
                        .chain(remaining)
                        .collect(),
                    ));
                }
            }
            if is_open {
                let default = default_matrix(matrix);
                return Ok(useful_inner(&default, tail, tail_types, domain)?
                    .map(|witness| std::iter::once(Pattern::Wildcard).chain(witness).collect()));
            }
            Ok(None)
        }
        Domain::Scalar { min, max, complete } => {
            for (start, end) in scalar_partitions(matrix, None, min, max) {
                let specialized = specialize_scalar(matrix, start, end);
                if let Some(witness) = useful_inner(&specialized, tail, tail_types, domain)? {
                    return Ok(Some(
                        std::iter::once(Pattern::ScalarRange { start, end: start })
                            .chain(witness)
                            .collect(),
                    ));
                }
            }
            if !complete {
                let default = default_matrix(matrix);
                return Ok(useful_inner(&default, tail, tail_types, domain)?
                    .map(|witness| std::iter::once(Pattern::Wildcard).chain(witness).collect()));
            }
            Ok(None)
        }
        Domain::Opaque => {
            let default = default_matrix(matrix);
            Ok(useful_inner(&default, tail, tail_types, domain)?
                .map(|witness| std::iter::once(Pattern::Wildcard).chain(witness).collect()))
        }
    }
}

fn find_constructor<'a, T, C: Eq>(
    domain: &'a Domain<T, C>,
    id: &C,
) -> Result<&'a Constructor<T, C>, AnalysisError> {
    let constructors = match domain {
        Domain::Finite(constructors) | Domain::Open(constructors) => constructors,
        Domain::Scalar { .. } | Domain::Opaque => {
            return Err(AnalysisError::UnknownConstructor);
        }
    };
    constructors
        .iter()
        .find(|constructor| constructor.id == *id)
        .ok_or(AnalysisError::UnknownConstructor)
}

fn check_arity(found: usize, expected: usize) -> Result<(), AnalysisError> {
    if found == expected {
        Ok(())
    } else {
        Err(AnalysisError::ConstructorArity { expected, found })
    }
}

/// Specialization replaces the first constructor with its fields. Wildcards
/// expand to one wildcard per field; rows headed by another constructor vanish.
fn specialize_constructor<C: Clone + Eq>(
    matrix: &[Vec<Pattern<C>>],
    id: &C,
    arity: usize,
) -> Result<Vec<Vec<Pattern<C>>>, AnalysisError> {
    let mut result = Vec::new();
    for row in matrix {
        match &row[0] {
            Pattern::Wildcard => {
                let mut specialized = vec![Pattern::Wildcard; arity];
                specialized.extend_from_slice(&row[1..]);
                result.push(specialized);
            }
            Pattern::Constructor { id: row_id, fields } if row_id == id => {
                check_arity(fields.len(), arity)?;
                let mut specialized = fields.clone();
                specialized.extend_from_slice(&row[1..]);
                result.push(specialized);
            }
            Pattern::Constructor { .. } | Pattern::ScalarRange { .. } | Pattern::Opaque => {}
        }
    }
    Ok(result)
}

fn specialize_scalar<C: Clone>(
    matrix: &[Vec<Pattern<C>>],
    part_start: i128,
    part_end: i128,
) -> Vec<Vec<Pattern<C>>> {
    matrix
        .iter()
        .filter_map(|row| match &row[0] {
            Pattern::Wildcard => Some(row[1..].to_vec()),
            Pattern::ScalarRange { start, end } if *start <= part_start && part_end <= *end => {
                Some(row[1..].to_vec())
            }
            Pattern::Constructor { .. } | Pattern::ScalarRange { .. } | Pattern::Opaque => None,
        })
        .collect()
}

/// The default matrix represents values not selected by a known constructor.
/// Opaque patterns are deliberately excluded because their equality behavior
/// cannot prove that any part of the domain is covered.
fn default_matrix<C: Clone>(matrix: &[Vec<Pattern<C>>]) -> Vec<Vec<Pattern<C>>> {
    matrix
        .iter()
        .filter(|row| matches!(row[0], Pattern::Wildcard))
        .map(|row| row[1..].to_vec())
        .collect()
}

fn scalar_partitions<C>(
    matrix: &[Vec<Pattern<C>>],
    query: Option<(i128, i128)>,
    min: i128,
    max: i128,
) -> Vec<(i128, i128)> {
    if min > max {
        return Vec::new();
    }
    let mut boundaries = vec![min];
    for range in matrix
        .iter()
        .filter_map(|row| match row.first() {
            Some(Pattern::ScalarRange { start, end }) => Some((*start, *end)),
            _ => None,
        })
        .chain(query)
    {
        if let Some((start, end)) = clip_range(range.0, range.1, min, max) {
            boundaries.push(start);
            if end < max {
                boundaries.push(end + 1);
            }
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = boundaries
                .get(index + 1)
                .map_or(max, |next| next.saturating_sub(1));
            (*start, end)
        })
        .collect()
}

fn clip_range(start: i128, end: i128, min: i128, max: i128) -> Option<(i128, i128)> {
    let start = start.max(min);
    let end = end.min(max);
    (start <= end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Ty {
        Bool,
        Byte,
        Pair,
        OptionBool,
        Open,
        Opaque,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Ctor {
        Pair,
        None,
        Some,
        A,
    }

    fn domain(ty: &Ty) -> Domain<Ty, Ctor> {
        match ty {
            Ty::Bool => Domain::Scalar {
                min: 0,
                max: 1,
                complete: true,
            },
            Ty::Byte => Domain::Scalar {
                min: 0,
                max: 255,
                complete: true,
            },
            Ty::Pair => Domain::Finite(vec![Constructor {
                id: Ctor::Pair,
                fields: vec![Ty::Bool, Ty::Bool],
            }]),
            Ty::OptionBool => Domain::Finite(vec![
                Constructor {
                    id: Ctor::None,
                    fields: Vec::new(),
                },
                Constructor {
                    id: Ctor::Some,
                    fields: vec![Ty::Bool],
                },
            ]),
            Ty::Open => Domain::Open(vec![Constructor {
                id: Ctor::A,
                fields: Vec::new(),
            }]),
            Ty::Opaque => Domain::Opaque,
        }
    }

    fn scalar(value: i128) -> Pattern<Ctor> {
        Pattern::ScalarRange {
            start: value,
            end: value,
        }
    }

    #[test]
    fn detects_boolean_holes_and_complete_scalar_ranges() {
        let matrix = vec![vec![scalar(0)]];
        assert_eq!(
            missing_witness(&matrix, Ty::Bool, domain),
            Ok(Some(scalar(1)))
        );

        let matrix = vec![vec![Pattern::ScalarRange { start: 0, end: 255 }]];
        assert_eq!(missing_witness(&matrix, Ty::Byte, domain), Ok(None));
    }

    #[test]
    fn analyzes_cross_product_coverage_instead_of_fields_independently() {
        let pair = |left, right| Pattern::Constructor {
            id: Ctor::Pair,
            fields: vec![left, right],
        };
        let matrix = vec![
            vec![pair(scalar(0), Pattern::Wildcard)],
            vec![pair(Pattern::Wildcard, scalar(0))],
        ];
        assert_eq!(
            missing_witness(&matrix, Ty::Pair, domain),
            Ok(Some(pair(scalar(1), scalar(1))))
        );
    }

    #[test]
    fn nested_constructor_payloads_are_exhaustive() {
        let matrix = vec![
            vec![Pattern::Constructor {
                id: Ctor::None,
                fields: Vec::new(),
            }],
            vec![Pattern::Constructor {
                id: Ctor::Some,
                fields: vec![scalar(0)],
            }],
            vec![Pattern::Constructor {
                id: Ctor::Some,
                fields: vec![scalar(1)],
            }],
        ];
        assert_eq!(missing_witness(&matrix, Ty::OptionBool, domain), Ok(None));
    }

    #[test]
    fn reports_shadowed_structured_patterns_as_not_useful() {
        let matrix = vec![vec![Pattern::Constructor {
            id: Ctor::Some,
            fields: vec![Pattern::Wildcard],
        }]];
        let query = vec![Pattern::Constructor {
            id: Ctor::Some,
            fields: vec![scalar(1)],
        }];
        assert_eq!(
            useful_witness(&matrix, &query, &[Ty::OptionBool], domain),
            Ok(None)
        );
    }

    #[test]
    fn open_domains_and_opaque_patterns_require_a_wildcard() {
        let open = vec![vec![Pattern::Constructor {
            id: Ctor::A,
            fields: Vec::new(),
        }]];
        assert_eq!(
            missing_witness(&open, Ty::Open, domain),
            Ok(Some(Pattern::Wildcard))
        );

        let opaque = vec![vec![Pattern::Opaque]];
        assert_eq!(
            missing_witness(&opaque, Ty::Opaque, domain),
            Ok(Some(Pattern::Wildcard))
        );
    }

    #[test]
    fn clips_ranges_to_the_scalar_domain_and_handles_i128_max() {
        let matrix = vec![vec![Pattern::ScalarRange {
            start: i128::MIN,
            end: i128::MAX,
        }]];
        assert_eq!(missing_witness(&matrix, Ty::Byte, domain), Ok(None));

        let all_i128 = |_: &Ty| Domain::<Ty, Ctor>::Scalar {
            min: i128::MIN,
            max: i128::MAX,
            complete: true,
        };
        assert_eq!(missing_witness(&matrix, Ty::Byte, all_i128), Ok(None));
    }

    #[test]
    fn incompletely_represented_scalar_domain_requires_a_wildcard() {
        let matrix = vec![vec![Pattern::ScalarRange {
            start: 0,
            end: i128::MAX,
        }]];
        let unsigned_128 = |_: &Ty| Domain::<Ty, Ctor>::Scalar {
            min: 0,
            max: i128::MAX,
            complete: false,
        };
        assert_eq!(
            missing_witness(&matrix, Ty::Byte, unsigned_128),
            Ok(Some(Pattern::Wildcard))
        );
    }
}
