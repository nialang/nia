// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure usefulness and exhaustiveness analysis for typed patterns.
//!
//! Frontends keep ownership of type resolution and diagnostics. This crate only
//! operates on canonical constructor identities, field types, and scalar bounds.
//!
//! # Design References
//!
//! The algorithm follows Luc Maranget, "Warnings for pattern matching",
//! *Journal of Functional Programming* 17(3), 2007, pp. 387-421,
//! <https://doi.org/10.1017/S0956796806006223>. In particular, the
//! usefulness relation over constructor matrices is used for both unreachable
//! arm detection and exhaustiveness witnesses. The constructor matrix is also
//! compatible with the representation described in Luc Maranget, "Compiling
//! Pattern Matching to Good Decision Trees", ML 2008,
//! <https://doi.org/10.1145/1411304.1411314>, although this crate does not
//! compile decision trees: runtime lowering owns tag tests and projections.
//!
//! Rust's pattern reference (<https://doc.rust-lang.org/reference/patterns.html>)
//! and OCaml's pattern manual (<https://ocaml.org/manual/patterns.html>) are
//! useful language-design comparisons, but are not normative for Nia.
//!
//! # Matrix Contract
//!
//! A matrix row contains one pattern per matched value column. Constructor
//! specialization replaces a constructor with its fields; wildcard rows are
//! expanded to one wildcard per field. The default matrix retains rows that
//! may match values outside the currently known constructor set. A wildcard
//! query is useful when one of those specialized/default queries is useful.
//!
//! The `domain` callback must return constructor fields in exactly the canonical
//! declaration order consumed by runtime and const lowering. This is the main
//! soundness invariant:
//!
//! ```text
//! analysis constructor fields[i] == lowered constructor field_defs[i]
//! ```
//!
//! Constructor ids and arities are validated rather than treated as display
//! names. An adapter failure must become a diagnostic; it must never silently
//! turn an unknown pattern into coverage. The early all-wildcard-row shortcut
//! is intentional: it gives recursive types a finite stopping point while
//! preserving the matrix semantics.
//!
//! # Domain Semantics
//!
//! `Finite` means every constructor is known. `Open` means the listed
//! constructors are known but unnamed values may exist, so an open enum still
//! requires a wildcard. `Scalar` partitions an integer or boolean interval at
//! pattern endpoints without enumerating all values. `complete = false` means
//! the adapter cannot represent the complete backing domain, so the analysis
//! must not prove exhaustiveness from intervals alone. `Opaque` patterns remain
//! useful unless shadowed, but never prove exhaustiveness.
//!
//! Scalar endpoints are validated against the target domain before they reach
//! the matrix. The current deliberate conservative boundary is `u128` and
//! target-dependent wide `usize` representations that cannot be losslessly
//! represented by the analysis integer; those switches require `_`.
//!
//! # Frontend Boundary
//!
//! Runtime and const adapters resolve names, types, fields, constants, and the
//! terminal nominal `..` marker before calling this crate. Omitted nominal
//! fields become wildcard children in declaration order; no synthetic field is
//! introduced. Thus `Point { .. }` is irrefutable for `Point`, while
//! `Event::Resize { .. }` covers only that variant's payload.
//!
//! For each arm, frontends query `useful_witness` against the matrix of prior
//! useful arms. A `None` result is an unreachable-pattern diagnostic; a witness
//! is appended to the matrix. After all arms, `missing_witness` is queried with
//! a wildcard. A returned pattern is a concrete missing-case explanation, not
//! a new syntax accepted by the language. Const evaluation remains path-driven;
//! whole-function control-flow soundness remains owned by `nia-body-check`.
//!
//! # Algorithm In The Paper
//!
//! Maranget's central question is `useful(P, q)`: does the row `q` contain a
//! value not already covered by matrix `P`? Nia's `useful_inner` is the direct
//! recursive implementation of that relation. Its two matrix operations are:
//!
//! ```text
//! specialize(P, C) = rows whose head is `_` or C, with C's fields exposed
//! default(P)       = rows whose head is `_`, with the first column removed
//!
//! useful([], q)                 = q
//! useful(P, [])                 = no witness
//! useful(P, C(args) :: q)       =
//!     rebuild C around useful(specialize(P, C), args :: q)
//! useful(P, _ :: q)             =
//!     first useful constructor specialization,
//!     then default(P) when the constructor universe is incomplete
//! ```
//!
//! A constructor query first specializes on that constructor and recursively
//! checks its payload fields. A wildcard query tries every known constructor;
//! for an open or incomplete domain it also checks `default(P)`, representing
//! values which the adapter cannot enumerate. This is why coverage is checked
//! over the product of fields rather than independently per field.
//!
//! | Paper notation | This crate | Purpose |
//! | --- | --- | --- |
//! | pattern vector `q` | `query` | candidate row being tested |
//! | pattern matrix `P` | `matrix` | earlier useful rows |
//! | constructor signature | `domain(type)` | constructors and field types |
//! | specialization `S(c, P)` | `specialize_constructor` | expose payload columns |
//! | default `D(P)` | `default_matrix` | remove the wildcard head column |
//! | usefulness `U(P, q)` | `useful_inner` | return an uncovered witness |
//!
//! For example, for `Option<Bool>` (booleans are scalar `0..=1` inside the
//! analysis representation):
//!
//! ```text
//! Nia source rows   matrix                 query
//! Some(false)       [ Some(0) ]            [_]
//! Some(_)           [ Some(_) ]
//! None              [ None ]
//!
//! specialize(Some): [ 0 ] [ _ ]
//! default:          [ None ]
//! ```
//!
//! The `Some` branch is exhaustive because `0` and `_` cover the payload; the
//! default branch is then checked and finds `None`. Once that row is present,
//! `missing_witness` returns `None`. With only `Some(_)`, the same recursion
//! returns the constructor witness `None`, making the diagnostic concrete
//! rather than merely saying “some constructor is missing”.
//!
//! Scalar patterns use the same recursion without enumerating every integer.
//! Endpoints partition `[min, max]` into disjoint intervals, and each interval
//! is specialized as a row. A witness is represented by one point from an
//! uncovered interval. The implementation deliberately emits a singleton
//! endpoint (`start == end`) so diagnostics remain stable and easy to read.
//!
//! | Domain | Known values | Exhaustiveness rule |
//! | --- | --- | --- |
//! | `Finite` | all constructors | all constructors and payloads covered |
//! | `Open` | listed constructors only | known constructors plus `_` |
//! | `Scalar` (`complete`) | bounded interval | every partition covered |
//! | `Scalar` (incomplete) | conservative interval | interval coverage plus `_` |
//! | `Opaque` | no static partition | `_` is required |
//!
//! This crate stops at usefulness and witnesses. It intentionally does not
//! build Maranget decision trees: runtime lowering and const execution have
//! different ownership and evaluation constraints, so sharing a coverage
//! proof is safer than sharing generated control flow.
//!
//! When changing pattern syntax, type lowering, or match lowering, audit the
//! parser representation, both adapters, constructor identity and field order,
//! finite/open/opaque classification, scalar clipping, witness formatting,
//! irrefutable `let`/`for` checks, runtime lowering, const evaluation, and the
//! parser/body-check/const-check/lowering/executable test layers together.

