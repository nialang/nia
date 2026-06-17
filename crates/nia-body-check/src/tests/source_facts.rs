// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn records_body_facts_by_source_versioned_node_keys() {
    let version = SourceVersion {
        id: SourceId(7),
        revision: SourceRevision(3),
    };
    let syntax = nia_syntax::parse_source(
        r#"
fn main() i32 {
    var x = 1;
    x
}
"#,
        Some(version),
    );
    let (module, parse_errors, origins) = parse_module_syntax_with_origins(&syntax);
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_resolved = resolve_module_types(&module, &defs);
    let lowered = lower_module_types(&module, &type_resolved);
    let values = nia_value_resolve::resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let semantic_uses = semantic_use_table(ModuleId(0), &values, &locals, &lowered);
    let signatures = collect_item_signatures(&module, &defs, &lowered);
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
    let normalization = TypeNormalization {
        interner: lowered.interner.clone(),
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let layouts = nia_layout::compute_layouts(
        &defs,
        &lowered.interner,
        &signatures,
        nia_layout::TargetDataLayout::LP64,
    );
    let checked = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: Some(version),
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
        extensions: &VisibleExtensionMethods::default(),
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
            type_aliases: &HashMap::new(),
            trait_impls: &[],
        },
        program_comptime: ProgramComptimeMaps {
            comptimes: &HashMap::new(),
            modules: &HashMap::new(),
        },
        filter: crate::BodyCheckFilter::All,
    });

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(!checked.facts.node_expr_types.is_empty());
    assert!(checked.facts.node_expr_types.keys().any(|key| {
        key.source_version() == version
            && key.kind == SyntaxKind::Expr
            && matches!(key.position, NodePosition::ChildPathRange { .. })
    }));
}

#[test]
fn records_body_facts_by_red_child_path_origins() {
    let version = SourceVersion {
        id: SourceId(8),
        revision: SourceRevision(2),
    };
    let syntax = nia_syntax::parse_source(
        r#"
struct Pair {
    a: u8,
    b: i32,
}

fn id(value: i32) i32 { value }
fn call_ptr(f: &fn(i32) i32, value: i32) i32 { f(value) }
fn take(xs: & [i32]) usize { xs.len() }
fn text(xs: & [char]) usize { xs.len() }
fn cstr(value: & u8) usize { 1 }

fn main() i32 {
    var x = 1;
    var y = id(x);
    var n = @size[Pair]();
    var s = take(&[1, 2, 3]);
    var literal_slice: &[i32] = &[4, 5, 6];
    var t = text("ok");
    let ok = b"ok\0";
    var p = cstr(&(ok.*[0]));
    var q = call_ptr(& id, y);
    q + n as i32 + s as i32 + literal_slice.len() as i32 + t as i32 + p as i32
}
"#,
        Some(version),
    );
    let (module, parse_errors, origins) = parse_module_syntax_with_origins(&syntax);
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_resolved = resolve_module_types(&module, &defs);
    let lowered = lower_module_types(&module, &type_resolved);
    let values = nia_value_resolve::resolve_module_values(&module, &defs);
    let locals = nia_local_resolve::resolve_module_locals_with_origins(
        &module,
        &defs,
        &values,
        Some(version),
        &origins,
    );
    let semantic_uses = semantic_use_table(ModuleId(0), &values, &locals, &lowered);
    let signatures = collect_item_signatures(&module, &defs, &lowered);
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
    let normalization = TypeNormalization {
        interner: lowered.interner.clone(),
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let layouts = nia_layout::compute_layouts(
        &defs,
        &lowered.interner,
        &signatures,
        nia_layout::TargetDataLayout::LP64,
    );
    let checked = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: Some(version),
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
        extensions: &VisibleExtensionMethods::default(),
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
            type_aliases: &HashMap::new(),
            trait_impls: &[],
        },
        program_comptime: ProgramComptimeMaps {
            comptimes: &HashMap::new(),
            modules: &HashMap::new(),
        },
        filter: crate::BodyCheckFilter::All,
    });

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(checked.facts.node_expr_types.keys().any(|key| {
        key.source_version() == version
            && key.kind == SyntaxKind::Expr
            && matches!(key.position, NodePosition::ChildPathRange { .. })
    }));
    assert!(checked.facts.node_builtin_values.keys().any(|key| {
        key.source_version() == version
            && key.kind == SyntaxKind::Expr
            && matches!(key.position, NodePosition::ChildPathRange { .. })
    }));
    assert!(checked.facts.node_resolved_calls.keys().any(|key| {
        key.source_version() == version
            && key.kind == SyntaxKind::Expr
            && matches!(key.position, NodePosition::ChildPathRange { .. })
    }));
    assert!(checked.facts.node_function_references.keys().any(|key| {
        key.source_version() == version
            && key.kind == SyntaxKind::Expr
            && matches!(key.position, NodePosition::ChildPathRange { .. })
    }));
    assert!(
        checked
            .facts
            .node_pointer_array_to_slice_coercions
            .keys()
            .any(|key| {
                key.source_version() == version
                    && key.kind == SyntaxKind::Expr
                    && matches!(key.position, NodePosition::ChildPathRange { .. })
            })
    );
}
