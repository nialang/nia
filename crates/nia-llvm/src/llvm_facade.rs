// SPDX-License-Identifier: GPL-3.0-or-later
//! Public facade over the internal LLVM wrapper layer.
//!
//! The backend imports LLVM through small typed wrappers instead of using raw
//! `llvm-sys` handles everywhere. Re-exporting them here keeps codegen modules
//! decoupled from the wrapper module layout.

pub use crate::llvm_api::{
    AddressSpace, AtomicOrdering, AtomicRMWBinOp, FloatPredicate, IntPredicate, LlvmError,
    LlvmResult, OptimizationLevel,
};

/// Raw LLVM bindings for the few backend integrations not yet typed here.
pub mod llvm_sys {
    pub use llvm_sys::*;
}

/// Instruction builder wrapper.
pub mod builder {
    pub use crate::llvm_api::Builder;
}

/// Context ownership and context-created values.
pub mod context {
    pub use crate::llvm_api::Context;
}

/// Module ownership, linking, and declaration APIs.
pub mod module {
    pub use crate::llvm_api::{Linkage, Module};
}

/// Native target discovery, configuration, and object emission.
pub mod target {
    pub use crate::llvm_api::{TargetMachine, TargetMachineIdentity};
}

/// Typed LLVM type handles and conversions.
pub mod types {
    pub use crate::llvm_api::{
        ArrayType, AsTypeRef, BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FloatType,
        FunctionType, IntType, PointerType, ScalableVectorType, StructType, VectorType, VoidType,
    };
}

/// Typed LLVM value handles and conversions.
pub mod values {
    pub use crate::llvm_api::{
        ArrayValue, AsValueRef, BasicMetadataValueEnum, BasicValue, BasicValueEnum, CallSiteValue,
        FloatValue, FunctionValue, GlobalValue, InstructionValue, IntValue, PhiValue, PointerValue,
        StructValue,
    };
}

/// LLVM basic-block handles.
pub mod basic_block {
    pub use crate::llvm_api::BasicBlock;
}

/// LLVM function attribute handles and attachment locations.
pub mod attributes {
    pub use crate::llvm_api::{Attribute, AttributeLoc};
}

/// Supported LLVM intrinsic lookup and declaration helpers.
pub mod intrinsics {
    use crate::llvm_api::{BasicTypeEnum, FunctionValue, Module};

    #[derive(Clone, Copy)]
    /// A recognized LLVM intrinsic name.
    pub struct Intrinsic {
        name: &'static str,
    }

    impl Intrinsic {
        /// Resolves an intrinsic supported by Nia's code generator.
        pub fn find(name: &str) -> Option<Self> {
            match name {
                "llvm.trap" => Some(Self { name: "llvm.trap" }),
                "llvm.debugtrap" => Some(Self {
                    name: "llvm.debugtrap",
                }),
                "llvm.ctpop" => Some(Self { name: "llvm.ctpop" }),
                "llvm.ctlz" => Some(Self { name: "llvm.ctlz" }),
                "llvm.cttz" => Some(Self { name: "llvm.cttz" }),
                "llvm.sadd.with.overflow" => Some(Self {
                    name: "llvm.sadd.with.overflow",
                }),
                "llvm.uadd.with.overflow" => Some(Self {
                    name: "llvm.uadd.with.overflow",
                }),
                "llvm.ssub.with.overflow" => Some(Self {
                    name: "llvm.ssub.with.overflow",
                }),
                "llvm.usub.with.overflow" => Some(Self {
                    name: "llvm.usub.with.overflow",
                }),
                "llvm.smul.with.overflow" => Some(Self {
                    name: "llvm.smul.with.overflow",
                }),
                "llvm.umul.with.overflow" => Some(Self {
                    name: "llvm.umul.with.overflow",
                }),
                "llvm.bswap" => Some(Self { name: "llvm.bswap" }),
                "llvm.sqrt" => Some(Self { name: "llvm.sqrt" }),
                "llvm.floor" => Some(Self { name: "llvm.floor" }),
                "llvm.ceil" => Some(Self { name: "llvm.ceil" }),
                "llvm.trunc" => Some(Self { name: "llvm.trunc" }),
                "llvm.round" => Some(Self { name: "llvm.round" }),
                _ => None,
            }
        }

        /// Gets or inserts this intrinsic's declaration for `types`.
        pub fn get_declaration<'ctx>(
            self,
            module: &Module<'ctx>,
            types: &[BasicTypeEnum<'ctx>],
        ) -> Option<FunctionValue<'ctx>> {
            module.get_intrinsic_declaration(self.name, types)
        }
    }
}
