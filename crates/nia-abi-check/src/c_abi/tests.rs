// SPDX-License-Identifier: GPL-3.0-or-later
//! Expectations come from the declarations clang 23 emits for the same C
//! signatures on each target.

use super::*;

const I8: CType = CType::Int {
    bytes: 1,
    signed: true,
};
const U8: CType = CType::Int {
    bytes: 1,
    signed: false,
};
const I16: CType = CType::Int {
    bytes: 2,
    signed: true,
};
const U16: CType = CType::Int {
    bytes: 2,
    signed: false,
};
const I32: CType = CType::Int {
    bytes: 4,
    signed: true,
};
const I64: CType = CType::Int {
    bytes: 8,
    signed: true,
};
const I128: CType = CType::Int {
    bytes: 16,
    signed: true,
};
const F32: CType = CType::Float;
const F64: CType = CType::Double;
const PTR: CType = CType::Pointer;

fn align_of(ty: &CType, convention: CConvention) -> u64 {
    match ty {
        CType::Int { bytes: 8, .. } | CType::Double if convention == CConvention::I386 => 4,
        CType::Int { bytes, .. } => *bytes,
        CType::Float => 4,
        CType::Double => 8,
        CType::Pointer => convention.pointer_size(),
        CType::Array { elem, .. } => align_of(elem, convention),
        CType::Aggregate(aggregate) => aggregate.align,
    }
}

/// A C struct with natural field placement under `convention`.
fn record(convention: CConvention, fields: &[CType]) -> CType {
    let mut offset = 0u64;
    let mut align = 1;
    let mut placed = Vec::new();
    for field in fields {
        let field_align = align_of(field, convention);
        offset = offset.next_multiple_of(field_align);
        placed.push((offset, field.clone()));
        offset += field.size(convention.pointer_size());
        align = align.max(field_align);
    }
    CType::Aggregate(CAggregate {
        size: offset.next_multiple_of(align),
        align,
        fields: placed,
    })
}

fn array(elem: CType, len: u64) -> CType {
    CType::Array {
        elem: Box::new(elem),
        len,
    }
}

fn arg(convention: CConvention, param: CType) -> CArg {
    classify_c_function(convention, &[param], None)
        .args
        .remove(0)
}

fn ret(convention: CConvention, result: CType) -> CRet {
    classify_c_function(convention, &[], Some(&result)).ret
}

fn direct(ty: CAbiType) -> CArg {
    coerced(ty)
}

fn pair(elem: CAbiType) -> CAbiType {
    CAbiType::Array(Box::new(elem), 2)
}

#[test]
fn conventions_follow_the_target_matrix() {
    let convention = |name| CConvention::for_target(TargetConfig::parse(name).unwrap());
    assert_eq!(convention("x86_64-unknown-linux"), CConvention::SysV64);
    assert_eq!(convention("x86_64-apple-macos"), CConvention::SysV64);
    assert_eq!(convention("x86-unknown-linux"), CConvention::I386);
    assert_eq!(convention("x86_64-pc-windows-msvc"), CConvention::Win64);
    assert_eq!(convention("aarch64-unknown-linux"), CConvention::Aapcs64);
    assert_eq!(convention("aarch64-apple-macos"), CConvention::AppleArm64);
}

#[test]
fn small_integers_extend_where_the_convention_requires() {
    let small = [I8, U8, I16, U16, I32];
    let extensions = |convention| {
        classify_c_function(convention, &small, None)
            .args
            .into_iter()
            .map(|arg| match arg {
                CArg::Direct { extension, .. } => extension,
                other => panic!("{other:?}"),
            })
            .collect::<Vec<_>>()
    };
    let extended = vec![
        CExtension::Sign,
        CExtension::Zero,
        CExtension::Sign,
        CExtension::Zero,
        CExtension::None,
    ];
    let none = vec![CExtension::None; 5];
    assert_eq!(extensions(CConvention::SysV64), extended);
    assert_eq!(extensions(CConvention::I386), extended);
    assert_eq!(extensions(CConvention::AppleArm64), extended);
    assert_eq!(extensions(CConvention::Win64), none);
    assert_eq!(extensions(CConvention::Aapcs64), none);
    assert_eq!(
        ret(CConvention::SysV64, I8),
        CRet::Direct {
            pass: CPass::Natural,
            extension: CExtension::Sign
        }
    );
    assert_eq!(
        ret(CConvention::Aapcs64, U16),
        CRet::Direct {
            pass: CPass::Natural,
            extension: CExtension::None
        }
    );
}

