// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use crate::*;
pub(super) use nia_body_ir::{BracketSuffixResolution, BuiltinValue};
pub(super) use nia_defs::{
    DefKind, ModuleId, VisibleExtensionMethod, VisibleExtensionMethods, collect_module_defs,
};
pub(super) use nia_item_signatures::{ProgramSignatureMaps, collect_item_signatures};
pub(super) use nia_local_resolve::resolve_module_locals;
pub(super) use nia_node_id::{NodeOriginTable, NodePosition, SyntaxKind};
pub(super) use nia_parser::{parse_module, parse_module_syntax_with_origins};
pub(super) use nia_source::{SourceId, SourceRevision, SourceVersion};
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
            type_uses: &lowered.type_uses,
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
        signatures: &signatures,
        interner: &lowered.interner,
        type_uses: &lowered.type_uses,
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
        let Some(target_ty) = lowered.type_uses.get(&extend.target.span).copied() else {
            continue;
        };
        let target_ty = normalization.normalize(target_ty);
        for method in &extend.methods {
            let Some(method_id) = defs.def_spans.get(method.function.span) else {
                continue;
            };
            let Some(method_def) = defs.defs.get(method_id) else {
                continue;
            };
            if method_def.kind != DefKind::Method {
                continue;
            }
            extensions.insert(
                target_ty,
                VisibleExtensionMethod {
                    name: method_def.name.clone(),
                    def_id: GlobalDefId {
                        module_id: ModuleId(0),
                        def_id: method_id,
                    },
                    trait_id: None,
                    trait_args: Vec::new(),
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
    let origins = NodeOriginTable::default();
    check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: None,
        origins: &origins,
        module: &module,
        defs: &defs,
        values: &values,
        locals: &locals,
        lowered: &lowered,
        signatures: &signatures,
        normalization: &normalization,
        target: &target,
        comptime: &comptime,
        comptime_module: &comptime_module.module,
        layouts: &layouts,
        extensions: &extensions,
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
            trait_impls: &[],
        },
        program_comptime: ProgramComptimeMaps {
            comptimes: &HashMap::new(),
            modules: &HashMap::new(),
        },
    })
}
