// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_abi_check::check_module_abi;
use nia_body_check::{BodyCheckInput, check_module_bodies_with_program_signatures_and_layouts};
use nia_defs::{DefKind, VisibleExtensionMethod, VisibleExtensionMethods, collect_module_defs};
use nia_flow_check::check_module_flow;
use nia_function_ir::{
    FunctionArrayElements, FunctionBlockId, FunctionExpr, FunctionExprKind, FunctionOp,
    FunctionTerminator,
};
use nia_function_lower::{FunctionTypeContext, lower_function_body};
use nia_ids::{DefId, GlobalDefId, LocalId, ModuleIdAllocator};
use nia_item_signatures::{
    ItemSignatureInput, ItemSignatureSource, ProgramFunctionSignature, collect_item_signatures,
};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_layout::TargetDataLayout;
use nia_local_resolve::resolve_module_locals;
use nia_node_id::NodeOriginTable;
use nia_parser::parse_module_with_symbols;
use nia_program_signatures::{ProgramSignatureContext, ProgramSignatureLookup};
use nia_sema_ir::SemanticUseTable;
use nia_source::SourcePath;
use nia_static_ir::StaticInit;
use nia_symbol::SymbolId;
use nia_symbol_table::SymbolTable;
use nia_type_lower::{
    ProgramDefsContext, TypeLowering, TypeLoweringContext, lower_module_types_with_context,
};
use nia_type_normalize::normalize_module_types;
use nia_type_resolve::resolve_module_types_with_symbols;
use nia_value_resolve::resolve_module_values;
use std::collections::HashMap;
use std::sync::Arc;

mod test_fixture;
use test_fixture::*;
mod test_assertions;
use test_assertions::*;
mod program_signature_fixture;
use program_signature_fixture::*;
mod program_facts_fixture;
use program_facts_fixture::*;
mod lowering_pipeline;
use lowering_pipeline::*;
mod lowering_wrappers;
use lowering_wrappers::*;

#[test]
fn vtable_owner_payloads_match_semantic_integer_consts() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let ty = type_store
        .append_for_module(module_id)
        .primitive(nia_ty::PrimitiveTy::I32);
    let trait_id = nia_ids::TraitId::Source(GlobalDefId {
        module_id,
        def_id: DefId(1),
    });
    let key = BackendTraitObjectVtableKey {
        self_ty: ty,
        object_ty: ty,
    };
    let signed = nia_ty::ConstGenericArg {
        ty,
        value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::signed_bits(7)),
    };
    let unsigned = nia_ty::ConstGenericArg {
        ty,
        value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(7)),
    };
    let left = BackendTraitObjectVtable {
        key: key.clone(),
        trait_id,
        trait_args: vec![ty],
        trait_const_args: vec![signed.clone()],
        entries: vec![],
        span: nia_span::Span::default(),
    };
    let right = BackendTraitObjectVtable {
        key,
        trait_id,
        trait_args: vec![ty],
        trait_const_args: vec![unsigned],
        entries: vec![],
        span: nia_span::Span::default(),
    };

    assert!(backend_vtable_payloads_match(&type_store, &left, &right));
    let mut conflicting = right;
    conflicting.trait_id = nia_ids::TraitId::Builtin(nia_ids::BuiltinTrait::Sized);
    assert!(!backend_vtable_payloads_match(
        &type_store,
        &left,
        &conflicting
    ));
}

