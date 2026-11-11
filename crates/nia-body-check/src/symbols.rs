// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ids::{BuiltinAssociatedConst, BuiltinFunction, BuiltinTrait, BuiltinTraitMethod};
use nia_symbol::{SymbolId, ToSymbolId, known};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AsmConfigField {
    Code,
    Inputs,
    Outputs,
    Clobbers,
    Options,
}

pub(crate) fn asm_config_field(name: SymbolId) -> Option<AsmConfigField> {
    match name {
        known::CODE => Some(AsmConfigField::Code),
        known::INPUTS => Some(AsmConfigField::Inputs),
        known::OUTPUTS_LOWER => Some(AsmConfigField::Outputs),
        known::CLOBBERS => Some(AsmConfigField::Clobbers),
        known::OPTIONS => Some(AsmConfigField::Options),
        _ => None,
    }
}

pub(crate) fn builtin_function_symbol(name: SymbolId) -> Option<BuiltinFunction> {
    BuiltinFunction::ALL
        .into_iter()
        .find(|builtin| builtin.symbol_id() == name)
}

pub(crate) fn builtin_associated_const_symbol(
    trait_id: BuiltinTrait,
    name: SymbolId,
) -> Option<BuiltinAssociatedConst> {
    match trait_id {
        BuiltinTrait::Simd if name == known::LANES => Some(BuiltinAssociatedConst::Lanes),
        _ => None,
    }
}

pub(crate) fn builtin_trait_method_symbol(name: SymbolId) -> Option<BuiltinTraitMethod> {
    match name {
        known::ADD => Some(BuiltinTraitMethod::Add),
        known::SUB => Some(BuiltinTraitMethod::Sub),
        known::MUL => Some(BuiltinTraitMethod::Mul),
        known::DIV => Some(BuiltinTraitMethod::Div),
        known::REM => Some(BuiltinTraitMethod::Rem),
        known::NEG => Some(BuiltinTraitMethod::Neg),
        known::LOGICAL_NOT => Some(BuiltinTraitMethod::Not),
        known::BIT_NOT => Some(BuiltinTraitMethod::BitNot),
        known::BIT_AND => Some(BuiltinTraitMethod::BitAnd),
        known::BIT_OR => Some(BuiltinTraitMethod::BitOr),
        known::BIT_XOR => Some(BuiltinTraitMethod::BitXor),
        known::SHL => Some(BuiltinTraitMethod::Shl),
        known::SHR => Some(BuiltinTraitMethod::Shr),
        known::EQ => Some(BuiltinTraitMethod::Eq),
        known::NE => Some(BuiltinTraitMethod::Ne),
        known::LT => Some(BuiltinTraitMethod::Lt),
        known::LE => Some(BuiltinTraitMethod::Le),
        known::GT => Some(BuiltinTraitMethod::Gt),
        known::GE => Some(BuiltinTraitMethod::Ge),
        known::DEREF => Some(BuiltinTraitMethod::Deref),
        known::DEREF_MUT => Some(BuiltinTraitMethod::DerefMut),
        known::INDEX => Some(BuiltinTraitMethod::Index),
        known::INDEX_MUT => Some(BuiltinTraitMethod::IndexMut),
        known::SLICE => Some(BuiltinTraitMethod::Slice),
        known::SLICE_MUT => Some(BuiltinTraitMethod::SliceMut),
        known::ITER_METHOD => Some(BuiltinTraitMethod::IterableIter),
        known::NEXT => Some(BuiltinTraitMethod::IteratorNext),
        known::INTO_ERROR => Some(BuiltinTraitMethod::IntoError),
        _ => None,
    }
}
