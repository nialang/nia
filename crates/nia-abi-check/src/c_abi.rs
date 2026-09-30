// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-target C calling convention classification.
//!
//! The classifier maps each C value of a signature to the machine form the
//! target's C calling convention passes it in, following the rules clang
//! implements for the same target. It works on byte layouts only, so it has no
//! LLVM or type-store dependency; code generation turns its result into LLVM
//! types and attributes.

use nia_target::{Arch, Os, TargetConfig};

/// A value as a C calling convention sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CType {
    /// An integer of `bytes` bytes: 1, 2, 4, 8, or 16.
    Int { bytes: u64, signed: bool },
    /// `float`.
    Float,
    /// `double`.
    Double,
    /// Any data or function pointer.
    Pointer,
    /// A fixed-length array; it occurs only as an aggregate field.
    Array { elem: Box<CType>, len: u64 },
    /// A struct with its complete byte layout.
    Aggregate(CAggregate),
}

/// Byte layout of a C struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CAggregate {
    pub size: u64,
    pub align: u64,
    /// Fields in declaration order with their byte offsets.
    pub fields: Vec<(u64, CType)>,
}

/// A C calling convention of a maintained target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CConvention {
    /// System V AMD64: Linux and macOS on x86_64.
    SysV64,
    /// System V i386: Linux on x86.
    I386,
    /// Microsoft x64.
    Win64,
    /// AAPCS64 as used by Linux.
    Aapcs64,
    /// Apple's arm64 variant of AAPCS64.
    AppleArm64,
}

impl CConvention {
    /// The C calling convention of `target`.
    pub fn for_target(target: TargetConfig) -> Self {
        match (target.arch(), target.os()) {
            (Arch::X86_64, Os::Windows) => Self::Win64,
            (Arch::X86_64, _) => Self::SysV64,
            (Arch::X86, _) => Self::I386,
            (Arch::Aarch64, Os::Macos) => Self::AppleArm64,
            (Arch::Aarch64, _) => Self::Aapcs64,
        }
    }

    /// Pointer size in bytes.
    pub fn pointer_size(self) -> u64 {
        match self {
            Self::I386 => 4,
            Self::SysV64 | Self::Win64 | Self::Aapcs64 | Self::AppleArm64 => 8,
        }
    }
}

/// A machine type a C value is passed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CAbiType {
    /// An integer of the given bit width, which need not be a power of two.
    Int(u32),
    Float,
    Double,
    Pointer,
    /// `<2 x float>`.
    FloatPair,
    /// `<2 x i64>`, the Win64 return register form of a 128-bit integer.
    IntVectorPair,
    Array(Box<CAbiType>, u32),
    /// A literal (unnamed) struct with natural field placement.
    Struct(Vec<CAbiType>),
}

/// How a direct value reaches its machine form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CPass {
    /// In its own representation: a scalar's type or an aggregate's struct.
    Natural,
    /// Reinterpreted through memory as another machine type.
    Coerced(CAbiType),
}

/// Integer extension a small integer receives at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CExtension {
    None,
    Sign,
    Zero,
}

/// Machine form of one C argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CArg {
    /// A zero-sized aggregate has no machine argument.
    Ignore,
    /// One machine argument.
    Direct {
        pass: CPass,
        extension: CExtension,
        /// Minimum stack slot alignment when the value is passed on the stack.
        align_stack: Option<u64>,
    },
    /// Several machine arguments, each read from the aggregate at its offset.
    Expand(Vec<(u64, CAbiType)>),
    /// A pointer to a copy the callee owns (`byval`).
    Byval { align: u64 },
    /// A pointer to a copy the caller owns.
    Indirect { align: u64 },
}

/// Machine form of a C result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CRet {
    Void,
    Direct {
        pass: CPass,
        extension: CExtension,
    },
    /// Written through a hidden leading pointer (`sret`).
    SRet {
        align: u64,
    },
}

/// Machine form of a complete C call or definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CFunctionAbi {
    pub args: Vec<CArg>,
    pub ret: CRet,
}

impl CType {
    /// Size in bytes on a convention with `pointer_size`-byte pointers.
    pub fn size(&self, pointer_size: u64) -> u64 {
        match self {
            Self::Int { bytes, .. } => *bytes,
            Self::Float => 4,
            Self::Double => 8,
            Self::Pointer => pointer_size,
            Self::Array { elem, len } => elem.size(pointer_size) * len,
            Self::Aggregate(aggregate) => aggregate.size,
        }
    }

