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
use nia_item_signatures::{ProgramFunctionSignature, collect_item_signatures};
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
use nia_type_lower::{TypeLowering, lower_module_types_with_id};
use nia_type_normalize::normalize_module_types;
use nia_type_resolve::resolve_module_types_with_symbols;
use nia_value_resolve::resolve_module_values;
use std::collections::HashMap;

fn local_name(text: &str) -> nia_function_ir::LocalName {
    nia_function_ir::LocalName::named(sym(text))
}

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(nia_symbol::stable_hash(text))
}

struct EmptyBodyProgramSignatures {
    functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    globals: HashMap<GlobalDefId, nia_item_signatures::ProgramGlobalSignature>,
    comptimes: HashMap<GlobalDefId, nia_item_signatures::ProgramComptimeSignature>,
    structs: HashMap<GlobalDefId, nia_item_signatures::ProgramStructSignature>,
    unions: HashMap<GlobalDefId, nia_item_signatures::ProgramUnionSignature>,
    enums: HashMap<GlobalDefId, nia_item_signatures::ProgramEnumSignature>,
    traits: HashMap<GlobalDefId, nia_item_signatures::ProgramTraitSignature>,
    type_aliases: HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>,
    trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
}

impl EmptyBodyProgramSignatures {
    fn new() -> Self {
        Self {
            functions: HashMap::new(),
            globals: HashMap::new(),
            comptimes: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            enums: HashMap::new(),
            traits: HashMap::new(),
            type_aliases: HashMap::new(),
            trait_impls: Vec::new(),
        }
    }

    fn context(&self) -> ProgramSignatureContext<'_> {
        ProgramSignatureContext {
            lookup: self,
            trait_impls: &self.trait_impls,
            trait_impl_index: None,
        }
    }
}