#[test]
fn sysv64_splits_small_aggregates_into_eightbytes() {
    let c = CConvention::SysV64;
    let s = |fields: &[CType]| record(c, fields);
    assert_eq!(arg(c, s(&[I8])), direct(CAbiType::Int(8)));
    assert_eq!(arg(c, s(&[U8, U8, U8])), direct(CAbiType::Int(24)));
    assert_eq!(arg(c, s(&[array(U8, 5)])), direct(CAbiType::Int(40)));
    assert_eq!(arg(c, s(&[U16, U8])), direct(CAbiType::Int(32)));
    assert_eq!(arg(c, s(&[I32, CType::Float])), direct(CAbiType::Int(64)));
    assert_eq!(arg(c, s(&[CType::Float])), direct(CAbiType::Float));
    assert_eq!(
        arg(c, s(&[CType::Float, CType::Float])),
        direct(CAbiType::FloatPair)
    );
    assert_eq!(arg(c, s(&[CType::Pointer])), direct(CAbiType::Pointer));
    assert_eq!(
        arg(c, s(&[I64, I32])),
        CArg::Expand(vec![(0, CAbiType::Int(64)), (8, CAbiType::Int(32))])
    );
    assert_eq!(
        arg(c, s(&[U8, I64])),
        CArg::Expand(vec![(0, CAbiType::Int(8)), (8, CAbiType::Int(64))])
    );
    assert_eq!(
        arg(c, s(&[CType::Float, CType::Double])),
        CArg::Expand(vec![(0, CAbiType::Float), (8, CAbiType::Double)])
    );
    assert_eq!(
        arg(c, s(&[CType::Float, CType::Float, I32])),
        CArg::Expand(vec![(0, CAbiType::FloatPair), (8, CAbiType::Int(32))])
    );
    assert_eq!(
        arg(c, s(&[I32, CType::Float, CType::Float])),
        CArg::Expand(vec![(0, CAbiType::Int(64)), (8, CAbiType::Float)])
    );
    let nested = s(&[s(&[CType::Float, CType::Float]), CType::Float]);
    assert_eq!(
        arg(c, nested),
        CArg::Expand(vec![(0, CAbiType::FloatPair), (8, CAbiType::Float)])
    );
    assert_eq!(
        arg(c, s(&[array(CType::Float, 3)])),
        CArg::Expand(vec![(0, CAbiType::FloatPair), (8, CAbiType::Float)])
    );
    assert_eq!(
        arg(c, s(&[CType::Pointer, CType::Pointer])),
        CArg::Expand(vec![(0, CAbiType::Pointer), (8, CAbiType::Pointer)])
    );
    assert_eq!(arg(c, s(&[I128])), direct(CAbiType::Int(128)));
    assert_eq!(
        arg(c, s(&[CType::Double, CType::Double, CType::Double])),
        CArg::Byval { align: 8 }
    );
    assert_eq!(arg(c, s(&[I32; 5])), CArg::Byval { align: 8 });
}

#[test]
fn sysv64_returns_register_aggregates_as_literal_structs() {
    let c = CConvention::SysV64;
    let s = |fields: &[CType]| record(c, fields);
    assert_eq!(
        ret(c, s(&[I64, I32])),
        coerced_ret(CAbiType::Struct(vec![CAbiType::Int(64), CAbiType::Int(32)]))
    );
    assert_eq!(
        ret(c, s(&[CType::Double, CType::Float])),
        coerced_ret(CAbiType::Struct(vec![CAbiType::Double, CAbiType::Float]))
    );
    assert_eq!(ret(c, s(&[U8, U8, U8])), coerced_ret(CAbiType::Int(24)));
    assert_eq!(ret(c, s(&[CType::Double])), coerced_ret(CAbiType::Double));
    assert_eq!(ret(c, s(&[F64; 3])), CRet::SRet { align: 8 });
}

#[test]
fn sysv64_passes_aggregates_in_memory_when_registers_run_out() {
    let c = CConvention::SysV64;
    let pointers = record(c, &[CType::Pointer, CType::Pointer]);
    let with_ints = |count| {
        let mut params = vec![I64; count];
        params.push(pointers.clone());
        params.push(I64);
        classify_c_function(c, &params, None).args[count].clone()
    };
    assert!(matches!(with_ints(4), CArg::Expand(_)));
    assert_eq!(with_ints(5), CArg::Byval { align: 8 });

    // An `sret` result takes the first integer register.
    let mut params = vec![I64; 4];
    params.push(pointers.clone());
    let abi = classify_c_function(c, &params, Some(&record(c, &[F64; 3])));
    assert_eq!(abi.args[4], CArg::Byval { align: 8 });

    let float_double = record(c, &[CType::Float, CType::Double]);
    let one_double = record(c, &[CType::Double]);
    let mut params = vec![F64; 7];
    params.extend([one_double.clone(), float_double.clone(), CType::Double]);
    let abi = classify_c_function(c, &params, None);
    assert_eq!(abi.args[7], direct(CAbiType::Double));
    assert_eq!(abi.args[8], CArg::Byval { align: 8 });
}

