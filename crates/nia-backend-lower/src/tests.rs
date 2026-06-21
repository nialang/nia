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
use nia_function_lower::lower_function_body;
use nia_ids::{GlobalDefId, LocalId};
use nia_item_signatures::{ProgramSignatureMaps, collect_item_signatures};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_local_resolve::resolve_module_locals;
use nia_node_id::NodeOriginTable;
use nia_parser::parse_module;
use nia_sema_ir::SemanticUseTable;
use nia_source::SourcePath;
use nia_static_ir::StaticInit;
use nia_type_lower::{TypeLowering, lower_module_types_with_id};
use nia_type_normalize::normalize_module_types;
use nia_type_resolve::resolve_module_types;
use nia_value_resolve::resolve_module_values;
use std::collections::HashMap;

fn semantic_use_table(
    module_id: ModuleId,
    values: &nia_value_resolve::ValueResolution,
    locals: &nia_local_resolve::LocalResolution,
    type_lowering: &TypeLowering,
) -> SemanticUseTable {
    let mut builder = SemanticUseTable::builder();
    for (key, local_use) in &locals.node_uses {
        if let nia_local_resolve::LocalUse::Local(local_id) = local_use {
            builder.insert_node_local_value_use(key.clone(), *local_id);
        }
    }
    builder.extend_node_global_value_uses(
        values
            .node_qualified_values
            .iter()
            .map(|(key, global_id)| (key.clone(), *global_id)),
    );
    for (key, resolution) in &values.node_names {
        match resolution {
            nia_value_resolve::ValueNameResolution::Def(def_id) => {
                builder.insert_node_global_value_use(
                    key.clone(),
                    GlobalDefId {
                        module_id,
                        def_id: *def_id,
                    },
                );
            }
            nia_value_resolve::ValueNameResolution::External(global_id) => {
                builder.insert_node_global_value_use(key.clone(), *global_id);
            }
            nia_value_resolve::ValueNameResolution::Module
            | nia_value_resolve::ValueNameResolution::LocalDeferred
            | nia_value_resolve::ValueNameResolution::Error => {}
        }
    }
    builder.extend_node_local_defs(
        locals
            .node_local_defs
            .iter()
            .map(|(key, local_id)| (key.clone(), *local_id)),
    );
    builder.extend_node_type_uses(
        type_lowering
            .node_type_uses
            .iter()
            .map(|(key, ty)| (key.clone(), *ty)),
    );
    builder.finish()
}

mod cfg_and_scalar_passes;
mod diagnostics;
mod inlining_and_cross_function;
mod local_optimizations;
mod lowering;
mod reachability_and_instances;
mod static_initializers;

fn lower_source(source: &str) -> BackendLowering {
    let lowering = lower_source_with_comptime_mutation(source, |_, _| {});
    assert!(
        lowering.diagnostics.is_empty(),
        "{:?}",
        lowering.diagnostics
    );
    lowering
}

fn lower_source_with_comptime_mutation(
    source: &str,
    mutate_comptime: impl FnOnce(&mut nia_comptime_check::ComptimeCheck, &TypeLowering),
) -> BackendLowering {
    lower_source_with_body_mutation_comptime_mutation_and_optimization(
        source,
        |_| {},
        mutate_comptime,
        nia_opt::OptimizationPolicy::default(),
    )
}

fn lower_source_with_body_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    optimization: nia_opt::OptimizationPolicy,
) -> BackendLowering {
    lower_source_with_body_mutation_extensions_comptime_mutation_and_optimization(
        source,
        mutate_body,
        |_, _, _, _| {},
        |_, _| {},
        optimization,
    )
}

fn lower_source_with_body_mutation_comptime_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_comptime: impl FnOnce(&mut nia_comptime_check::ComptimeCheck, &TypeLowering),
    optimization: nia_opt::OptimizationPolicy,
) -> BackendLowering {
    lower_source_with_body_mutation_extensions_comptime_mutation_and_optimization(
        source,
        mutate_body,
        |_, _, _, _| {},
        mutate_comptime,
        optimization,
    )
}

