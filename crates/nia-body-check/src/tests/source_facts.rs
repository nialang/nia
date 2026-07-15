// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn records_function_global_value_dependencies() {
    let checked = pipeline(
        r#"
static mut calls: i32 = 0;

fn main() i32 {
    calls += 1;
    calls
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let global_uses = checked
        .facts
        .function_facts
        .values()
        .flat_map(|facts| facts.global_value_uses.iter())
        .collect::<Vec<_>>();
    assert_eq!(global_uses.len(), 1, "{global_uses:?}");
}

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
    let symbols = SymbolTable::new();
    let (module, parse_errors, origins) =
        nia_parser::parse_module_syntax_with_origins_and_symbols(&syntax, symbols.clone());
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let lowered = lower_module_types(&module, &type_resolved);
    let values = nia_value_resolve::resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let active_item_tree = active_item_tree(&module);
    let semantic_uses =
        semantic_use_table(ModuleId(0), &values, &locals, &lowered, &active_item_tree);
    let signatures = collect_item_signatures(&module, &defs, &lowered);
    let target = nia_target_config::TargetConfig::host();
    let source_path = SourcePath::new("/tmp/nia-body-check-test/source-facts.nia");
    let const_module = nia_const_check::lower_module_const(nia_const_check::ConstModuleInput {
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
        const_module.diagnostics.is_empty(),
        "{:?}",
        const_module.diagnostics
    );
    let const_input = nia_const_check::ConstInput {
        module: &const_module.module,
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
        program: nia_const_check::ConstProgramContext::empty(),
    };
    let mut const_interner = lowered.interner.clone();
    let const_array_lengths =
        nia_const_check::compute_module_const_array_lengths(const_input, &mut const_interner);
    let const_enum_values = nia_const_check::compute_module_const_enum_values(
        const_input,
        &mut const_interner,
        const_array_lengths.clone(),
    );
    let const_values = nia_const_check::compute_module_const_values(
        const_input,
        &mut const_interner,
        const_array_lengths.clone(),
        const_enum_values.clone(),
    );
    let const_typed_facts = nia_const_check::compute_module_const_typed_facts(
        const_input,
        &mut const_interner,
        const_array_lengths.clone(),
        const_enum_values,
        const_values.clone(),
    );
    let const_eval = crate::BodyConst::from_phases(
        &const_interner,
        &const_values,
        &const_array_lengths,
        &const_typed_facts,
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
        symbols: &symbols,
        origins: &origins,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        lowered: &lowered,
        signatures: BodyLocalSignatures::from_item_signatures(&signatures),
        const_signatures: &signatures,
        normalization: &normalization,
        seed: None,
        target: &target,
        const_eval,
        const_module: &const_module.module,
        layouts: &layouts,
        extensions: &VisibleExtensionMethods::default(),
        lazy_extensions: None,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        extension_interner: None,
        program: BodyProgramContext::empty(),
        program_signatures: program_signatures.context(),
        function_scope: FunctionCheckScope::LocalModule,
        program_const: ProgramConstMaps::empty(),
        filter: crate::BodyCheckFilter::All,
        product: crate::BodyCheckProduct::Full,
        prechecked: None,
    });

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(checked.facts.iter_node_expr_types().next().is_some());
    assert!(checked.facts.iter_node_expr_types().any(|(key, _)| {
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
    let symbols = SymbolTable::new();
    let (module, parse_errors, origins) =
        nia_parser::parse_module_syntax_with_origins_and_symbols(&syntax, symbols.clone());
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
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
    let const_module = nia_const_check::lower_module_const(nia_const_check::ConstModuleInput {
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
        const_module.diagnostics.is_empty(),
        "{:?}",
        const_module.diagnostics
    );
    let const_input = nia_const_check::ConstInput {
        module: &const_module.module,
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
        program: nia_const_check::ConstProgramContext::empty(),
    };
    let mut const_interner = lowered.interner.clone();
    let const_array_lengths =
        nia_const_check::compute_module_const_array_lengths(const_input, &mut const_interner);
    let const_enum_values = nia_const_check::compute_module_const_enum_values(
        const_input,
        &mut const_interner,
        const_array_lengths.clone(),
    );
    let const_values = nia_const_check::compute_module_const_values(
        const_input,
        &mut const_interner,
        const_array_lengths.clone(),
        const_enum_values.clone(),
    );
    let const_typed_facts = nia_const_check::compute_module_const_typed_facts(
        const_input,
        &mut const_interner,
        const_array_lengths.clone(),
        const_enum_values,
        const_values.clone(),
    );
    let const_eval = crate::BodyConst::from_phases(
        &const_interner,
        &const_values,
        &const_array_lengths,
        &const_typed_facts,
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
        symbols: &symbols,
        origins: &origins,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        lowered: &lowered,
        signatures: BodyLocalSignatures::from_item_signatures(&signatures),
        const_signatures: &signatures,
        normalization: &normalization,
        seed: None,
        target: &target,
        const_eval,
        const_module: &const_module.module,
        layouts: &layouts,
        extensions: &VisibleExtensionMethods::default(),
        lazy_extensions: None,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        extension_interner: None,
        program: BodyProgramContext::empty(),
        program_signatures: program_signatures.context(),
        function_scope: FunctionCheckScope::LocalModule,
        program_const: ProgramConstMaps::empty(),
        filter: crate::BodyCheckFilter::All,
        product: crate::BodyCheckProduct::Full,
        prechecked: None,
    });

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(checked.facts.iter_node_expr_types().any(|(key, _)| {
        key.source_version() == version
            && key.kind() == SyntaxKind::Expr
            && matches!(key.position(), NodePosition::ChildPathRange { .. })
    }));
    assert!(checked.facts.iter_node_builtin_values().any(|(key, _)| {
        key.source_version() == version
            && key.kind() == SyntaxKind::Expr
            && matches!(key.position(), NodePosition::ChildPathRange { .. })
    }));
    assert!(checked.facts.iter_node_resolved_calls().any(|(key, _)| {
        key.source_version() == version
            && key.kind() == SyntaxKind::Expr
            && matches!(key.position(), NodePosition::ChildPathRange { .. })
    }));
    assert!(
        checked
            .facts
            .iter_node_function_references()
            .any(|(key, _)| {
                key.source_version() == version
                    && key.kind() == SyntaxKind::Expr
                    && matches!(key.position(), NodePosition::ChildPathRange { .. })
            })
    );
    assert!(
        checked
            .facts
            .iter_node_pointer_array_to_slice_coercions()
            .any(|(key, _)| {
                key.source_version() == version
                    && key.kind() == SyntaxKind::Expr
                    && matches!(key.position(), NodePosition::ChildPathRange { .. })
            })
    );
}
