// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use super::common::*;
pub(super) use nia_ast::{AssignOp, BinaryOp, UnaryOp};
pub(super) use nia_backend_ir::BackendEnumVariantPayload;
pub(super) use nia_function_ir::{
    AtomicOrder, AtomicRmwOp, FunctionArrayElements, FunctionAsmInput, FunctionAsmOutput,
    FunctionAtomic, FunctionBitIntrinsicOp, FunctionInlineAsm, FunctionMemoryIntrinsic,
    FunctionMemoryIntrinsicOp, FunctionMemoryIntrinsicSource, FunctionSliceRange,
    FunctionSwitchArm,
};
pub(super) use nia_ty::{ConstGenericArg, ConstGenericValue, IntConst};

pub(super) fn single_module_program(
    module_id: ModuleId,
    layouts: BackendLayouts,
    structs: Vec<BackendStruct>,
    unions: Vec<BackendUnion>,
    globals: Vec<BackendGlobal>,
    functions: Vec<BackendFunction>,
) -> BackendProgram {
    BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            symbol_package_identity: "test/package@0".into(),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts,
            structs,
            struct_instances: Vec::new(),
            unions,
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals,
            global_instances: Vec::new(),
            functions,
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .try_into()
        .expect("build backend modules"),
    }
}

// Function declarations, instances, and calling-convention contracts.
mod function_abi;
// Target layouts, generated symbols, and static aggregate contracts.
mod layout_and_symbols;
// Aggregate, literal, projection, and terminator expression contracts.
mod aggregate_expressions;
// Atomic, memory, builtin, operator, and cast contracts.
mod low_level_expressions;
// Tagged-union and enum expression contracts.
mod enum_expressions;
// Source-level smoke coverage for Function IR emission.
mod source_emission;
// Backend ownership and declaration-membership contracts.
mod backend_ownership;
// Backend call, inline-assembly, and static-initializer contracts.
mod backend_calls;
// Function CFG, entry, successor, and ABI mapping contracts.
mod function_structure;
// Places, addresses, storage, and assignment targets.
mod function_storage;
// Trait-object and unresolved-value contracts.
mod function_values;
