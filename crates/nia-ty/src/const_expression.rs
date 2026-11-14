// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed, substitution-preserving const computations. Execution belongs to the
//! const interpreter; this module defines identities and structural operations.
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_span::Span;
use nia_symbol::SymbolId;

use crate::ConstGenericArg;

/// A const computation with source provenance independent of semantic identity.
#[derive(Debug, Clone)]
pub struct ConstComputation {
    /// Module in which the operation was written.
    pub module_id: ModuleId,
    /// Exact operation span, used if evaluation fails after substitution.
    pub span: Span,
    /// Typed semantic operation.
    pub kind: ConstComputationKind,
}

impl PartialEq for ConstComputation {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}
impl Eq for ConstComputation {}
impl Hash for ConstComputation {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.kind.hash(state);
    }
}

/// An operation whose operands retain their own semantic types.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstComputationKind {
    /// Primitive unary operator.
    Unary {
        /// Operator.
        op: ConstUnaryOp,
        /// Typed operand.
        operand: ConstGenericArg,
    },
    /// Primitive binary operator, including lazy boolean operators.
    Binary {
        /// Operator.
        op: ConstBinaryOp,
        /// Left operand, evaluated first.
        lhs: ConstGenericArg,
        /// Right operand, evaluated when the operator requires it.
        rhs: ConstGenericArg,
    },
    /// An explicit conversion; its target is the containing argument's type.
    Cast(ConstGenericArg),
    /// Conditional selection; only the selected operand is executed.
    If {
        /// Boolean condition.
        condition: ConstGenericArg,
        /// Value when true.
        then_value: ConstGenericArg,
        /// Value when false.
        else_value: ConstGenericArg,
    },
    /// Array literal with explicit elements.
    ArrayList(Vec<ConstGenericArg>),
    /// Array literal that repeats one value a const number of times.
    ArrayRepeat {
        /// Repeated value.
        value: ConstGenericArg,
        /// Number of repetitions.
        count: ConstGenericArg,
    },
    /// Indexed array projection.
    Index {
        /// Array value.
        array: ConstGenericArg,
        /// Element index.
        index: ConstGenericArg,
    },
    /// A resolved const function. The interpreter owns its complete body.
    Call {
        /// Authoritative callee identity, independent of spelling or imports.
        function: GlobalDefId,
        /// Type substitutions sorted by the resolved parameter's symbol identity.
        type_args: Vec<(SymbolId, InternedTyId)>,
        /// Const substitutions sorted by the resolved parameter's symbol identity.
        const_args: Vec<(SymbolId, ConstGenericArg)>,
        /// Ordinary typed arguments, including a method receiver if present.
        args: Vec<ConstGenericArg>,
    },
    /// A `match` on a scalar: the first arm with a pattern that accepts the
    /// scrutinee is executed, and no other arm is.
    Match {
        /// Matched value.
        scrutinee: ConstGenericArg,
        /// Arms in source order.
        arms: Vec<ConstMatchArm>,
    },
    /// A closed operand whose evaluation fails, such as
    /// `std::builtin::error(message)` or `std::builtin::trap()`. The failure
    /// is its error message, and happens only if the operand is executed: an
    /// unselected branch never reports it.
    Fail(Arc<str>),
}

/// One arm of a scalar [`ConstComputationKind::Match`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConstMatchArm {
    /// Alternatives; the arm is selected when any accepts the scrutinee.
    pub patterns: Vec<ConstPattern>,
    /// The arm's value.
    pub value: ConstGenericArg,
}

/// A pattern of a scalar [`ConstComputationKind::Match`]. A binding pattern
/// has no form of its own: the arm's value refers to the scrutinee instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstPattern {
    /// Accepts any value.
    Wildcard,
    /// Accepts a value equal to this one.
    Value(ConstGenericArg),
    /// Accepts a value in this range.
    Range {
        /// Lower bound, included.
        start: ConstGenericArg,
        /// Upper bound.
        end: ConstGenericArg,
        /// Whether `end` is included.
        inclusive: bool,
    },
}

