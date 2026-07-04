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
    let mut x = 1;
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
    let active_item_tree = active_item_tree(&module);
    let semantic_uses =
        semantic_use_table(ModuleId(0), &values, &locals, &lowered, &active_item_tree);
    let signatures = collect_item_signatures(&module, &defs, &lowered);
    let target = nia_target_config::TargetConfig::host();
    let source_path = SourcePath::new("/tmp/nia-body-check-test/source-facts.nia");
    let comptime_module =
        nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            signatures: &signatures,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
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
    let program_signatures = EmptyBodyProgramSignatures::new();
    let checked = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: Some(version),
        source_path: &source_path,
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
        extensions: &VisibleExtensionMethods::default(),
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        extension_interner: None,
        program: BodyProgramContext::empty(),
        program_signatures: program_signatures.context(),
        function_scope: FunctionCheckScope::LocalModule,
        program_comptime: ProgramComptimeMaps::empty(),
        filter: crate::BodyCheckFilter::All,
    });

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(!checked.facts.node_expr_types.is_empty());
    assert!(checked.facts.node_expr_types.keys().any(|key| {
        key.source_version() == version
            && key.kind() == SyntaxKind::Expr
            && matches!(key.position(), NodePosition::ChildPathRange { .. })
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
    let mut x = 1;
    let mut y = id(x);
    let mut n = std::builtin::size[Pair]();
    let mut s = take(&[1, 2, 3]);
    let mut literal_slice: &[i32] = &[4, 5, 6];
    let mut t = text(&"ok");
    let ok = b"ok\0";
    let mut p = cstr(&ok[0]);
    let mut q = call_ptr(& id, y);
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
    let active_item_tree = active_item_tree(&module);
    let semantic_uses =
        semantic_use_table(ModuleId(0), &values, &locals, &lowered, &active_item_tree);
    let signatures = collect_item_signatures(&module, &defs, &lowered);
    let target = nia_target_config::TargetConfig::host();
    let source_path = SourcePath::new("/tmp/nia-body-check-test/source-facts-red.nia");
    let comptime_module =
        nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            signatures: &signatures,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
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
    let program_signatures = EmptyBodyProgramSignatures::new();
    let checked = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: Some(version),
        source_path: &source_path,
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
        extensions: &VisibleExtensionMethods::default(),
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        extension_interner: None,
        program: BodyProgramContext::empty(),
        program_signatures: program_signatures.context(),
        function_scope: FunctionCheckScope::LocalModule,
        program_comptime: ProgramComptimeMaps::empty(),
        filter: crate::BodyCheckFilter::All,
    });

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(checked.facts.node_expr_types.keys().any(|key| {
        key.source_version() == version
            && key.kind() == SyntaxKind::Expr
            && matches!(key.position(), NodePosition::ChildPathRange { .. })
    }));
    assert!(checked.facts.node_builtin_values.keys().any(|key| {
        key.source_version() == version
            && key.kind() == SyntaxKind::Expr
            && matches!(key.position(), NodePosition::ChildPathRange { .. })
    }));
    assert!(checked.facts.node_resolved_calls.keys().any(|key| {
        key.source_version() == version
            && key.kind() == SyntaxKind::Expr
            && matches!(key.position(), NodePosition::ChildPathRange { .. })
    }));
    assert!(checked.facts.node_function_references.keys().any(|key| {
        key.source_version() == version
            && key.kind() == SyntaxKind::Expr
            && matches!(key.position(), NodePosition::ChildPathRange { .. })
    }));
    assert!(
        checked
            .facts
            .node_pointer_array_to_slice_coercions
            .keys()
            .any(|key| {
                key.source_version() == version
                    && key.kind() == SyntaxKind::Expr
                    && matches!(key.position(), NodePosition::ChildPathRange { .. })
            })
    );
}