#[test]
fn i386_expands_scalar_structs_and_returns_aggregates_in_memory() {
    let c = CConvention::I386;
    let s = |fields: &[CType]| record(c, fields);
    assert_eq!(
        arg(c, s(&[I32, CType::Float])),
        CArg::Expand(vec![(0, CAbiType::Int(32)), (4, CAbiType::Float)])
    );
    assert_eq!(
        arg(c, s(&[CType::Float, CType::Double])),
        CArg::Expand(vec![(0, CAbiType::Float), (4, CAbiType::Double)])
    );
    assert_eq!(
        arg(c, s(&[CType::Pointer, CType::Pointer])),
        CArg::Expand(vec![(0, CAbiType::Pointer), (4, CAbiType::Pointer)])
    );
    assert_eq!(arg(c, s(&[U8])), CArg::Byval { align: 4 });
    assert_eq!(arg(c, s(&[U8, I64])), CArg::Byval { align: 4 });
    assert_eq!(
        arg(c, s(&[array(CType::Float, 3)])),
        CArg::Byval { align: 4 }
    );
    let nested = s(&[s(&[CType::Float, CType::Float]), CType::Float]);
    assert_eq!(arg(c, nested), CArg::Byval { align: 4 });
    assert_eq!(arg(c, s(&[F64; 3])), CArg::Byval { align: 4 });
    assert_eq!(ret(c, s(&[U8])), CRet::SRet { align: 1 });
    assert_eq!(ret(c, s(&[I64, I64])), CRet::SRet { align: 4 });
}

#[test]
fn win64_passes_register_sized_aggregates_as_integers() {
    let c = CConvention::Win64;
    let s = |fields: &[CType]| record(c, fields);
    assert_eq!(arg(c, s(&[U8])), direct(CAbiType::Int(8)));
    assert_eq!(arg(c, s(&[CType::Float])), direct(CAbiType::Int(32)));
    assert_eq!(arg(c, s(&[CType::Double])), direct(CAbiType::Int(64)));
    assert_eq!(arg(c, s(&[U8, U8, U8])), CArg::Indirect { align: 1 });
    assert_eq!(arg(c, s(&[I64, I64])), CArg::Indirect { align: 8 });
    assert_eq!(arg(c, I128), CArg::Indirect { align: 16 });
    assert_eq!(ret(c, I128), coerced_ret(CAbiType::IntVectorPair));
    assert_eq!(
        ret(c, s(&[CType::Float, CType::Float])),
        coerced_ret(CAbiType::Int(64))
    );
    assert_eq!(ret(c, s(&[U16, U16, U16])), CRet::SRet { align: 2 });
}

#[test]
fn aapcs64_passes_homogeneous_float_aggregates_in_float_registers() {
    for (c, align_stack) in [
        (CConvention::Aapcs64, Some(8)),
        (CConvention::AppleArm64, None),
    ] {
        let s = |fields: &[CType]| record(c, fields);
        assert_eq!(
            arg(c, s(&[F32; 3])),
            CArg::Direct {
                pass: CPass::Coerced(CAbiType::Array(Box::new(CAbiType::Float), 3)),
                extension: CExtension::None,
                align_stack,
            }
        );
        let nested = s(&[s(&[CType::Float, CType::Float]), CType::Float]);
        assert_eq!(
            arg(c, nested),
            CArg::Direct {
                pass: CPass::Coerced(CAbiType::Array(Box::new(CAbiType::Float), 3)),
                extension: CExtension::None,
                align_stack,
            }
        );
        assert_eq!(
            ret(c, s(&[F64; 4])),
            CRet::Direct {
                pass: CPass::Natural,
                extension: CExtension::None
            }
        );
        assert_eq!(arg(c, s(&[F64; 5])), CArg::Indirect { align: 8 });
        assert_eq!(ret(c, s(&[F64; 5])), CRet::SRet { align: 8 });
    }
}

#[test]
fn aapcs64_passes_other_small_aggregates_as_integer_words() {
    let c = CConvention::Aapcs64;
    let s = |fields: &[CType]| record(c, fields);
    assert_eq!(arg(c, s(&[U8, U8, U8])), direct(CAbiType::Int(64)));
    assert_eq!(ret(c, s(&[U8, U8, U8])), coerced_ret(CAbiType::Int(24)));
    assert_eq!(
        arg(c, s(&[CType::Float, CType::Double])),
        direct(pair(CAbiType::Int(64)))
    );
    assert_eq!(arg(c, s(&[CType::Pointer])), direct(CAbiType::Pointer));
    assert_eq!(
        arg(c, s(&[CType::Pointer, CType::Pointer])),
        direct(pair(CAbiType::Pointer))
    );
    assert_eq!(
        arg(c, s(&[CType::Pointer, I64])),
        direct(pair(CAbiType::Int(64)))
    );
    assert_eq!(ret(c, s(&[CType::Pointer])), coerced_ret(CAbiType::Int(64)));
    assert_eq!(
        ret(c, s(&[CType::Pointer, CType::Pointer])),
        coerced_ret(pair(CAbiType::Int(64)))
    );
    assert_eq!(arg(c, s(&[I128])), direct(CAbiType::Int(128)));
    assert_eq!(ret(c, s(&[I128])), coerced_ret(CAbiType::Int(128)));
    assert_eq!(arg(c, s(&[PTR; 3])), CArg::Indirect { align: 8 });
}

#[test]
fn variadic_extras_receive_default_promotions() {
    assert_eq!(I8.promoted_variadic(), I32);
    assert_eq!(U16.promoted_variadic(), I32);
    assert_eq!(CType::Float.promoted_variadic(), CType::Double);
    assert_eq!(I64.promoted_variadic(), I64);
}