impl ConstPattern {
    fn operands(&self) -> Vec<&ConstGenericArg> {
        match self {
            Self::Wildcard => Vec::new(),
            Self::Value(value) => vec![value],
            Self::Range { start, end, .. } => vec![start, end],
        }
    }

    fn same_shape(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Wildcard, Self::Wildcard) | (Self::Value(_), Self::Value(_)) => true,
            (
                Self::Range {
                    inclusive: left, ..
                },
                Self::Range {
                    inclusive: right, ..
                },
            ) => left == right,
            _ => false,
        }
    }

    fn try_map<E>(
        &self,
        operand: &mut impl FnMut(&ConstGenericArg) -> Result<ConstGenericArg, E>,
    ) -> Result<Self, E> {
        Ok(match self {
            Self::Wildcard => Self::Wildcard,
            Self::Value(value) => Self::Value(operand(value)?),
            Self::Range {
                start,
                end,
                inclusive,
            } => Self::Range {
                start: operand(start)?,
                end: operand(end)?,
                inclusive: *inclusive,
            },
        })
    }
}

impl ConstComputation {
    /// Structural equality with contextual comparison of all type handles.
    pub fn equivalent(
        &self,
        other: &Self,
        same_type: &mut dyn FnMut(InternedTyId, InternedTyId) -> bool,
    ) -> bool {
        use ConstComputationKind as C;
        let same_operation = match (&self.kind, &other.kind) {
            (C::Unary { op: left, .. }, C::Unary { op: right, .. }) => left == right,
            (C::Binary { op: left, .. }, C::Binary { op: right, .. }) => left == right,
            (C::Cast(_), C::Cast(_)) | (C::If { .. }, C::If { .. }) => true,
            (C::ArrayList(left), C::ArrayList(right)) => left.len() == right.len(),
            (C::ArrayRepeat { .. }, C::ArrayRepeat { .. }) | (C::Index { .. }, C::Index { .. }) => {
                true
            }
            (
                C::Call {
                    function: left,
                    type_args: lt,
                    const_args: lc,
                    args: la,
                },
                C::Call {
                    function: right,
                    type_args: rt,
                    const_args: rc,
                    args: ra,
                },
            ) => {
                left == right
                    && lt.len() == rt.len()
                    && lc.len() == rc.len()
                    && la.len() == ra.len()
                    && lt
                        .iter()
                        .zip(rt)
                        .all(|((ln, l), (rn, r))| ln == rn && same_type(*l, *r))
                    && lc.iter().zip(rc).all(|((ln, _), (rn, _))| ln == rn)
            }
            (
                C::Match {
                    arms: left_arms, ..
                },
                C::Match {
                    arms: right_arms, ..
                },
            ) => {
                left_arms.len() == right_arms.len()
                    && left_arms.iter().zip(right_arms).all(|(left, right)| {
                        left.patterns.len() == right.patterns.len()
                            && left
                                .patterns
                                .iter()
                                .zip(&right.patterns)
                                .all(|(left, right)| left.same_shape(right))
                    })
            }
            (C::Fail(left), C::Fail(right)) => left == right,
            _ => false,
        };
        same_operation
            && self
                .operands()
                .iter()
                .zip(other.operands())
                .all(|(left, right)| left.equivalent(right, same_type))
    }

    /// Visits every embedded operand and callee type argument.
    pub fn visit_referenced_types(&self, visit: &mut dyn FnMut(InternedTyId)) {
        if let ConstComputationKind::Call { type_args, .. } = &self.kind {
            for (_, ty) in type_args {
                visit(*ty);
            }
        }
        for arg in self.operands() {
            arg.visit_types(visit);
        }
    }

    /// Rebuilds every nested type handle in this computation.
    pub fn try_map_types<E>(
        &self,
        map: &mut dyn FnMut(InternedTyId) -> Result<InternedTyId, E>,
    ) -> Result<Arc<Self>, E> {
        let map = std::cell::RefCell::new(map);
        self.try_map(
            |arg| {
                let mut arg = arg.clone();
                arg.try_map_types_with(&mut **map.borrow_mut())?;
                Ok(arg)
            },
            |ty| map.borrow_mut()(ty),
        )
    }

