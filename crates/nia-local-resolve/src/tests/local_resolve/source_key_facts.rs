use super::*;

#[test]
fn records_local_facts_by_source_versioned_node_keys() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let version = SourceVersion {
        id: SourceId(4),
        revision: SourceRevision(2),
    };
    let syntax = nia_syntax::parse_source(
        r#"
fn main(a: i32) i32 {
let mut x = a;
x
}
"#,
        Some(version),
    );
    let (module, errors, origins) = parse_module_syntax_with_origins(&syntax);
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let values = resolve_module_values(&module, &defs);
    let locals =
        resolve_module_locals_with_origins(&module, &defs, &values, Some(version), &origins);

    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    assert!(!locals.node_local_defs.is_empty());
    assert!(!locals.node_uses.is_empty());
    assert!(locals.node_uses.iter().any(|(key, use_kind)| {
        key.source_version() == version
            && key.kind() == SyntaxKind::Expr
            && matches!(key.position(), NodePosition::ChildPathRange { .. })
            && matches!(use_kind, LocalUse::Local(_))
    }));
}

#[test]
fn records_local_facts_by_red_child_path_origins() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let version = SourceVersion {
        id: SourceId(5),
        revision: SourceRevision(1),
    };
    let syntax = nia_syntax::parse_source(
        r#"
fn main(a: i32) i32 {
let mut x = a;
x
}
"#,
        Some(version),
    );
    let (module, errors, origins) = parse_module_syntax_with_origins(&syntax);
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let values = resolve_module_values(&module, &defs);
    let locals =
        resolve_module_locals_with_origins(&module, &defs, &values, Some(version), &origins);

    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    assert!(locals.node_uses.iter().any(|(key, use_kind)| {
        key.source_version() == version
            && key.kind() == SyntaxKind::Expr
            && matches!(key.position(), NodePosition::ChildPathRange { .. })
            && matches!(use_kind, LocalUse::Local(_))
    }));
}