use std::fmt;

/// A pattern after names, aliases, field order, and constants have been resolved.
///
/// The representation is intentionally smaller than the AST. At this point a
/// bind and a wildcard are both `Wildcard`, nominal `..` has already become
/// wildcard children, and a scalar expression is either a checked interval or
/// `Opaque`. Keeping unresolved syntax out of the matrix prevents the coverage
/// algorithm from accidentally making a semantic decision about name lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern<C> {
    /// Pattern matching any value in the column.
    Wildcard,
    /// Constructor pattern with recursively matched fields.
    Constructor {
        /// Constructor identity.
        id: C,
        /// Field patterns in canonical declaration order.
        fields: Vec<Self>,
    },
    /// Scalar interval pattern.
    ScalarRange {
        /// Inclusive lower endpoint.
        start: i128,
        /// Inclusive upper endpoint.
        end: i128,
    },
    /// A valid pattern whose matched values cannot be characterized statically.
    ///
    /// Opaque patterns are useful unless shadowed by a wildcard, but never make a
    /// match exhaustive. This is the conservative rule needed for runtime
    /// equality expressions and other user-defined matching behavior.
    Opaque,
}

/// One constructor in a type's canonical constructor universe.
///
/// `fields` is ordered data, not a set: the index is the ABI/lowering
/// projection index used by specialization. Adapters must therefore source it
/// from the same canonical declaration metadata as destructuring code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constructor<T, C> {
    /// Canonical constructor identity.
    pub id: C,
    /// Constructor fields in declaration/lowering order.
    pub fields: Vec<T>,
}

/// The values which can inhabit one pattern column.
///
/// The distinction between `Finite`, `Open`, and incomplete `Scalar` is
/// semantic, not an optimization hint. It determines whether the fallback
/// (`default`) matrix is explored and therefore whether `_` is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Domain<T, C> {
    /// Every constructor is known. An empty universe represents an uninhabited type.
    Finite(Vec<Constructor<T, C>>),
    /// Known constructors plus values introduced outside the current compilation unit.
    Open(Vec<Constructor<T, C>>),
    /// A finite scalar domain. Pattern endpoints partition it without enumerating values.
    Scalar {
        /// Minimum representable scalar value.
        min: i128,
        /// Maximum representable scalar value.
        max: i128,
        /// Whether `[min, max]` is the type's entire scalar domain.
        complete: bool,
    },
    /// No sound finite constructor interpretation is available.
    Opaque,
}