fn lower_source_with_body_mutation_extensions_comptime_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_extensions: impl FnOnce(
        &mut VisibleExtensionMethods,
        &nia_defs::DefCollection,
        &TypeLowering,
        &ItemSignatures,
    ),
    mutate_comptime: impl FnOnce(&mut nia_comptime_check::ComptimeCheck, &TypeLowering),
    optimization: nia_opt::OptimizationPolicy,
) -> BackendLowering {
    lower_source_with_body_check_mutation_and_optimization(
        source,
        mutate_body,
        mutate_extensions,
        mutate_comptime,
        |_, _, _, _| {},
        optimization,
    )
}

fn lower_source_with_body_check_mutation_and_optimization(
    source: &str,
    mut mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_extensions: impl FnOnce(
        &mut VisibleExtensionMethods,
        &nia_defs::DefCollection,
        &TypeLowering,
        &ItemSignatures,
    ),
    mutate_comptime: impl FnOnce(&mut nia_comptime_check::ComptimeCheck, &TypeLowering),
    mutate_body_check: impl FnOnce(
        &mut nia_body_check::BodyCheck,
        &nia_ast::Module,
        &nia_defs::DefCollection,
        &ItemSignatures,
    ),
    optimization: nia_opt::OptimizationPolicy,
) -> BackendLowering {
    let (module, errors) = parse_module(source);
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_resolved = resolve_module_types(&module, &defs);
    let type_lowering = lower_module_types_with_id(ModuleId(0), &module, &type_resolved);
    let signatures = collect_item_signatures(&module, &defs, &type_lowering);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let semantic_uses = semantic_use_table(ModuleId(0), &values, &locals, &type_lowering);
    let normalization = normalize_module_types(ModuleId(0), &type_lowering.interner, &signatures);
    let target = nia_target_config::TargetConfig::host();
    let source_path = SourcePath::new("/tmp/nia-backend-lower-test/main.nia");
    let active_item_tree = active_item_tree(&module);
    let comptime_module =
        nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            const_exprs: &type_lowering.const_exprs,
            source_path: &source_path,
        });
    assert!(
        comptime_module.diagnostics.is_empty(),
        "{:?}",
        comptime_module.diagnostics
    );
    let comptime = nia_comptime_check::check_module_comptime(nia_comptime_check::ComptimeInput {
        module: &comptime_module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        signatures: &signatures,
        interner: &normalization.interner,
        normalized: &normalization.normalized,
        target: &target,
        source_path: &source_path,
        program: nia_comptime_check::ComptimeProgramContext::empty(),
    });
    let layouts = nia_layout::compute_layouts_with_normalized_types(
        &defs,
        &normalization.interner,
        &signatures,
        &normalization.normalized,
        &|id| comptime.array_lengths.get(&id).copied(),
        nia_layout::TargetDataLayout::LP64,
    );
    let mut extensions = VisibleExtensionMethods::default();
    mutate_extensions(&mut extensions, &defs, &type_lowering, &signatures);
    let origins = NodeOriginTable::default();
    let mut body_check = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: None,
        source_path: &source_path,
        origins: &origins,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        lowered: &type_lowering,
        signatures: &signatures,
        normalization: &normalization,
        target: &target,
        comptime: &comptime,
        comptime_module: &comptime_module.module,
        layouts: &layouts,
        extensions: &extensions,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        extension_interner: None,
        program: nia_body_check::BodyProgramContext::empty(),
        program_signatures: ProgramSignatureMaps {
            functions: &HashMap::new(),
            globals: &HashMap::new(),
            comptimes: &HashMap::new(),
            structs: &HashMap::new(),
            unions: &HashMap::new(),
            enums: &HashMap::new(),
            traits: &HashMap::new(),
            type_aliases: &HashMap::new(),
            trait_impls: &[],
        },
        program_comptime: nia_body_check::ProgramComptimeMaps {
            comptimes: &HashMap::new(),
            modules: &HashMap::new(),
        },
        filter: nia_body_check::BodyCheckFilter::All,
    });
    assert!(
        body_check.diagnostics.is_empty(),
        "{:?}",
        body_check.diagnostics
    );
    mutate_body_check(&mut body_check, &module, &defs, &signatures);
    let function_bodies = body_check
        .ir
        .function_bodies
        .iter()
        .map(|(def_id, body)| {
            let mut body = lower_function_body(body).expect("valid typed body");
            mutate_body(&mut body);
            (*def_id, body)
        })
        .collect::<HashMap<_, _>>();
    let monomorphization =
        nia_monomorphize::collect_monomorphizations(&[nia_monomorphize::MonomorphizeModuleInput {
            module_id: ModuleId(0),
            defs: &defs,
            interner: &body_check.ir.interner,
            normalization: &normalization,
            comptime: &comptime,
            const_expr_summaries: &type_lowering.const_expr_summaries,
            layouts: Some(&layouts),
            local_enums: &signatures.enums,
            program_enums: &HashMap::new(),
            trait_impls: &[],
            instantiations: &body_check.facts.generic_instantiations,
        }]);
    assert!(
        monomorphization.diagnostics.is_empty(),
        "{:?}",
        monomorphization.diagnostics
    );
    let mut comptime = comptime;
    mutate_comptime(&mut comptime, &type_lowering);
    let program_comptime = HashMap::from([(ModuleId(0), &comptime)]);

    let input = BackendLowerModuleInput {
        module_id: ModuleId(0),
        module_name: "main".to_string(),
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        type_lowering: &type_lowering,
        signatures: &signatures,
        type_normalization: &normalization,
        body_ir: &body_check.ir,
        function_interner: &body_check.ir.interner,
        semantic_facts: &body_check.facts,
        extensions: &extensions,
        comptime: &comptime,
        program_comptime: &program_comptime,
        layouts: &layouts,
        function_bodies: &function_bodies,
        roots: BackendFunctionRoots::Public,
        program_function_bodies: &function_bodies,
        extension_interner: None,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        program_extensions: &HashMap::new(),
        program_defs: &HashMap::new(),
        program_type_interners: &HashMap::new(),
        program_functions: &HashMap::new(),
        program_structs: &HashMap::new(),
        program_unions: &HashMap::new(),
        program_enums: &HashMap::new(),
        program_traits: &HashMap::new(),
        trait_impls: &[],
    };
    lower_backend_program(&[input], &monomorphization, optimization)
}

