use crate::{
    ConstAbiField, ConstAbiType, ConstCommonEnv, ConstEndianness, ConstError, ConstEvalBudget,
    ConstScalarType, ConstUnionValue, ConstValue, EarlyConstEnv, EmptyEnv, ResolvedConstEnv,
    eval_early_const_bool_expr, eval_early_const_expr, eval_early_const_int_expr,
    eval_float_literal, eval_int_literal, eval_resolved_const_int_expr,
};
use nia_const_ir::{
    ConstAssignOp, ConstNameResolution, EarlyConstAssign, EarlyConstAssignTarget, EarlyConstExpr,
    EarlyConstExprKind, EarlyConstName, EarlyConstTypeArg, ResolvedConstExpr,
    ResolvedConstGenericArg, ResolvedConstTypeArg,
};
use nia_ids::{LayoutBuiltin, ModuleId, ModuleIdAllocator, ValueBuiltin};
use nia_span::Span;
use nia_symbol::{SymbolId, stable_hash};
use nia_ty::IntConst;
use std::collections::BTreeMap;

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

#[path = "tests/lowered_collections.rs"]
mod lowered_collections;
#[path = "tests/resolution_contracts.rs"]
mod resolution_contracts;
#[path = "tests/test_environments.rs"]
mod test_environments;

#[test]
fn const_eval_budget_limits_steps_and_resets_between_outer_sessions() {
    let span = Span::new(4, 8);
    let mut budget = ConstEvalBudget::new(2, 4);

    budget.begin_session();
    assert!(budget.consume_step(span).is_ok());
    assert!(budget.consume_step(span).is_ok());
    let error = budget.consume_step(span).expect_err("third step must fail");
    assert_eq!(error.span, span);
    assert!(error.message.contains("2 step limit"), "{}", error.message);
    budget.end_session();

    budget.begin_session();
    assert!(budget.consume_step(span).is_ok());
    budget.end_session();
}

#[test]
fn const_eval_budget_limits_nested_calls_and_releases_depth() {
    let span = Span::new(9, 12);
    let mut budget = ConstEvalBudget::new(8, 2);
    budget.begin_session();

    assert!(budget.enter_call(span).is_ok());
    assert!(budget.enter_call(span).is_ok());
    let error = budget
        .enter_call(span)
        .expect_err("third nested call must fail");
    assert_eq!(error.span, span);
    assert!(
        error.message.contains("2 call depth limit"),
        "{}",
        error.message
    );
    budget.leave_call();
    assert!(budget.enter_call(span).is_ok());

    budget.end_session();
}

#[test]
fn struct_union_write_clears_previously_initialized_padding() {
    let all = sym("all");
    let prefix = sym("prefix");
    let padded = sym("padded");
    let marker = sym("marker");
    let word = sym("word");
    let byte = ConstAbiType::Scalar(ConstScalarType::Integer {
        bits: 8,
        signed: false,
    });
    let mut fields = BTreeMap::new();
    fields.insert(
        all,
        ConstAbiType::Array {
            element: Box::new(byte.clone()),
            len: 8,
        },
    );
    fields.insert(
        prefix,
        ConstAbiType::Array {
            element: Box::new(byte.clone()),
            len: 5,
        },
    );
    fields.insert(
        padded,
        ConstAbiType::Struct {
            fields: vec![
                ConstAbiField {
                    name: word,
                    offset: 0,
                    ty: ConstAbiType::Scalar(ConstScalarType::Integer {
                        bits: 32,
                        signed: false,
                    }),
                },
                ConstAbiField {
                    name: marker,
                    offset: 4,
                    ty: byte,
                },
            ],
            size: 8,
        },
    );
    let mut union = ConstUnionValue::new(
        fields,
        8,
        all,
        ConstValue::Array(
            (1..=8)
                .map(|value| ConstValue::Int(IntConst::unsigned(value)))
                .collect(),
        ),
        ConstEndianness::Little,
    )
    .expect("initialize full union storage");
    union
        .write(
            padded,
            ConstValue::Struct(BTreeMap::from([
                (marker, ConstValue::Int(IntConst::unsigned(0xaa))),
                (word, ConstValue::Int(IntConst::unsigned(0x1122_3344))),
            ])),
        )
        .expect("write padded struct");

    assert_eq!(
        union.read(prefix).expect("read initialized field bytes"),
        ConstValue::Array(
            [0x44, 0x33, 0x22, 0x11, 0xaa]
                .map(|value| ConstValue::Int(IntConst::unsigned(value)))
                .into(),
        )
    );
    assert_eq!(
        union.read(all).expect_err("padding must be uninitialized"),
        "const union field reads uninitialized storage"
    );
}

