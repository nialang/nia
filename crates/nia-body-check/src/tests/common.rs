// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use crate::*;
pub(super) use nia_defs::{
    DefKind, ModuleId, VisibleExtensionMethod, VisibleExtensionMethods, collect_module_defs,
};
pub(super) use nia_item_signatures::{
    ProgramFunctionSignature, ProgramTraitImplSignature, collect_item_signatures,
};
pub(super) use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
pub(super) use nia_local_resolve::resolve_module_locals;
pub(super) use nia_node_id::{NodeOriginTable, NodePosition, SyntaxKind};
pub(super) use nia_parser::parse_module_with_symbols;
pub(super) use nia_program_signatures::{ProgramSignatureContext, ProgramSignatureLookup};
pub(super) use nia_sema_ir::{BracketSuffixResolution, BuiltinValue, SemanticUseTable};
pub(super) use nia_source::{SourceId, SourcePath, SourceRevision, SourceVersion};
pub(super) use nia_symbol::{SymbolId, stable_hash};
pub(super) use nia_symbol_table::SymbolTable;
pub(super) use nia_ty::{TraitId, TyKind};
pub(super) use nia_type_lower::lower_module_types;
pub(super) use nia_type_resolve::resolve_module_types_with_symbols;
pub(super) use std::collections::HashMap;

pub(super) fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

pub(super) struct EmptyBodyProgramSignatures {
    pub functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub globals: HashMap<GlobalDefId, nia_item_signatures::ProgramGlobalSignature>,
    pub comptimes: HashMap<GlobalDefId, nia_item_signatures::ProgramComptimeSignature>,
    pub structs: HashMap<GlobalDefId, nia_item_signatures::ProgramStructSignature>,
    pub unions: HashMap<GlobalDefId, nia_item_signatures::ProgramUnionSignature>,
    pub enums: HashMap<GlobalDefId, nia_item_signatures::ProgramEnumSignature>,
    pub traits: HashMap<GlobalDefId, nia_item_signatures::ProgramTraitSignature>,
    pub type_aliases: HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>,
    pub trait_impls: Vec<ProgramTraitImplSignature>,
}

impl EmptyBodyProgramSignatures {
    pub fn new() -> Self {
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

    pub fn context(&self) -> ProgramSignatureContext<'_> {
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

    fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
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

pub(super) fn pipeline(source: &str) -> BodyCheck {
    pipeline_with_values(source, |_, _, _| {})
}

pub(super) fn pipeline_without_visible_extensions(source: &str) -> BodyCheck {
    pipeline_with_options(source, |_, _, _| {}, false)
}

pub(super) fn pipeline_with_values(
    source: &str,
    adjust_values: impl FnOnce(
        &nia_ast::Module,
        &nia_defs::DefCollection,
        &mut nia_value_resolve::ValueResolution,
    ),
) -> BodyCheck {
    pipeline_with_options(source, adjust_values, true)
}

fn pipeline_with_options(
    source: &str,
    adjust_values: impl FnOnce(
        &nia_ast::Module,
        &nia_defs::DefCollection,
        &mut nia_value_resolve::ValueResolution,
    ),
    include_visible_extensions: bool,
) -> BodyCheck {
    let symbols = SymbolTable::new();
    let (module, parse_errors) = parse_module_with_symbols(source, symbols.clone());
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let type_resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    assert!(
        type_resolved.diagnostics.is_empty(),
        "{:?}",
        type_resolved.diagnostics
    );
    let lowered = lower_module_types(&module, &type_resolved);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let mut values = nia_value_resolve::resolve_module_values(&module, &defs);
    adjust_values(&module, &defs, &mut values);
    assert!(values.diagnostics.is_empty(), "{:?}", values.diagnostics);
    let locals = resolve_module_locals(&module, &defs, &values);
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    let active_item_tree = active_item_tree(&module);
    let semantic_uses =
        semantic_use_table(ModuleId(0), &values, &locals, &lowered, &active_item_tree);
    let signatures = collect_item_signatures(&module, &defs, &lowered);
    assert!(
        signatures.diagnostics.is_empty(),
        "{:?}",
        signatures.diagnostics
    );
    let target = nia_target_config::TargetConfig::host();
    let source_path = SourcePath::new("/tmp/nia-body-check-test/main.nia");
    let comptime_module =
        nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            signatures: &signatures,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            symbols: &symbols,
            const_exprs: &lowered.const_exprs,
            source_path: &source_path,
        });
    assert!(
        comptime_module.diagnostics.is_empty(),
        "{:?}",
        comptime_module.diagnostics
    );
    let comptime_input = nia_comptime_check::ComptimeInput {
        module: &comptime_module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        lowered: &lowered,
        signatures: &signatures,
        interner: &lowered.interner,
        normalized: &std::collections::HashMap::new(),
        target: &target,
        source_path: &source_path,
        program: nia_comptime_check::ComptimeProgramContext::empty(),
    };
    let comptime_array_lengths =
        nia_comptime_check::compute_module_comptime_array_lengths(comptime_input);
    let comptime_enum_values = nia_comptime_check::compute_module_comptime_enum_values(
        comptime_input,
        comptime_array_lengths.clone(),
    );
    let comptime_values = nia_comptime_check::compute_module_comptime_values(
        comptime_input,
        comptime_array_lengths.clone(),
        comptime_enum_values.clone(),
    );
    let comptime_typed_facts = nia_comptime_check::compute_module_comptime_typed_facts(
        comptime_input,
        comptime_array_lengths.clone(),
        comptime_enum_values,
        comptime_values.clone(),
    );
    let comptime = crate::BodyComptime::from_phases(
        &comptime_values,
        &comptime_array_lengths,
        &comptime_typed_facts,
    );
    assert!(
        comptime_values.diagnostics.is_empty(),
        "{:?}",
        comptime_values.diagnostics
    );
    let normalization =
        nia_type_normalize::normalize_module_types(ModuleId(0), &lowered.interner, &signatures);
    assert!(
        normalization.diagnostics.is_empty(),
        "{:?}",
        normalization.diagnostics
    );
    let mut extensions = VisibleExtensionMethods::default();
    if include_visible_extensions {
        for item in &module.items {
            let nia_ast::ItemKind::Extend(extend) = &item.kind else {
                continue;
            };
            let Some(target_ty) = lowered.ty_for_key(&extend.target.node_key) else {
                continue;
            };
            let target_ty = normalization.normalize(target_ty);
            let extend_generics = nia_ast::generic_param_names(&extend.generics);
            let Some(impl_signature) = signatures.trait_impls.iter().find(|signature| {
                signature.generics == extend_generics
                    && normalization.normalize(signature.target_ty) == target_ty
            }) else {
                continue;
            };
            for method in &extend.methods {
                let Some(method_id) = defs.def_nodes.get(&method.function.node_key) else {
                    continue;
                };
                let Some(method_def) = defs.defs.get(method_id) else {
                    continue;
                };
                if method_def.kind != DefKind::Method {
                    continue;
                }
                let mut effective_generics = extend_generics.clone();
                effective_generics.extend(method_def.generics.iter().cloned());
                extensions.insert(
                    impl_signature.impl_id,
                    target_ty,
                    VisibleExtensionMethod {
                        name: method_def.name.clone(),
                        def_id: GlobalDefId {
                            module_id: ModuleId(0),
                            def_id: method_id,
                        },
                        impl_id: impl_signature.impl_id,
                        effective_generics,
                        trait_id: None,
                        trait_args: Vec::new(),
                        where_predicates: Vec::new(),
                        is_callable: true,
                        is_trait_witness: false,
                    },
                );
            }
        }
    }
    let layouts = nia_layout::compute_layouts(
        &defs,
        &lowered.interner,
        &signatures,
        nia_layout::TargetDataLayout::LP64,
    );
    let mut program_signatures = EmptyBodyProgramSignatures::new();
    program_signatures.trait_impls = single_module_trait_impls(ModuleId(0), &signatures, &lowered);
    let origins = NodeOriginTable::default();
    check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: None,
        source_path: &source_path,
        symbols: &symbols,
        origins: &origins,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        lowered: &lowered,
        signatures: BodyLocalSignatures::from_item_signatures(&signatures),
        comptime_signatures: &signatures,
        normalization: &normalization,
        seed_interner: None,
        target: &target,
        comptime,
        comptime_module: &comptime_module.module,
        layouts: &layouts,
        extensions: &extensions,
        lazy_extensions: None,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        extension_interner: None,
        program: BodyProgramContext::empty(),
        program_signatures: program_signatures.context(),
        function_scope: FunctionCheckScope::LocalModule,
        program_comptime: ProgramComptimeMaps::empty(),
        filter: crate::BodyCheckFilter::All,
        product: crate::BodyCheckProduct::Full,
        prechecked: None,
    })
}

