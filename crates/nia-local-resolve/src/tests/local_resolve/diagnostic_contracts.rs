use super::*;

#[test]
fn reports_unresolved_deferred_names() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
fn main() i32 {
missing
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    assert!(
        locals
            .node_uses
            .values()
            .any(|use_kind| matches!(use_kind, LocalUse::Unresolved)),
        "{:?}",
        locals.node_uses
    );
}

#[test]
fn reports_duplicates_in_same_scope() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
fn main(a: i32, a: i32) i32 {
let mut x = 1;
let mut x = 2;
x
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    assert_eq!(locals.diagnostics.len(), 2);
    assert!(
        locals
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("duplicate parameter name"))
    );
    assert!(
        locals
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("duplicate local binding"))
    );
}

#[test]
fn marks_type_prefixes_for_associated_functions_and_enum_variants() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
struct Point {
x: i32,
}

extend Point {
fn origin() Point {
    Self { x: 0 }
}
}

enum Color {
Red,
}

fn main() Point {
let mut c = Color::Red;
Point::origin()
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    assert!(
        locals
            .node_uses
            .values()
            .any(|use_kind| matches!(use_kind, LocalUse::TypePrefix))
    );
}
