use super::*;

#[test]
fn rejects_const_value_generic_type_arguments() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
struct Box[T] {
value: T,
}

fn make() Box[4] {
Box[4] { value: 0 }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    let (_type_store, lowered) = lower_test_module(&module, &defs, &resolved);
    assert!(
        lowered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("const value generic"))
    );
}

#[test]
fn reports_generic_type_argument_count_mismatches() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
struct Point {}
struct Box[T] { value: T }
type Pair[T, U] = T;
fn missing_arg(a: Box) {}
fn extra_arg(a: Box[i32, bool]) {}
fn alias_missing_arg(a: Pair[i32]) {}
fn non_generic_arg(a: Point[i32]) {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let resolved = resolve_module_types(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let (_type_store, lowered) = lower_test_module(&module, &defs, &resolved);
    let mismatch_count = lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .summary
                .contains("generic argument count mismatch")
        })
        .count();
    assert_eq!(mismatch_count, 4, "{:?}", lowered.diagnostics);
}

#[test]
fn accepts_void_value_types_but_rejects_never_value_types_and_enum_backing_types() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
enum Bad: bool {
A,
}

struct BadFields {
field: (),
array: [1](),
never_field: never,
}

fn bad_param(x: ()) () {}
fn bad_never_param(x: never) () {}
fn good_return() () {}
fn good_never_return() never {}

static mut global_void: ();
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    let (_type_store, lowered) = lower_test_module(&module, &defs, &resolved);
    assert!(
        lowered.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("enum backing type must be an integer type")),
        "{:?}",
        lowered.diagnostics
    );
    assert_eq!(
        lowered
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("`never` is not valid"))
            .count(),
        2,
        "{:?}",
        lowered.diagnostics
    );
}
