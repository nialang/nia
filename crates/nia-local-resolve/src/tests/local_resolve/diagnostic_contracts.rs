use super::*;

#[test]
fn reports_unresolved_deferred_names() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, errors) = parse_module(
        r#"
fn main() i32 {
missing
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values).expect("resolve locals");
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
fn closure_body_requires_explicit_outer_local_captures() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, errors) = parse_module(
        r#"
fn main() i32 {
    let make = \x: i32, y: i32 -> \z: i32 -> x * y + z;
    let add = make(2, 3);
    add(4)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values).expect("resolve locals");

    let missing_captures = locals
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.summary.contains("not captured by this closure"))
        .collect::<Vec<_>>();
    assert_eq!(missing_captures.len(), 2, "{:?}", locals.diagnostics);
    assert_ne!(
        missing_captures[0].primary_span(),
        missing_captures[1].primary_span()
    );
    assert!(
        missing_captures.iter().all(|diagnostic| diagnostic
            .help
            .iter()
            .any(|help| help.contains("capture list"))),
        "missing capture diagnostics should explain the fix: {:?}",
        missing_captures
    );
}

#[test]
fn closure_capture_initializers_resolve_outside_the_body_boundary() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, errors) = parse_module(
        r#"
fn main() i32 {
    let make = \x: i32, y: i32 -> \[x, y] z: i32 -> x * y + z;
    let add = make(2, 3);
    add(4)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values).expect("resolve locals");

    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    assert!(
        locals
            .node_uses
            .values()
            .all(|use_kind| !matches!(use_kind, LocalUse::Unresolved)),
        "{:?}",
        locals.node_uses
    );
}

#[test]
fn closure_body_cannot_implicitly_capture_a_method_receiver() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, errors) = parse_module(
        r#"
struct Box { value: i32 }

extend Box {
    fn run(self) i32 {
        let callback = \z: i32 -> self.value + z;
        callback(1)
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values).expect("resolve locals");

    assert!(
        locals.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("`self` cannot cross a closure boundary")),
        "{:?}",
        locals.diagnostics
    );
}

#[test]
fn binding_initializer_cannot_reference_binding_being_defined() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, errors) = parse_module(
        r#"
fn main() i32 {
    let f = f;
    0
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values).expect("resolve locals");

    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    assert!(
        locals
            .node_uses
            .values()
            .any(|use_kind| matches!(use_kind, LocalUse::Unresolved))
    );
    assert_eq!(
        locals.node_local_defs.len(),
        1,
        "the binding itself is still allocated even though its initializer is unresolved"
    );
}

#[test]
fn reports_duplicates_in_same_scope() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
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
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values).expect("resolve locals");
    assert_eq!(locals.diagnostics.len(), 1);
    assert!(
        locals
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("duplicate parameter name"))
    );
    let duplicate_parameter = locals
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.summary.contains("duplicate parameter name"))
        .expect("duplicate parameter diagnostic");
    assert_eq!(duplicate_parameter.related.len(), 1);
    assert!(
        duplicate_parameter.related[0]
            .message
            .contains("already declared")
    );
    assert!(
        !locals
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("duplicate local binding"))
    );
}

#[test]
fn reports_duplicate_bindings_within_one_pattern() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, errors) = parse_module(
        r#"
fn main() i32 {
let (x, x) = (1, 2);
x
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values).expect("resolve locals");
    assert_eq!(locals.diagnostics.len(), 1);
    assert!(
        locals.diagnostics[0]
            .summary
            .contains("duplicate local binding")
    );
    assert_eq!(locals.diagnostics[0].related.len(), 1);
    assert!(
        locals.diagnostics[0].related[0]
            .message
            .contains("already bound by this pattern")
    );
}

#[test]
fn marks_type_prefixes_for_associated_functions_and_enum_variants() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
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
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values).expect("resolve locals");
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    assert!(
        locals
            .node_uses
            .values()
            .any(|use_kind| matches!(use_kind, LocalUse::TypePrefix))
    );
}

#[test]
fn records_nominal_pattern_constructor_identity() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, errors) = parse_module(
        r#"
struct Point { x: i32 }

fn read(point: Point) i32 {
    let Point { x } = point;
    x
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let point = GlobalDefId {
        module_id,
        def_id: defs
            .module_scope
            .types
            .get(&sym("Point"))
            .expect("Point definition"),
    };
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values).expect("resolve locals");
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    assert!(locals.node_type_prefixes.values().any(|id| *id == point));
}