    /// The type a variadic extra argument has after C's default argument
    /// promotions: small integers widen to `int`, `float` to `double`.
    pub fn promoted_variadic(self) -> Self {
        match self {
            // Every value of a narrower integer type, signed or not, is an `int`.
            Self::Int { bytes, .. } if bytes < 4 => Self::Int {
                bytes: 4,
                signed: true,
            },
            Self::Float => Self::Double,
            other => other,
        }
    }
}

/// A scalar leaf of an aggregate at its byte offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Leaf {
    Int(u64),
    Float,
    Double,
    Pointer,
}

impl Leaf {
    fn size(self, pointer_size: u64) -> u64 {
        match self {
            Self::Int(bytes) => bytes,
            Self::Float => 4,
            Self::Double => 8,
            Self::Pointer => pointer_size,
        }
    }
}

fn push_leaves(ty: &CType, offset: u64, pointer_size: u64, leaves: &mut Vec<(u64, Leaf)>) {
    match ty {
        CType::Int { bytes, .. } => leaves.push((offset, Leaf::Int(*bytes))),
        CType::Float => leaves.push((offset, Leaf::Float)),
        CType::Double => leaves.push((offset, Leaf::Double)),
        CType::Pointer => leaves.push((offset, Leaf::Pointer)),
        CType::Array { elem, len } => {
            let stride = elem.size(pointer_size);
            for index in 0..*len {
                push_leaves(elem, offset + index * stride, pointer_size, leaves);
            }
        }
        CType::Aggregate(aggregate) => {
            for (field_offset, field) in &aggregate.fields {
                push_leaves(field, offset + field_offset, pointer_size, leaves);
            }
        }
    }
}

fn leaves(aggregate: &CAggregate, pointer_size: u64) -> Vec<(u64, Leaf)> {
    let mut leaves = Vec::new();
    for (offset, field) in &aggregate.fields {
        push_leaves(field, *offset, pointer_size, &mut leaves);
    }
    leaves
}

fn scalar_extension(ty: &CType) -> CExtension {
    match ty {
        CType::Int { bytes, signed } if *bytes < 4 => {
            if *signed {
                CExtension::Sign
            } else {
                CExtension::Zero
            }
        }
        _ => CExtension::None,
    }
}

fn natural(extension: CExtension) -> CArg {
    CArg::Direct {
        pass: CPass::Natural,
        extension,
        align_stack: None,
    }
}

fn coerced(ty: CAbiType) -> CArg {
    CArg::Direct {
        pass: CPass::Coerced(ty),
        extension: CExtension::None,
        align_stack: None,
    }
}

fn coerced_ret(ty: CAbiType) -> CRet {
    CRet::Direct {
        pass: CPass::Coerced(ty),
        extension: CExtension::None,
    }
}

fn int_bits(bytes: u64) -> u32 {
    u32::try_from(bytes * 8).unwrap_or(u32::MAX)
}

/// Classifies a C function. `params` lists every argument in order,
/// including variadic extras after their default promotions; `ret` is `None`
/// for a `void` result.
pub fn classify_c_function(
    convention: CConvention,
    params: &[CType],
    ret: Option<&CType>,
) -> CFunctionAbi {
    match convention {
        CConvention::SysV64 => sysv64::classify(params, ret),
        CConvention::I386 => i386::classify(params, ret),
        CConvention::Win64 => win64::classify(params, ret),
        CConvention::Aapcs64 => aapcs64::classify(params, ret, false),
        CConvention::AppleArm64 => aapcs64::classify(params, ret, true),
    }
}

mod sysv64 {
    use super::*;

    const POINTER_SIZE: u64 = 8;
    const INT_REGISTERS: u32 = 6;
    const SSE_REGISTERS: u32 = 8;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Class {
        None,
        Integer,
        Sse,
    }

    fn merge(left: Class, right: Class) -> Class {
        match (left, right) {
            (Class::None, other) | (other, Class::None) => other,
            (Class::Integer, _) | (_, Class::Integer) => Class::Integer,
            (Class::Sse, Class::Sse) => Class::Sse,
        }
    }

    /// Classes of the two eightbytes, or `None` when the value is passed in
    /// memory.
    fn classify_eightbytes(aggregate: &CAggregate, leaves: &[(u64, Leaf)]) -> Option<[Class; 2]> {
        if aggregate.size > 16 {
            return None;
        }
        let mut classes = [Class::None; 2];
        for (offset, leaf) in leaves {
            let class = match leaf {
                Leaf::Int(_) | Leaf::Pointer => Class::Integer,
                Leaf::Float | Leaf::Double => Class::Sse,
            };
            let end = offset + leaf.size(POINTER_SIZE);
            for (index, slot) in classes.iter_mut().enumerate() {
                let start = index as u64 * 8;
                if *offset < start + 8 && end > start {
                    *slot = merge(*slot, class);
                }
            }
        }
        Some(classes)
    }