fn active_item_tree(module: &nia_ast::Module) -> ActiveModuleItemTree {
    let item_tree = ModuleItemTree::from_module(module);
    ActiveModuleItemTree::new(
        item_tree.active_items_without_comptime(),
        Default::default(),
    )
}

fn global_def_id_by_name(defs: &nia_defs::DefCollection, name: &str) -> GlobalDefId {
    defs.defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == name).then_some(GlobalDefId {
                module_id: defs.module_id,
                def_id,
            })
        })
        .unwrap_or_else(|| panic!("missing def `{name}`"))
}

fn nominal_type_by_def(interner: &nia_ty::TyInterner, target: GlobalDefId) -> InternedTyId {
    nominal_type_by_def_with_args(interner, target, &[])
}

fn nominal_type_by_def_with_args(
    interner: &nia_ty::TyInterner,
    target: GlobalDefId,
    target_args: &[InternedTyId],
) -> InternedTyId {
    interner
        .iter()
        .find_map(|(ty, kind)| {
            matches!(
                kind,
                nia_ty::TyKind::Nominal {
                    def_id,
                    args
                } if *def_id == target && args == target_args
            )
            .then_some(ty)
        })
        .unwrap_or_else(|| panic!("missing nominal type {target:?} with args {target_args:?}"))
}

fn first_terminal_value(body: &nia_function_ir::FunctionBody) -> &nia_function_ir::FunctionExpr {
    body.blocks
        .iter()
        .find_map(|block| match &block.terminator {
            FunctionTerminator::Return {
                value: Some(value), ..
            }
            | FunctionTerminator::Tail {
                value: Some(value), ..
            } => Some(value),
            _ => None,
        })
        .expect("terminal value")
}

fn first_terminal_value_mut(
    body: &mut nia_function_ir::FunctionBody,
) -> &mut nia_function_ir::FunctionExpr {
    body.blocks
        .iter_mut()
        .find_map(|block| match &mut block.terminator {
            FunctionTerminator::Return {
                value: Some(value), ..
            }
            | FunctionTerminator::Tail {
                value: Some(value), ..
            } => Some(value),
            _ => None,
        })
        .expect("terminal value")
}
