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

mod cfg_and_scalar_passes;
mod diagnostics;
mod finalization_contracts;
mod inlining_and_cross_function;
mod local_optimizations;
mod lowering;
mod reachability_and_instances;
mod static_initializers;