    /// Whether any operand or embedded type still depends on a parameter.
    pub fn contains_generic(&self, type_contains: &mut dyn FnMut(InternedTyId) -> bool) -> bool {
        self.operands()
            .iter()
            .any(|arg| arg.contains_generic(&mut *type_contains))
            || matches!(&self.kind, ConstComputationKind::Call { type_args, .. } if type_args.iter().any(|(_, ty)| type_contains(*ty)))
    }

    /// Function identities needed to execute this term, including nested calls.
    pub fn referenced_functions(&self) -> Vec<GlobalDefId> {
        let mut result = Vec::new();
        if let ConstComputationKind::Call { function, .. } = self.kind {
            result.push(function);
        }
        for arg in self.operands() {
            if let crate::ConstGenericValue::Computation(expr) = &arg.value {
                result.extend(expr.referenced_functions());
            }
        }
        result
    }

    /// Visits substitutions in canonical symbol order, then ordinary operands
    /// in evaluation order. Conditional operands are visited structurally.
    pub fn operands(&self) -> Vec<&ConstGenericArg> {
        match &self.kind {
            ConstComputationKind::Unary { operand, .. } | ConstComputationKind::Cast(operand) => {
                vec![operand]
            }
            ConstComputationKind::Binary { lhs, rhs, .. } => vec![lhs, rhs],
            ConstComputationKind::If {
                condition,
                then_value,
                else_value,
            } => {
                vec![condition, then_value, else_value]
            }
            ConstComputationKind::ArrayList(values) => values.iter().collect(),
            ConstComputationKind::ArrayRepeat { value, count } => vec![value, count],
            ConstComputationKind::Index { array, index } => vec![array, index],
            ConstComputationKind::Call {
                const_args, args, ..
            } => const_args.iter().map(|(_, arg)| arg).chain(args).collect(),
            ConstComputationKind::Match { scrutinee, arms } => std::iter::once(scrutinee)
                .chain(arms.iter().flat_map(|arm| {
                    arm.patterns
                        .iter()
                        .flat_map(ConstPattern::operands)
                        .chain(std::iter::once(&arm.value))
                }))
                .collect(),
            ConstComputationKind::Fail(_) => Vec::new(),
        }
    }