/// Validation failures from matrix, query, or domain normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisError {
    /// A matrix row has a different number of columns than the domain.
    MatrixWidth {
        /// Expected column count.
        expected: usize,
        /// Actual row column count.
        found: usize,
    },
    /// A query has a different number of columns than the domain.
    QueryWidth {
        /// Expected column count.
        expected: usize,
        /// Actual query column count.
        found: usize,
    },
    /// A pattern refers to a constructor absent from its domain.
    UnknownConstructor,
    /// A constructor pattern has the wrong number of fields.
    ConstructorArity {
        /// Expected field count.
        expected: usize,
        /// Actual pattern field count.
        found: usize,
    },
    /// A scalar range was supplied for a non-scalar domain.
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
/// This is Maranget's `useful(P, q)` relation with a witness attached. `Some`
/// means the query is useful and contains the returned counterexample;
/// `None` means every value described by the query is already covered. The
/// matrix is assumed to contain only earlier useful rows, which is why callers
/// append a row only after receiving `Some`.
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
    T: Clone + Eq,
    C: Clone + Eq,
    F: FnMut(&T) -> Domain<T, C>,
{
    validate_widths(matrix, query, types.len())?;
    useful_inner(matrix, query, types, &mut domain)
}

/// Returns one value not covered by `matrix`, or `None` when it is exhaustive.
///
/// This is the one-column convenience form used after all match arms have
/// been normalized. It asks `useful_witness` about `_` and unwraps the only
/// column, preserving the same validation and conservative domain rules.
pub fn missing_witness<T, C, F>(
    matrix: &[Vec<Pattern<C>>],
    ty: T,
    domain: F,
) -> Result<Option<Pattern<C>>, AnalysisError>
where
    T: Clone + Eq,
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
    T: Clone + Eq,
    C: Clone + Eq,
    F: FnMut(&T) -> Domain<T, C>,
{
    if matrix.is_empty() {
        return Ok(validate_empty_query(query, types, domain)?.then(|| query.to_vec()));
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

/// Validate an otherwise-uncovered query without specializing it into a more
/// concrete witness. The old empty-matrix shortcut returned the query verbatim,
/// which is useful for stable diagnostics, but it also skipped constructor and
/// scalar-domain validation. This pass preserves the witness shape while
/// checking that the queried product has at least one inhabitant.
//
// The recursive helper guards wildcard expansion, where the analysis is free
// to choose any constructor for a type. Explicit constructor queries are not
// added to that guard: a recursive field may revisit the same type and choose a
// finite base constructor even when the outer query selected a recursive one.
// This still rejects cycle-only domains such as `Loop(Loop)`, because every
// finite query eventually reaches a wildcard whose expansion encounters its
// own active type without finding a base constructor.
fn validate_empty_query<T, C, F>(
    query: &[Pattern<C>],
    types: &[T],
    domain: &mut F,
) -> Result<bool, AnalysisError>
where
    T: Clone + Eq,
    C: Clone + Eq,
    F: FnMut(&T) -> Domain<T, C>,
{
    validate_empty_query_inner(query, types, domain, &mut Vec::new())
}

fn validate_empty_query_inner<T, C, F>(
    query: &[Pattern<C>],
    types: &[T],
    domain: &mut F,
    active_types: &mut Vec<T>,
) -> Result<bool, AnalysisError>
where
    T: Clone + Eq,
    C: Clone + Eq,
    F: FnMut(&T) -> Domain<T, C>,
{
    if query.is_empty() {
        return Ok(true);
    }
    let current_ty = &types[0];
    let head_result = match &query[0] {
        Pattern::Wildcard | Pattern::Opaque => {
            if active_types.iter().any(|ty| ty == current_ty) {
                false
            } else {
                active_types.push(current_ty.clone());
                let head_domain = domain(current_ty);
                let result = validate_wildcard_head(&head_domain, domain, active_types)?;
                active_types.pop();
                result
            }
        }
        Pattern::Constructor { id, fields } => {
            let head_domain = domain(current_ty);
            let constructor = find_constructor(&head_domain, id)?;
            check_arity(fields.len(), constructor.fields.len())?;
            validate_empty_query_inner(fields, &constructor.fields, domain, active_types)?
        }
        Pattern::ScalarRange { start, end } => {
            let head_domain = domain(current_ty);
            let Domain::Scalar { min, max, .. } = head_domain else {
                return Err(AnalysisError::ScalarPatternOutsideScalarDomain);
            };
            clip_range(*start, *end, min, max).is_some()
        }
    };
    if !head_result {
        return Ok(false);
    }
    validate_empty_query_inner(&query[1..], &types[1..], domain, active_types)
}

fn validate_wildcard_head<T, C, F>(
    head_domain: &Domain<T, C>,
    domain: &mut F,
    active_types: &mut Vec<T>,
) -> Result<bool, AnalysisError>
where
    T: Clone + Eq,
    C: Clone + Eq,
    F: FnMut(&T) -> Domain<T, C>,
{
    match head_domain {
        Domain::Finite(constructors) => {
            for constructor in constructors {
                let wildcard_fields = vec![Pattern::Wildcard; constructor.fields.len()];
                if validate_empty_query_inner(
                    &wildcard_fields,
                    &constructor.fields,
                    domain,
                    active_types,
                )? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Domain::Open(_) | Domain::Opaque => Ok(true),
        Domain::Scalar { min, max, .. } => Ok(min <= max),
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
    T: Clone + Eq,
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
        RecursiveLoop,
        RecursiveList,
        RecursiveOptional,
        RecursiveNode,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Ctor {
        Pair,
        None,
        Some,
        A,
        Loop,
        Nil,
        Cons,
        Node,
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
            Ty::RecursiveLoop => Domain::Finite(vec![Constructor {
                id: Ctor::Loop,
                fields: vec![Ty::RecursiveLoop],
            }]),
            Ty::RecursiveList => Domain::Finite(vec![
                Constructor {
                    id: Ctor::Nil,
                    fields: Vec::new(),
                },
                Constructor {
                    id: Ctor::Cons,
                    fields: vec![Ty::RecursiveList],
                },
            ]),
            Ty::RecursiveOptional => Domain::Finite(vec![
                Constructor {
                    id: Ctor::None,
                    fields: Vec::new(),
                },
                Constructor {
                    id: Ctor::Some,
                    fields: vec![Ty::RecursiveNode],
                },
            ]),
            Ty::RecursiveNode => Domain::Finite(vec![Constructor {
                id: Ctor::Node,
                fields: vec![Ty::RecursiveOptional],
            }]),
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

    #[test]
    fn empty_finite_domains_are_exhaustive_without_a_pattern() {
        let uninhabited = |_: &Ty| Domain::<Ty, Ctor>::Finite(Vec::new());
        assert_eq!(missing_witness(&[], Ty::Opaque, uninhabited), Ok(None));
    }

    #[test]
    fn rejects_matrix_and_query_width_mismatches() {
        let matrix = vec![vec![Pattern::Wildcard, Pattern::Wildcard]];
        assert_eq!(
            useful_witness(&matrix, &[Pattern::Wildcard], &[Ty::Bool], domain),
            Err(AnalysisError::MatrixWidth {
                expected: 1,
                found: 2,
            })
        );
        assert_eq!(
            useful_witness(
                &[],
                &[Pattern::Wildcard, Pattern::Wildcard],
                &[Ty::Bool],
                domain
            ),
            Err(AnalysisError::QueryWidth {
                expected: 1,
                found: 2,
            })
        );
    }

    #[test]
    fn rejects_scalar_patterns_for_constructor_domains() {
        assert_eq!(
            useful_witness(
                &[],
                &[Pattern::ScalarRange { start: 0, end: 0 }],
                &[Ty::OptionBool],
                domain,
            ),
            Err(AnalysisError::ScalarPatternOutsideScalarDomain)
        );
    }

    #[test]
    fn validates_recursive_domains_with_a_finite_type_path() {
        assert_eq!(
            missing_witness(&[], Ty::RecursiveLoop, domain),
            Ok(None),
            "a recursive constructor without a base case is uninhabited"
        );
        assert_eq!(
            missing_witness(&[], Ty::RecursiveList, domain),
            Ok(Some(Pattern::Wildcard))
        );
    }

    #[test]
    fn validation_does_not_confuse_independent_columns_with_recursion() {
        assert_eq!(
            useful_witness(
                &[],
                &[Pattern::Wildcard, Pattern::Wildcard],
                &[Ty::Bool, Ty::Bool],
                domain,
            ),
            Ok(Some(vec![Pattern::Wildcard, Pattern::Wildcard]))
        );
    }

    #[test]
    fn constrained_recursive_query_can_reach_a_nested_base_constructor() {
        let some = Pattern::Constructor {
            id: Ctor::Some,
            fields: vec![Pattern::Wildcard],
        };
        assert_eq!(
            useful_witness(
                &[],
                std::slice::from_ref(&some),
                &[Ty::RecursiveOptional],
                domain,
            ),
            Ok(Some(vec![some.clone()]))
        );
        let matrix = vec![
            vec![some],
            vec![Pattern::Constructor {
                id: Ctor::None,
                fields: Vec::new(),
            }],
        ];
        assert_eq!(
            missing_witness(&matrix, Ty::RecursiveOptional, domain),
            Ok(None)
        );
    }
}