#[test]
fn nested_union_round_trip_preserves_raw_storage_and_wider_tail() {
    let inner = sym("inner");
    let bytes = sym("bytes");
    let narrow = sym("narrow");
    let wide = sym("wide");
    let byte = ConstAbiType::Scalar(ConstScalarType::Integer {
        bits: 8,
        signed: false,
    });
    let inner_fields = BTreeMap::from([
        (
            narrow,
            ConstAbiType::Scalar(ConstScalarType::Integer {
                bits: 16,
                signed: false,
            }),
        ),
        (
            wide,
            ConstAbiType::Scalar(ConstScalarType::Integer {
                bits: 32,
                signed: false,
            }),
        ),
    ]);
    let mut inner_value = ConstUnionValue::new(
        inner_fields.clone(),
        4,
        wide,
        ConstValue::Int(IntConst::unsigned(0x4433_2211)),
        ConstEndianness::Little,
    )
    .expect("initialize wide nested union field");
    inner_value
        .write(narrow, ConstValue::Int(IntConst::unsigned(0x6655)))
        .expect("overwrite narrow nested union field");

    let outer_fields = BTreeMap::from([
        (
            bytes,
            ConstAbiType::Array {
                element: Box::new(byte),
                len: 4,
            },
        ),
        (
            inner,
            ConstAbiType::Union {
                fields: inner_fields,
                size: 4,
            },
        ),
    ]);
    let outer = ConstUnionValue::new(
        outer_fields,
        4,
        inner,
        ConstValue::Union(inner_value),
        ConstEndianness::Little,
    )
    .expect("initialize nested union field");

    assert_eq!(
        outer.read(bytes).expect("reinterpret nested storage"),
        ConstValue::Array(
            [0x55, 0x66, 0x33, 0x44]
                .map(|value| ConstValue::Int(IntConst::unsigned(value)))
                .into(),
        )
    );
    let ConstValue::Union(decoded_inner) =
        outer.read(inner).expect("round-trip nested union storage")
    else {
        panic!("nested union field must decode as a union value");
    };
    assert_eq!(
        decoded_inner.read(wide).expect("read retained wider tail"),
        ConstValue::Int(IntConst::unsigned(0x4433_6655))
    );
}

#[test]
fn nested_union_propagates_uninitialized_tail() {
    let inner = sym("inner");
    let bytes = sym("bytes");
    let narrow = sym("narrow");
    let wide = sym("wide");
    let byte = ConstAbiType::Scalar(ConstScalarType::Integer {
        bits: 8,
        signed: false,
    });
    let inner_fields = BTreeMap::from([
        (
            narrow,
            ConstAbiType::Scalar(ConstScalarType::Integer {
                bits: 16,
                signed: false,
            }),
        ),
        (
            wide,
            ConstAbiType::Scalar(ConstScalarType::Integer {
                bits: 32,
                signed: false,
            }),
        ),
    ]);
    let inner_value = ConstUnionValue::new(
        inner_fields.clone(),
        4,
        narrow,
        ConstValue::Int(IntConst::unsigned(0x2211)),
        ConstEndianness::Little,
    )
    .expect("initialize narrow nested union field");
    let outer = ConstUnionValue::new(
        BTreeMap::from([
            (
                bytes,
                ConstAbiType::Array {
                    element: Box::new(byte),
                    len: 4,
                },
            ),
            (
                inner,
                ConstAbiType::Union {
                    fields: inner_fields,
                    size: 4,
                },
            ),
        ]),
        4,
        inner,
        ConstValue::Union(inner_value),
        ConstEndianness::Little,
    )
    .expect("initialize nested union field");

    assert_eq!(
        outer
            .read(bytes)
            .expect_err("nested unwritten tail must remain uninitialized"),
        "const union field reads uninitialized storage"
    );
}

#[test]
fn nested_union_propagates_struct_padding() {
    let all = sym("all");
    let inner = sym("inner");
    let marker = sym("marker");
    let padded = sym("padded");
    let value = sym("value");
    let byte = ConstAbiType::Scalar(ConstScalarType::Integer {
        bits: 8,
        signed: false,
    });
    let bytes = ConstAbiType::Array {
        element: Box::new(byte.clone()),
        len: 8,
    };
    let inner_fields = BTreeMap::from([
        (all, bytes.clone()),
        (
            padded,
            ConstAbiType::Struct {
                fields: vec![
                    ConstAbiField {
                        name: value,
                        offset: 0,
                        ty: ConstAbiType::Scalar(ConstScalarType::Integer {
                            bits: 32,
                            signed: false,
                        }),
                    },
                    ConstAbiField {
                        name: marker,
                        offset: 4,
                        ty: byte,
                    },
                ],
                size: 8,
            },
        ),
    ]);
    let inner_value = ConstUnionValue::new(
        inner_fields.clone(),
        8,
        padded,
        ConstValue::Struct(BTreeMap::from([
            (marker, ConstValue::Int(IntConst::unsigned(0xaa))),
            (value, ConstValue::Int(IntConst::unsigned(0x1122_3344))),
        ])),
        ConstEndianness::Little,
    )
    .expect("initialize padded nested union field");
    let outer = ConstUnionValue::new(
        BTreeMap::from([
            (all, bytes),
            (
                inner,
                ConstAbiType::Union {
                    fields: inner_fields,
                    size: 8,
                },
            ),
        ]),
        8,
        inner,
        ConstValue::Union(inner_value),
        ConstEndianness::Little,
    )
    .expect("initialize outer union from padded nested storage");

    assert_eq!(
        outer
            .read(all)
            .expect_err("nested struct padding must remain uninitialized"),
        "const union field reads uninitialized storage"
    );
}