    fn has_data(leaves: &[(u64, Leaf)], start: u64, end: u64) -> bool {
        leaves
            .iter()
            .any(|(offset, leaf)| *offset < end && offset + leaf.size(POINTER_SIZE) > start)
    }

    fn part_type(
        aggregate: &CAggregate,
        leaves: &[(u64, Leaf)],
        eightbyte: u64,
        class: Class,
    ) -> CAbiType {
        let start = eightbyte * 8;
        let end = aggregate.size.min(start + 8);
        let leaf_at = leaves
            .iter()
            .find(|(offset, _)| *offset == start)
            .map(|(_, leaf)| *leaf);
        match class {
            Class::Sse => match leaf_at {
                Some(Leaf::Double) => CAbiType::Double,
                _ if leaves
                    .iter()
                    .any(|(offset, leaf)| *offset == start + 4 && *leaf == Leaf::Float) =>
                {
                    CAbiType::FloatPair
                }
                _ if !has_data(leaves, start + 4, end) => CAbiType::Float,
                _ => CAbiType::Double,
            },
            Class::Integer | Class::None => match leaf_at {
                Some(Leaf::Pointer) => CAbiType::Pointer,
                Some(Leaf::Int(8)) => CAbiType::Int(64),
                Some(Leaf::Int(bytes)) if bytes < 8 && !has_data(leaves, start + bytes, end) => {
                    CAbiType::Int(int_bits(bytes))
                }
                _ => CAbiType::Int(int_bits(end - start)),
            },
        }
    }

    fn natural_align(ty: &CAbiType) -> u64 {
        match ty {
            CAbiType::Int(bits) => u64::from(bits.div_ceil(8)).next_power_of_two().min(8),
            CAbiType::Float => 4,
            CAbiType::Double | CAbiType::Pointer | CAbiType::FloatPair => 8,
            CAbiType::IntVectorPair => 16,
            CAbiType::Array(elem, _) => natural_align(elem),
            CAbiType::Struct(fields) => fields.iter().map(natural_align).max().unwrap_or(1),
        }
    }

    fn natural_size(ty: &CAbiType) -> u64 {
        match ty {
            CAbiType::Int(bits) => u64::from(bits.div_ceil(8)),
            CAbiType::Float => 4,
            _ => 8,
        }
    }

    /// The machine parts of a register-passed aggregate. As in clang, a low
    /// part narrower than an eightbyte widens when the high part's own
    /// alignment would otherwise place it before offset 8.
    fn parts(aggregate: &CAggregate, leaves: &[(u64, Leaf)], classes: [Class; 2]) -> Vec<CAbiType> {
        if leaves == [(0, Leaf::Int(16))] {
            return vec![CAbiType::Int(128)];
        }
        let mut low = part_type(aggregate, leaves, 0, classes[0]);
        if classes[1] == Class::None {
            return vec![low];
        }
        let high = part_type(aggregate, leaves, 1, classes[1]);
        if natural_size(&low) < 8 && natural_align(&high) < 8 {
            low = match classes[0] {
                Class::Sse => CAbiType::Double,
                Class::Integer | Class::None => CAbiType::Int(64),
            };
        }
        vec![low, high]
    }

    fn needed_registers(classes: [Class; 2]) -> (u32, u32) {
        classes
            .iter()
            .fold((0, 0), |(int, sse), class| match class {
                Class::Integer => (int + 1, sse),
                Class::Sse => (int, sse + 1),
                Class::None => (int, sse),
            })
    }