impl ProgramSignatureLookup for EmptyBodyProgramSignatures {
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        self.functions.get(&def_id).cloned()
    }

    fn global(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramGlobalSignature> {
        self.globals.get(&def_id).cloned()
    }

    fn comptime(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_item_signatures::ProgramComptimeSignature> {
        self.comptimes.get(&def_id).cloned()
    }

    fn struct_(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramStructSignature> {
        self.structs.get(&def_id).cloned()
    }

    fn union(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramUnionSignature> {
        self.unions.get(&def_id).cloned()
    }

    fn enum_(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramEnumSignature> {
        self.enums.get(&def_id).cloned()
    }

    fn trait_(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramTraitSignature> {
        self.traits.get(&def_id).cloned()
    }

    fn type_alias(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_item_signatures::ProgramTypeAliasSignature> {
        self.type_aliases.get(&def_id).cloned()
    }

    fn trait_ids_with_method_named(&self, name: &nia_symbol::SymbolId) -> Vec<GlobalDefId> {
        self.traits
            .iter()
            .filter_map(|(trait_id, signature)| {
                signature
                    .signature
                    .methods
                    .iter()
                    .any(|method| &method.name == name)
                    .then_some(*trait_id)
            })
            .collect()
    }

    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, nia_item_signatures::ProgramTraitSignature)> {
        self.traits.iter().find_map(|(trait_id, signature)| {
            signature
                .signature
                .methods
                .iter()
                .any(|method| {
                    GlobalDefId {
                        module_id: trait_id.module_id,
                        def_id: method.def_id,
                    } == method_id
                })
                .then(|| (*trait_id, signature.clone()))
        })
    }
}

fn semantic_use_table(
    module_id: ModuleId,
    values: &nia_value_resolve::ValueResolution,
    locals: &nia_local_resolve::LocalResolution,
    type_lowering: &TypeLowering,
    active_item_tree: &ActiveModuleItemTree,
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
        type_lowering.versioned_type_uses_from_active_item_tree(active_item_tree),
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
    let symbols = SymbolTable::new();
    let (module, errors) = parse_module_with_symbols(source, symbols.clone());
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let mut type_lowering = lower_module_types_with_id(ModuleId(0), &module, &type_resolved);
    let signatures = collect_item_signatures(&module, &defs, &type_lowering);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let active_item_tree = active_item_tree(&module);
    let semantic_uses = semantic_use_table(
        ModuleId(0),
        &values,
        &locals,
        &type_lowering,
        &active_item_tree,
    );
    let normalization_input = type_lowering
        .interner
        .iter()
        .map(|(ty_id, _)| ty_id)
        .collect::<Vec<_>>();
    let normalization = normalize_module_types(
        ModuleId(0),
        &mut type_lowering.interner,
        &normalization_input,
        &signatures,
    );
    let target = nia_target_config::TargetConfig::host();
    let source_path = SourcePath::new("/tmp/nia-backend-lower-test/main.nia");
    let comptime_module =
        nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            signatures: &signatures,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            symbols: &symbols,
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
        symbols: &symbols,
        lowered: &type_lowering,
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
    let comptime_array_lengths = nia_comptime_check::ComptimeArrayLengths {
        interner: comptime.interner.clone(),
        values: comptime.array_lengths.clone(),
        diagnostics: Vec::new(),
    };
    let comptime_values = nia_comptime_check::ComptimeValues {
        interner: comptime.interner.clone(),
        values: comptime.values.clone(),
        typed_values: comptime.typed_values.clone(),
        diagnostics: Vec::new(),
    };
    let comptime_typed_facts = nia_comptime_check::ComptimeTypedFacts {
        interner: comptime.interner.clone(),
        typed_values: comptime.typed_values.clone(),
        diagnostics: Vec::new(),
    };
    let body_comptime = nia_body_check::BodyComptime::from_phases(
        &comptime_values,
        &comptime_array_lengths,
        &comptime_typed_facts,
    );
    let mut extensions = VisibleExtensionMethods::default();
    mutate_extensions(&mut extensions, &defs, &type_lowering, &signatures);
    let origins = NodeOriginTable::default();
    let program_signatures = EmptyBodyProgramSignatures::new();
    let mut body_check = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: None,
        source_path: &source_path,
        symbols: &symbols,
        origins: &origins,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        lowered: &type_lowering,
        signatures: nia_body_check::BodyLocalSignatures::from_item_signatures(&signatures),
        comptime_signatures: &signatures,
        normalization: &normalization,
        seed: None,
        target: &target,
        comptime: body_comptime,
        comptime_module: &comptime_module.module,
        layouts: &layouts,
        extensions: &extensions,
        lazy_extensions: None,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        extension_interner: None,
        program: nia_body_check::BodyProgramContext::empty(),
        program_signatures: program_signatures.context(),
        function_scope: nia_body_check::FunctionCheckScope::LocalModule,
        program_comptime: nia_body_check::ProgramComptimeMaps::empty(),
        filter: nia_body_check::BodyCheckFilter::All,
        product: nia_body_check::BodyCheckProduct::Full,
        prechecked: None,
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
    let trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::default();
    let semantic_instantiations = body_check
        .facts
        .iter_generic_instantiations()
        .cloned()
        .collect::<Vec<_>>();
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
            trait_impl_index: &trait_impl_index,
            instantiations: &semantic_instantiations,
        }]);
    assert!(
        monomorphization.diagnostics.is_empty(),
        "{:?}",
        monomorphization.diagnostics
    );
    let mut comptime = comptime;
    mutate_comptime(&mut comptime, &type_lowering);
    let comptime_array_lengths = nia_comptime_check::ComptimeArrayLengths {
        interner: comptime.interner.clone(),
        values: comptime.array_lengths.clone(),
        diagnostics: Vec::new(),
    };
    let program_comptime = HashMap::from([(ModuleId(0), &comptime_array_lengths)]);
    let comptime_enum_values = comptime_enum_values_from_check(&comptime);
    let program_function_body_interners = ProgramFunctionBodyInterners::default();
    let no_program_defs = |_| None;

    let input = BackendLowerModuleInput {
        module_id: ModuleId(0),
        module_name: "main".to_string(),
        symbols: &symbols,
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
        comptime_array_lengths: &comptime_array_lengths,
        comptime_enum_values: &comptime_enum_values,
        program_comptime: &program_comptime,
        layouts: &layouts,
        function_bodies: &function_bodies,
        roots: BackendFunctionRoots::Public,
        reachable_globals: None,
        reachable_structs: None,
        reachable_unions: None,
        program_function_bodies: &function_bodies,
        extension_interner: None,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        program_extensions: &HashMap::new(),
        program_defs: &no_program_defs,
        program_function_body_interners: &program_function_body_interners,
        program_type_normalizations: &HashMap::new(),
        program_functions: &HashMap::new(),
        program_structs: &HashMap::new(),
        program_unions: &HashMap::new(),
        program_enums: &HashMap::new(),
        program_traits: &HashMap::new(),
        program_type_aliases: &HashMap::new(),
        trait_impls: &[],
        trait_impl_index: &trait_impl_index,
    };
    lower_backend_program(&[input], &monomorphization, optimization)
}

fn comptime_enum_values_from_check(
    comptime: &nia_comptime_check::ComptimeCheck,
) -> nia_comptime_check::ComptimeEnumValues {
    nia_comptime_check::ComptimeEnumValues {
        interner: comptime.interner.clone(),
        values: comptime.enum_values.clone(),
        typed_values: comptime.typed_enum_values.clone(),
        diagnostics: Vec::new(),
    }
}

fn active_item_tree(module: &nia_ast::Module) -> ActiveModuleItemTree {
    let item_tree = ModuleItemTree::from_module(module);
    ActiveModuleItemTree::new(
        item_tree.active_items_without_comptime(),
        Default::default(),
    )
}

fn global_def_id_by_name(defs: &nia_defs::DefCollection, name: &str) -> GlobalDefId {
    let name_symbol = sym(name);
    defs.defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == name_symbol).then_some(GlobalDefId {
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
                    args,
                    ..
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