    /// Rebuilds operands and embedded type arguments without changing provenance.
    pub fn try_map<E>(
        &self,
        mut operand: impl FnMut(&ConstGenericArg) -> Result<ConstGenericArg, E>,
        mut ty: impl FnMut(InternedTyId) -> Result<InternedTyId, E>,
    ) -> Result<Arc<Self>, E> {
        let kind = match &self.kind {
            ConstComputationKind::Unary { op, operand: arg } => ConstComputationKind::Unary {
                op: *op,
                operand: operand(arg)?,
            },
            ConstComputationKind::Binary { op, lhs, rhs } => ConstComputationKind::Binary {
                op: *op,
                lhs: operand(lhs)?,
                rhs: operand(rhs)?,
            },
            ConstComputationKind::Cast(arg) => ConstComputationKind::Cast(operand(arg)?),
            ConstComputationKind::If {
                condition,
                then_value,
                else_value,
            } => ConstComputationKind::If {
                condition: operand(condition)?,
                then_value: operand(then_value)?,
                else_value: operand(else_value)?,
            },
            ConstComputationKind::ArrayList(values) => ConstComputationKind::ArrayList(
                values.iter().map(&mut operand).collect::<Result<_, _>>()?,
            ),
            ConstComputationKind::ArrayRepeat { value, count } => {
                ConstComputationKind::ArrayRepeat {
                    value: operand(value)?,
                    count: operand(count)?,
                }
            }
            ConstComputationKind::Index { array, index } => ConstComputationKind::Index {
                array: operand(array)?,
                index: operand(index)?,
            },
            ConstComputationKind::Call {
                function,
                type_args,
                const_args,
                args,
            } => ConstComputationKind::Call {
                function: *function,
                type_args: type_args
                    .iter()
                    .map(|(name, arg)| Ok((*name, ty(*arg)?)))
                    .collect::<Result<_, E>>()?,
                const_args: const_args
                    .iter()
                    .map(|(name, arg)| Ok((*name, operand(arg)?)))
                    .collect::<Result<_, E>>()?,
                args: args.iter().map(&mut operand).collect::<Result<_, E>>()?,
            },
            ConstComputationKind::Match { scrutinee, arms } => ConstComputationKind::Match {
                scrutinee: operand(scrutinee)?,
                arms: arms
                    .iter()
                    .map(|arm| {
                        Ok(ConstMatchArm {
                            patterns: arm
                                .patterns
                                .iter()
                                .map(|pattern| pattern.try_map(&mut operand))
                                .collect::<Result<_, E>>()?,
                            value: operand(&arm.value)?,
                        })
                    })
                    .collect::<Result<_, E>>()?,
            },
            ConstComputationKind::Fail(message) => ConstComputationKind::Fail(message.clone()),
        };
        Ok(Arc::new(Self {
            module_id: self.module_id,
            span: self.span,
            kind,
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Unary operations available in const expressions.
pub enum ConstUnaryOp {
    /// Arithmetic negation.
    Neg,
    /// Boolean/logical negation.
    Not,
    /// Integer bitwise complement.
    BitNot,
    /// Read-only reference creation.
    RefReadOnly,
    /// Mutable reference creation.
    Ref,
    /// Pointer dereference.
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Binary operations available in const expressions.
pub enum ConstBinaryOp {
    /// Multiplication.
    Mul,
    /// Division.
    Div,
    /// Remainder.
    Rem,
    /// Addition.
    Add,
    /// Subtraction.
    Sub,
    /// Left shift.
    Shl,
    /// Right shift.
    Shr,
    /// Less-than comparison.
    Lt,
    /// Less-than-or-equal comparison.
    Le,
    /// Greater-than comparison.
    Gt,
    /// Greater-than-or-equal comparison.
    Ge,
    /// Equality comparison.
    Eq,
    /// Inequality comparison.
    Ne,
    /// Bitwise and.
    BitAnd,
    /// Bitwise xor.
    BitXor,
    /// Bitwise or.
    BitOr,
    /// Logical and.
    And,
    /// Logical or.
    Or,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConstGenericValue, IntConst, LayoutBuiltin, PrimitiveTy, TypeStore};
    use nia_ids::ModuleIdAllocator;
    use nia_symbol::stable_hash;
    use std::collections::hash_map::DefaultHasher;

    fn hash(value: &impl Hash) -> u64 {
        let mut state = DefaultHasher::new();
        value.hash(&mut state);
        state.finish()
    }

    #[test]
    fn identity_excludes_locations_and_preserves_operand_order_and_types() {
        let ids = ModuleIdAllocator::new().expect("module ids");
        let module_id = ids.allocate().expect("module");
        let store = TypeStore::new().expect("type store");
        let append = store.append_for_module(module_id);
        let ty = append.primitive(PrimitiveTy::Usize).expect("usize");
        let left = ConstGenericArg {
            ty,
            value: ConstGenericValue::GenericParam(SymbolId::from_stable_hash(stable_hash("N"))),
        };
        let right = ConstGenericArg {
            ty,
            value: ConstGenericValue::Int(IntConst::unsigned(1)),
        };
        let original = ConstComputation {
            module_id,
            span: Span::new(10, 15),
            kind: ConstComputationKind::Binary {
                op: ConstBinaryOp::Add,
                lhs: left.clone(),
                rhs: right.clone(),
            },
        };
        let relocated = ConstComputation {
            module_id: ids.allocate().expect("other module"),
            span: Span::new(20, 25),
            kind: original.kind.clone(),
        };
        assert_eq!(original, relocated);
        assert_eq!(hash(&original), hash(&relocated));
        let reversed = ConstComputation {
            kind: ConstComputationKind::Binary {
                op: ConstBinaryOp::Add,
                lhs: right,
                rhs: left,
            },
            ..original.clone()
        };
        assert_ne!(original, reversed);
        let mut retyped = original.clone();
        if let ConstComputationKind::Binary { rhs, .. } = &mut retyped.kind {
            rhs.ty = append.primitive(PrimitiveTy::U64).expect("u64");
        }
        assert_ne!(original, retyped);
    }

    #[test]
    fn substitution_rebuilds_nested_call_arguments_and_all_type_handles() {
        let module_id = ModuleIdAllocator::new()
            .expect("module ids")
            .allocate()
            .expect("module");
        let source = TypeStore::new().expect("source store");
        let destination = TypeStore::new().expect("destination store");
        let source_ty = source
            .append_for_module(module_id)
            .primitive(PrimitiveTy::Usize)
            .expect("source usize");
        let destination_ty = destination
            .append_for_module(module_id)
            .primitive(PrimitiveTy::Usize)
            .expect("destination usize");
        let name = SymbolId::from_stable_hash(stable_hash("N"));
        let parameter = ConstGenericArg {
            ty: source_ty,
            value: ConstGenericValue::GenericParam(name),
        };
        let value = ConstGenericArg {
            ty: source_ty,
            value: ConstGenericValue::Computation(Arc::new(ConstComputation {
                module_id,
                span: Span::new(1, 4),
                kind: ConstComputationKind::Call {
                    function: GlobalDefId {
                        module_id,
                        def_id: nia_ids::DefId(1),
                    },
                    type_args: vec![(SymbolId::from_stable_hash(stable_hash("T")), source_ty)],
                    const_args: vec![(name, parameter.clone())],
                    args: vec![parameter],
                },
            })),
        };
        let result: Result<_, std::convert::Infallible> = crate::substitute_const_generic_arg(
            &value,
            &|parameter| {
                (*parameter == name).then_some(ConstGenericArg {
                    ty: source_ty,
                    value: ConstGenericValue::Int(IntConst::unsigned(7)),
                })
            },
            &|ty| {
                assert_eq!(ty, source_ty);
                Ok(destination_ty)
            },
        );
        let result = result.expect("substitute");
        assert!(!result.contains_generic(|_| false));
        let mut referenced = Vec::new();
        result.visit_referenced_types(|ty| referenced.push(ty));
        assert_eq!(
            referenced,
            vec![destination_ty, destination_ty, source_ty, source_ty]
        );
        assert!(
            !value.equivalent(&result, &mut |left, right| source.get(left)
                == destination.get(right))
        );
        let mapped = match value.value {
            ConstGenericValue::Computation(term) => term
                .try_map_types::<std::convert::Infallible>(&mut |_| Ok(destination_ty))
                .expect("map types"),
            _ => unreachable!(),
        };
        let ConstGenericValue::Computation(original) = &result.value else {
            unreachable!()
        };
        assert!(!mapped.equivalent(original, &mut |left, right| left == right));
        let original = match &mapped.kind {
            ConstComputationKind::Call {
                const_args, args, ..
            } => (&const_args[0].1, &args[0]),
            _ => unreachable!(),
        };
        assert_eq!(original.0.value, ConstGenericValue::GenericParam(name));
        assert_eq!(original.1.value, ConstGenericValue::GenericParam(name));
    }

    #[test]
    fn const_parameter_replacement_is_simultaneous_with_type_substitution() {
        let ids = ModuleIdAllocator::new()
            .expect("module ids")
            .allocate()
            .expect("module");
        let store = TypeStore::new().expect("type store");
        let append = store.append_for_module(ids);
        let source_ty = append.primitive(PrimitiveTy::Usize).expect("source type");
        let replacement = ConstGenericArg {
            ty: source_ty,
            value: ConstGenericValue::Layout {
                builtin: LayoutBuiltin::Size,
                ty: source_ty,
            },
        };
        let parameter = SymbolId::from_stable_hash(stable_hash("N"));
        let arg = ConstGenericArg {
            ty: source_ty,
            value: ConstGenericValue::GenericParam(parameter),
        };
        let result = crate::substitute_const_generic_arg(
            &arg,
            &|name| (*name == parameter).then_some(replacement.clone()),
            &|_| -> Result<_, std::convert::Infallible> {
                panic!("caller replacement must not be remapped")
            },
        )
        .expect("simultaneous substitution");
        assert_eq!(result, replacement);
    }
}