    pub(super) fn classify(params: &[CType], ret: Option<&CType>) -> CFunctionAbi {
        let mut free_int = INT_REGISTERS;
        let mut free_sse = SSE_REGISTERS;
        let ret = match ret {
            None => CRet::Void,
            Some(CType::Aggregate(aggregate)) if aggregate.size == 0 => CRet::Void,
            Some(CType::Aggregate(aggregate)) => {
                let leaves = leaves(aggregate, POINTER_SIZE);
                match classify_eightbytes(aggregate, &leaves) {
                    None => {
                        free_int -= 1;
                        CRet::SRet {
                            align: aggregate.align,
                        }
                    }
                    Some(classes) => {
                        let mut parts = parts(aggregate, &leaves, classes);
                        coerced_ret(if parts.len() == 1 {
                            parts.remove(0)
                        } else {
                            CAbiType::Struct(parts)
                        })
                    }
                }
            }
            Some(scalar) => CRet::Direct {
                pass: CPass::Natural,
                extension: scalar_extension(scalar),
            },
        };
        let args = params
            .iter()
            .map(|param| match param {
                CType::Aggregate(aggregate) if aggregate.size == 0 => CArg::Ignore,
                CType::Aggregate(aggregate) => {
                    let byval = CArg::Byval {
                        align: aggregate.align.max(8),
                    };
                    let leaves = leaves(aggregate, POINTER_SIZE);
                    let Some(classes) = classify_eightbytes(aggregate, &leaves) else {
                        return byval;
                    };
                    let (int, sse) = needed_registers(classes);
                    if int > free_int || sse > free_sse {
                        return byval;
                    }
                    free_int -= int;
                    free_sse -= sse;
                    let mut parts = parts(aggregate, &leaves, classes);
                    if parts.len() == 1 {
                        coerced(parts.remove(0))
                    } else {
                        CArg::Expand(
                            parts
                                .into_iter()
                                .zip([0, 8])
                                .map(|(ty, offset)| (offset, ty))
                                .collect(),
                        )
                    }
                }
                scalar => {
                    let (int, sse) = match scalar {
                        CType::Float | CType::Double => (0, 1),
                        CType::Int { bytes: 16, .. } => (2, 0),
                        _ => (1, 0),
                    };
                    if int <= free_int && sse <= free_sse {
                        free_int -= int;
                        free_sse -= sse;
                    }
                    natural(scalar_extension(scalar))
                }
            })
            .collect();
        CFunctionAbi { args, ret }
    }
}

mod i386 {
    use super::*;

    const POINTER_SIZE: u64 = 4;

    /// Whether clang expands the struct into its fields: every field is a
    /// direct 32- or 64-bit scalar, the fields leave no padding, and the
    /// struct is at most 16 bytes. The stack image is the same as `byval`.
    fn expanded_fields(aggregate: &CAggregate) -> Option<Vec<(u64, CAbiType)>> {
        if aggregate.size > 16 {
            return None;
        }
        let mut fields = Vec::with_capacity(aggregate.fields.len());
        let mut covered = 0;
        for (offset, field) in &aggregate.fields {
            let ty = match field {
                CType::Int { bytes: 4, .. } => CAbiType::Int(32),
                CType::Int { bytes: 8, .. } => CAbiType::Int(64),
                CType::Float => CAbiType::Float,
                CType::Double => CAbiType::Double,
                CType::Pointer => CAbiType::Pointer,
                _ => return None,
            };
            covered += field.size(POINTER_SIZE);
            fields.push((*offset, ty));
        }
        (covered == aggregate.size).then_some(fields)
    }

    pub(super) fn classify(params: &[CType], ret: Option<&CType>) -> CFunctionAbi {
        let ret = match ret {
            None => CRet::Void,
            Some(CType::Aggregate(aggregate)) if aggregate.size == 0 => CRet::Void,
            Some(CType::Aggregate(aggregate)) => CRet::SRet {
                align: aggregate.align,
            },
            Some(scalar) => CRet::Direct {
                pass: CPass::Natural,
                extension: scalar_extension(scalar),
            },
        };
        let args = params
            .iter()
            .map(|param| match param {
                CType::Aggregate(aggregate) if aggregate.size == 0 => CArg::Ignore,
                CType::Aggregate(aggregate) => match expanded_fields(aggregate) {
                    Some(fields) => CArg::Expand(fields),
                    None => CArg::Byval {
                        align: aggregate.align.max(4),
                    },
                },
                scalar => natural(scalar_extension(scalar)),
            })
            .collect();
        CFunctionAbi { args, ret }
    }
}

mod win64 {
    use super::*;