pub(super) fn active_item_tree(module: &nia_ast::Module) -> ActiveModuleItemTree {
    let item_tree = ModuleItemTree::from_module(module);
    ActiveModuleItemTree::new(
        item_tree.active_items_without_comptime(),
        Default::default(),
    )
}

fn single_module_trait_impls(
    module_id: ModuleId,
    signatures: &nia_item_signatures::ItemSignatures,
    lowered: &nia_type_lower::TypeLowering,
) -> Vec<ProgramTraitImplSignature> {
    signatures
        .trait_impls
        .iter()
        .filter_map(|impl_signature| {
            let trait_ty = impl_signature.trait_ty?;
            let (trait_id, trait_args, trait_const_args) =
                trait_id_and_args(&lowered.interner, trait_ty)?;
            Some(ProgramTraitImplSignature {
                module_id,
                impl_id: impl_signature.impl_id,
                builtin: impl_signature.builtin.clone(),
                generics: impl_signature.generics.clone(),
                target_ty: impl_signature.target_ty,
                trait_id,
                trait_args,
                trait_const_args,
                where_predicates: impl_signature.where_predicates.clone(),
                associated_types: impl_signature.associated_types.clone(),
                associated_values: impl_signature.associated_values.clone(),
                interner: lowered.interner.clone(),
            })
        })
        .collect()
}

fn trait_id_and_args(
    interner: &nia_ty::TyInterner,
    ty: nia_ids::InternedTyId,
) -> Option<(
    TraitId,
    Vec<nia_ids::InternedTyId>,
    Vec<nia_ty::ConstGenericArg>,
)> {
    match interner.get(ty) {
        Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) => Some((TraitId::Source(*def_id), args.clone(), const_args.clone())),
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            Some((TraitId::Builtin(*trait_id), args.clone(), Vec::new()))
        }
        _ => None,
    }
}

pub(super) fn semantic_use_table(
    module_id: ModuleId,
    values: &nia_value_resolve::ValueResolution,
    locals: &nia_local_resolve::LocalResolution,
    type_lowering: &nia_type_lower::TypeLowering,
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
                    nia_ids::GlobalDefId {
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