#[test]
fn vtable_owner_payloads_match_structural_array_layout_operands() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let u8_ty = append.primitive(nia_ty::PrimitiveTy::U8);
    let i32_ty = append.primitive(nia_ty::PrimitiveTy::I32);
    let usize_ty = append.primitive(nia_ty::PrimitiveTy::Usize);
    let nominal_def = GlobalDefId {
        module_id,
        def_id: DefId(2),
    };
    let signed_arg = nia_ty::ConstGenericArg {
        ty: usize_ty,
        value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::signed_bits(7)),
    };
    let unsigned_arg = nia_ty::ConstGenericArg {
        ty: usize_ty,
        value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(7)),
    };
    let left_operand = append.intern(nia_ty::TyKind::Nominal {
        def_id: nominal_def,
        args: vec![i32_ty],
        const_args: vec![signed_arg],
    });
    let right_operand = append.intern(nia_ty::TyKind::Nominal {
        def_id: nominal_def,
        args: vec![i32_ty],
        const_args: vec![unsigned_arg],
    });
    let left_array = append.intern(nia_ty::TyKind::Array {
        len: nia_ty::ArrayLenTy::Builtin {
            builtin: nia_ty::LayoutBuiltin::Size,
            ty: left_operand,
        },
        elem: u8_ty,
    });
    let right_array = append.intern(nia_ty::TyKind::Array {
        len: nia_ty::ArrayLenTy::Builtin {
            builtin: nia_ty::LayoutBuiltin::Size,
            ty: right_operand,
        },
        elem: u8_ty,
    });
    let trait_id = nia_ids::TraitId::Source(nominal_def);
    let left = BackendTraitObjectVtable {
        key: BackendTraitObjectVtableKey {
            self_ty: left_array,
            object_ty: left_array,
        },
        trait_id,
        trait_args: vec![left_array],
        trait_const_args: vec![],
        entries: vec![],
        span: nia_span::Span::default(),
    };
    let right = BackendTraitObjectVtable {
        key: BackendTraitObjectVtableKey {
            self_ty: right_array,
            object_ty: right_array,
        },
        trait_id,
        trait_args: vec![right_array],
        trait_const_args: vec![],
        entries: vec![],
        span: nia_span::Span::default(),
    };

    assert!(backend_vtable_payloads_match(&type_store, &left, &right));
}

#[test]
fn vtable_owner_deduplicates_semantically_equal_rebuilt_keys() {
    let mut module_ids = ModuleIdAllocator::new();
    let left_module = module_ids.allocate();
    let right_module = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let left_append = type_store.append_for_module(left_module);
    let right_append = type_store.append_for_module(right_module);
    let left_i32 = left_append.primitive(nia_ty::PrimitiveTy::I32);
    let right_i32 = right_append.primitive(nia_ty::PrimitiveTy::I32);
    let left_usize = left_append.primitive(nia_ty::PrimitiveTy::Usize);
    let right_usize = right_append.primitive(nia_ty::PrimitiveTy::Usize);
    let nominal_def = GlobalDefId {
        module_id: left_module,
        def_id: DefId(91),
    };
    let left_object = left_append.intern(nia_ty::TyKind::Nominal {
        def_id: nominal_def,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: left_usize,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::signed(7)),
        }],
    });
    let right_object = right_append.intern(nia_ty::TyKind::Nominal {
        def_id: nominal_def,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: right_usize,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(7)),
        }],
    });
    let trait_id = nia_ids::TraitId::Source(nominal_def);
    let vtable = |self_ty, object_ty| nia_backend_ir::BackendTraitObjectVtable {
        key: BackendTraitObjectVtableKey { self_ty, object_ty },
        trait_id,
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        entries: Vec::new(),
        span: nia_span::Span::default(),
    };
    let empty_layouts = || nia_backend_ir::BackendLayouts {
        target: TargetDataLayout::LP64,
        types: Vec::new(),
        structs: Vec::new(),
        unions: Vec::new(),
        enums: Vec::new(),
        struct_instances: Vec::new(),
        union_instances: Vec::new(),
    };
    let empty_module =
        |id: nia_ids::ModuleId,
         source_identity: &str,
         table: Vec<nia_backend_ir::BackendTraitObjectVtable>| {
            nia_backend_ir::BackendModule {
                id,
                source_identity: nia_source::SourceIdentity::new(source_identity),
                name: source_identity.to_string(),
                const_eval: nia_backend_ir::BackendConstFacts::default(),
                layouts: empty_layouts(),
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
                enums: Vec::new(),
                globals: Vec::new(),
                global_instances: Vec::new(),
                functions: Vec::new(),
                function_instances: Vec::new(),
                closure_entries: Vec::new(),
                trait_object_vtables: table,
                generic_instantiations: Vec::new(),
            }
        };
    let mut modules = vec![
        empty_module(right_module, "b", vec![vtable(right_i32, right_object)]),
        empty_module(left_module, "a", vec![vtable(left_i32, left_object)]),
    ];

    assign_unique_vtable_owners(&mut modules, &type_store);

    assert!(modules[0].trait_object_vtables.is_empty());
    assert_eq!(modules[1].trait_object_vtables.len(), 1);
}