    pub(super) fn classify(params: &[CType], ret: Option<&CType>) -> CFunctionAbi {
        let ret = match ret {
            None => CRet::Void,
            Some(CType::Aggregate(aggregate)) if aggregate.size == 0 => CRet::Void,
            Some(CType::Aggregate(aggregate)) => match aggregate.size {
                1 | 2 | 4 | 8 => coerced_ret(CAbiType::Int(int_bits(aggregate.size))),
                _ => CRet::SRet {
                    align: aggregate.align,
                },
            },
            Some(CType::Int { bytes: 16, .. }) => coerced_ret(CAbiType::IntVectorPair),
            Some(_) => CRet::Direct {
                pass: CPass::Natural,
                extension: CExtension::None,
            },
        };
        let args = params
            .iter()
            .map(|param| match param {
                CType::Aggregate(aggregate) if aggregate.size == 0 => CArg::Ignore,
                CType::Aggregate(aggregate) => match aggregate.size {
                    1 | 2 | 4 | 8 => coerced(CAbiType::Int(int_bits(aggregate.size))),
                    _ => CArg::Indirect {
                        align: aggregate.align,
                    },
                },
                CType::Int { bytes: 16, .. } => CArg::Indirect { align: 16 },
                _ => natural(CExtension::None),
            })
            .collect();
        CFunctionAbi { args, ret }
    }
}

mod aapcs64 {
    use super::*;

    const POINTER_SIZE: u64 = 8;

    /// The element type and count of a homogeneous floating-point aggregate.
    fn homogeneous_float(leaves: &[(u64, Leaf)]) -> Option<(CAbiType, u32)> {
        let first = leaves.first()?.1;
        let elem = match first {
            Leaf::Float => CAbiType::Float,
            Leaf::Double => CAbiType::Double,
            Leaf::Int(_) | Leaf::Pointer => return None,
        };
        let count = u32::try_from(leaves.len()).ok()?;
        ((1..=4).contains(&count) && leaves.iter().all(|(_, leaf)| *leaf == first))
            .then_some((elem, count))
    }

    fn is_pointer_words(aggregate: &CAggregate, leaves: &[(u64, Leaf)]) -> bool {
        aggregate.size.is_multiple_of(8)
            && leaves.len() as u64 == aggregate.size / 8
            && leaves.iter().enumerate().all(|(index, (offset, leaf))| {
                *leaf == Leaf::Pointer && *offset == index as u64 * 8
            })
    }

    pub(super) fn classify(params: &[CType], ret: Option<&CType>, apple: bool) -> CFunctionAbi {
        let extension = |ty: &CType| {
            if apple {
                scalar_extension(ty)
            } else {
                CExtension::None
            }
        };
        let ret = match ret {
            None => CRet::Void,
            Some(CType::Aggregate(aggregate)) if aggregate.size == 0 => CRet::Void,
            Some(CType::Aggregate(aggregate)) => {
                let leaves = leaves(aggregate, POINTER_SIZE);
                if homogeneous_float(&leaves).is_some() {
                    CRet::Direct {
                        pass: CPass::Natural,
                        extension: CExtension::None,
                    }
                } else if aggregate.size > 16 {
                    CRet::SRet {
                        align: aggregate.align,
                    }
                } else if aggregate.align == 16 {
                    coerced_ret(CAbiType::Int(128))
                } else if aggregate.size <= 8 {
                    coerced_ret(CAbiType::Int(int_bits(aggregate.size)))
                } else {
                    coerced_ret(CAbiType::Array(Box::new(CAbiType::Int(64)), 2))
                }
            }
            Some(scalar) => CRet::Direct {
                pass: CPass::Natural,
                extension: extension(scalar),
            },
        };
        let args = params
            .iter()
            .map(|param| match param {
                CType::Aggregate(aggregate) if aggregate.size == 0 => CArg::Ignore,
                CType::Aggregate(aggregate) => {
                    let leaves = leaves(aggregate, POINTER_SIZE);
                    if let Some((elem, count)) = homogeneous_float(&leaves) {
                        CArg::Direct {
                            pass: CPass::Coerced(CAbiType::Array(Box::new(elem), count)),
                            extension: CExtension::None,
                            align_stack: (!apple).then_some(8),
                        }
                    } else if aggregate.size > 16 {
                        CArg::Indirect {
                            align: aggregate.align,
                        }
                    } else if aggregate.align == 16 {
                        coerced(CAbiType::Int(128))
                    } else if is_pointer_words(aggregate, &leaves) {
                        coerced(if aggregate.size == 8 {
                            CAbiType::Pointer
                        } else {
                            CAbiType::Array(Box::new(CAbiType::Pointer), 2)
                        })
                    } else if aggregate.size <= 8 {
                        coerced(CAbiType::Int(64))
                    } else {
                        coerced(CAbiType::Array(Box::new(CAbiType::Int(64)), 2))
                    }
                }
                scalar => natural(extension(scalar)),
            })
            .collect();
        CFunctionAbi { args, ret }
    }
}

#[cfg(test)]
mod tests;