#[test]
fn vector_union_storage_preserves_lane_order_and_endianness() {
    let bytes = sym("bytes");
    let vector = sym("vector");
    let byte = ConstAbiType::Scalar(ConstScalarType::Integer {
        bits: 8,
        signed: false,
    });
    let fields = BTreeMap::from([
        (
            bytes,
            ConstAbiType::Array {
                element: Box::new(byte),
                len: 4,
            },
        ),
        (
            vector,
            ConstAbiType::Vector {
                lane: ConstScalarType::Integer {
                    bits: 16,
                    signed: false,
                },
                lanes: 2,
                size: 4,
            },
        ),
    ]);
    let values = ConstValue::Vector(vec![
        ConstValue::Int(IntConst::unsigned(0x1122)),
        ConstValue::Int(IntConst::unsigned(0x3344)),
    ]);

    for (endianness, expected) in [
        (ConstEndianness::Little, [0x22, 0x11, 0x44, 0x33]),
        (ConstEndianness::Big, [0x11, 0x22, 0x33, 0x44]),
    ] {
        let union = ConstUnionValue::new(fields.clone(), 4, vector, values.clone(), endianness)
            .expect("initialize vector union field");
        assert_eq!(
            union.read(bytes).expect("reinterpret vector bytes"),
            ConstValue::Array(
                expected
                    .map(|value| ConstValue::Int(IntConst::unsigned(value as u128)))
                    .into(),
            )
        );
        assert_eq!(union.read(vector).expect("round-trip vector"), values);
    }
}

#[test]
fn bool_vector_union_storage_is_bit_packed() {
    let bytes = sym("bytes");
    let mask = sym("mask");
    let fields = BTreeMap::from([
        (
            bytes,
            ConstAbiType::Array {
                element: Box::new(ConstAbiType::Scalar(ConstScalarType::Integer {
                    bits: 8,
                    signed: false,
                })),
                len: 2,
            },
        ),
        (
            mask,
            ConstAbiType::Vector {
                lane: ConstScalarType::Bool,
                lanes: 16,
                size: 2,
            },
        ),
    ]);
    let values = ConstValue::Vector(
        (0..16)
            .map(|index| ConstValue::Bool(matches!(index, 0 | 9)))
            .collect(),
    );

    for (endianness, expected) in [
        (ConstEndianness::Little, [0x01, 0x02]),
        (ConstEndianness::Big, [0x02, 0x01]),
    ] {
        let union = ConstUnionValue::new(fields.clone(), 2, mask, values.clone(), endianness)
            .expect("initialize mask vector union field");
        assert_eq!(
            union.read(bytes).expect("reinterpret packed mask bytes"),
            ConstValue::Array(
                expected
                    .map(|value| ConstValue::Int(IntConst::unsigned(value as u128)))
                    .into(),
            )
        );
        assert_eq!(union.read(mask).expect("round-trip mask vector"), values);
    }
}

#[test]
fn vector_union_storage_keeps_allocation_tail_uninitialized() {
    let all = sym("all");
    let vector = sym("vector");
    let fields = BTreeMap::from([
        (
            all,
            ConstAbiType::Array {
                element: Box::new(ConstAbiType::Scalar(ConstScalarType::Integer {
                    bits: 8,
                    signed: false,
                })),
                len: 16,
            },
        ),
        (
            vector,
            ConstAbiType::Vector {
                lane: ConstScalarType::Float32,
                lanes: 3,
                size: 16,
            },
        ),
    ]);
    let union = ConstUnionValue::new(
        fields,
        16,
        vector,
        ConstValue::Vector(vec![
            ConstValue::Float(1.0),
            ConstValue::Float(-2.5),
            ConstValue::Float(3.25),
        ]),
        ConstEndianness::Little,
    )
    .expect("initialize padded vector union field");

    assert_eq!(
        union
            .read(all)
            .expect_err("vector allocation tail must remain uninitialized"),
        "const union field reads uninitialized storage"
    );
}