#[test]
fn aggregate_owner_deduplicates_semantically_equal_instance_keys() {
    let mut module_ids = ModuleIdAllocator::new();
    let left_module = module_ids.allocate();
    let right_module = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let left_append = type_store.append_for_module(left_module);
    let right_append = type_store.append_for_module(right_module);
    let left_i32 = left_append.primitive(nia_ty::PrimitiveTy::I32);
    let right_i32 = right_append.primitive(nia_ty::PrimitiveTy::I32);
    let left_usize = left_append.primitive(nia_ty::PrimitiveTy::Usize);
    let right_usize = right_append.primitive(nia_ty::PrimitiveTy::Usize);
    let def_id = GlobalDefId {
        module_id: left_module,
        def_id: DefId(92),
    };
    let left_arg = left_append.intern(nia_ty::TyKind::Nominal {
        def_id,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: left_usize,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::signed(5)),
        }],
    });
    let right_arg = right_append.intern(nia_ty::TyKind::Nominal {
        def_id,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: right_usize,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(5)),
        }],
    });
    let field_id = GlobalDefId {
        module_id: left_module,
        def_id: DefId(93),
    };
    let field = |ty| nia_backend_ir::BackendField {
        def_id: field_id,
        name: SymbolId::EMPTY,
        ty,
        span: nia_span::Span::default(),
    };
    let instance = |args, field_ty, symbol: &str| nia_backend_ir::BackendStructInstance {
        def_id,
        name: SymbolId::EMPTY,
        args: vec![args],
        const_args: Vec::new(),
        symbol: symbol.to_string(),
        fields: vec![field(field_ty)],
        is_extern: false,
        span: nia_span::Span::default(),
    };
    let empty_layouts = || nia_backend_ir::BackendLayouts {
        target: TargetDataLayout::LP64,
        types: Vec::new(),
        structs: Vec::new(),
        unions: Vec::new(),
        enums: Vec::new(),
        struct_instances: Vec::new(),
        union_instances: Vec::new(),
    };
    let module = |id: nia_ids::ModuleId,
                  source_identity: &str,
                  instances: Vec<nia_backend_ir::BackendStructInstance>| {
        nia_backend_ir::BackendModule {
            id,
            source_identity: nia_source::SourceIdentity::new(source_identity),
            name: source_identity.to_string(),
            const_eval: nia_backend_ir::BackendConstFacts::default(),
            layouts: empty_layouts(),
            structs: Vec::new(),
            unions: Vec::new(),
            struct_instances: instances,
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }
    };
    let mut modules = vec![
        module(
            right_module,
            "b",
            vec![instance(right_arg, right_i32, "instance")],
        ),
        module(
            left_module,
            "a",
            vec![instance(left_arg, left_i32, "instance")],
        ),
    ];

    assign_unique_aggregate_instance_owners(&mut modules, &type_store);

    assert!(modules[0].struct_instances.is_empty());
    assert_eq!(modules[1].struct_instances.len(), 1);
}

mod cfg_and_scalar_passes;
mod diagnostics;
mod finalization_contracts;
mod inlining_and_cross_function;
mod local_optimizations;
mod lowering;
mod reachability_and_instances;
mod static_initializers;
