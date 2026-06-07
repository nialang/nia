// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use crate::*;
pub(super) use nia_defs::{
    DefKind, ModuleId, VisibleExtensionMethod, VisibleExtensionMethods, collect_module_defs,
};
pub(super) use nia_item_signatures::{
    ProgramSignatureMaps, ProgramTraitImplSignature, collect_item_signatures,
};
pub(super) use nia_local_resolve::resolve_module_locals;
pub(super) use nia_node_id::{NodeOriginTable, NodePosition, SyntaxKind};
pub(super) use nia_parser::{parse_module, parse_module_syntax_with_origins};
pub(super) use nia_sema_ir::{BracketSuffixResolution, BuiltinValue, SemanticUseTable};
pub(super) use nia_source::{SourceId, SourceRevision, SourceVersion};
pub(super) use nia_ty::{TraitId, TyKind};
pub(super) use nia_type_lower::lower_module_types;
pub(super) use nia_type_resolve::resolve_module_types;
pub(super) use std::collections::HashMap;

pub(super) fn pipeline(source: &str) -> BodyCheck {
    let (module, parse_errors) = parse_module(source);
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let type_resolved = resolve_module_types(&module, &defs);
    assert!(
        type_resolved.diagnostics.is_empty(),
        "{:?}",
        type_resolved.diagnostics
    );
    let lowered = lower_module_types(&module, &type_resolved);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let values = nia_value_resolve::resolve_module_values(&module, &defs);
    assert!(values.diagnostics.is_empty(), "{:?}", values.diagnostics);
    let locals = resolve_module_locals(&module, &defs, &values);
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    let semantic_uses = semantic_use_table(ModuleId(0), &values, &locals, &lowered);
    let signatures = collect_item_signatures(&module, &defs, &lowered);
    assert!(
        signatures.diagnostics.is_empty(),
        "{:?}",
        signatures.diagnostics
    );
    let target = nia_target_config::TargetConfig::host();
    let comptime_module =
        nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
            module: &module,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            const_exprs: &lowered.const_exprs,
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
        interner: &lowered.interner,
        normalized: &std::collections::HashMap::new(),
        target: &target,
        program: nia_comptime_check::ComptimeProgramContext::empty(),
    });
    assert!(
        comptime.diagnostics.is_empty(),
        "{:?}",
        comptime.diagnostics
    );
    let normalization = TypeNormalization {
        interner: lowered.interner.clone(),
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let mut extensions = VisibleExtensionMethods::default();
    for item in &module.items {
        let nia_ast::ItemKind::Extend(extend) = &item.kind else {
            continue;
        };
        let Some(target_ty) = lowered.node_type_uses.get(&extend.target.node_key).copied() else {
            continue;
        };
        let target_ty = normalization.normalize(target_ty);
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
            extensions.insert(
                0,
                target_ty,
                VisibleExtensionMethod {
                    name: method_def.name.clone(),
                    def_id: GlobalDefId {
                        module_id: ModuleId(0),
                        def_id: method_id,
                    },
                    impl_index: 0,
                    impl_generics: extend.generics.clone(),
                    trait_id: None,
                    trait_args: Vec::new(),
                    where_predicates: Vec::new(),
                    is_callable: true,
                    is_trait_witness: false,
                },
            );
        }
    }
    let layouts = nia_layout::compute_layouts(
        &defs,
        &lowered.interner,
        &signatures,
        nia_layout::TargetDataLayout::LP64,
    );
    let trait_impls = single_module_trait_impls(ModuleId(0), &signatures, &lowered);
    let origins = NodeOriginTable::default();
    check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: None,
        origins: &origins,
        module: &module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        lowered: &lowered,
        signatures: &signatures,
        normalization: &normalization,
        target: &target,
        comptime: &comptime,
        comptime_module: &comptime_module.module,
        layouts: &layouts,
        extensions: &extensions,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        extension_interner: None,
        program: BodyProgramContext::empty(),
        program_signatures: ProgramSignatureMaps {
            functions: &HashMap::new(),
            globals: &HashMap::new(),
            comptimes: &HashMap::new(),
            structs: &HashMap::new(),
            unions: &HashMap::new(),
            enums: &HashMap::new(),
            traits: &HashMap::new(),
            trait_impls: &trait_impls,
        },
        program_comptime: ProgramComptimeMaps {
            comptimes: &HashMap::new(),
            modules: &HashMap::new(),
        },
    })
}

fn single_module_trait_impls(
    module_id: ModuleId,
    signatures: &nia_item_signatures::ItemSignatures,
    lowered: &nia_type_lower::TypeLowering,
) -> Vec<ProgramTraitImplSignature> {
    signatures
        .trait_impls
        .iter()
        .enumerate()
        .filter_map(|(local_index, impl_signature)| {
            let trait_ty = impl_signature.trait_ty?;
            let (trait_id, trait_args) = trait_id_and_args(&lowered.interner, trait_ty)?;
            Some(ProgramTraitImplSignature {
                module_id,
                local_index,
                generics: impl_signature.generics.clone(),
                target_ty: impl_signature.target_ty,
                trait_id,
                trait_args,
                where_predicates: impl_signature.where_predicates.clone(),
                associated_types: impl_signature.associated_types.clone(),
                interner: lowered.interner.clone(),
            })
        })
        .collect()
}

fn trait_id_and_args(
    interner: &nia_ty::TyInterner,
    ty: nia_ids::InternedTyId,
) -> Option<(TraitId, Vec<nia_ids::InternedTyId>)> {
    match interner.get(ty) {
        Some(TyKind::Nominal { def_id, args }) => Some((TraitId::Source(*def_id), args.clone())),
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            Some((TraitId::Builtin(*trait_id), args.clone()))
        }
        _ => None,
    }
}

pub(super) fn semantic_use_table(
    module_id: ModuleId,
    values: &nia_value_resolve::ValueResolution,
    locals: &nia_local_resolve::LocalResolution,
    type_lowering: &nia_type_lower::TypeLowering,
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
            nia_value_resolve::ValueNameResolution::ImportAlias
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
