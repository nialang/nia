use super::*;

#[test]
fn resolves_index_expr_inside_field_bracket_suffix() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
struct S {
x: i32,
}

struct T {
xs: [4]S,
}

fn main() i32 {
let mut t = T { xs: [S { x: 0 }; 4] };
for i in 0u16..4u16 {
    t.xs[i as usize] = S { x: i as i32 };
}
t.xs[2].x
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    let i_id = locals
        .locals
        .iter()
        .find_map(|(id, local)| (local.name.symbol() == Some(sym("i"))).then_some(id))
        .expect("expected loop local");
    assert!(
        locals
            .node_uses
            .values()
            .any(|use_kind| *use_kind == LocalUse::Local(i_id)),
        "{:?}",
        locals.node_uses
    );
}

#[test]
fn resolves_local_named_like_type_inside_field_bracket_suffix() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
struct S {
x: i32,
}

struct T {
xs: [4]S,
}

fn main() i32 {
let mut t = T { xs: [S { x: 0 }; 4] };
let mut i32: usize = 2;
t.xs[i32].x
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    let i32_id = locals
        .locals
        .iter()
        .find_map(|(id, local)| (local.name.symbol() == Some(sym("i32"))).then_some(id))
        .expect("expected local named i32");
    assert!(
        locals
            .node_uses
            .values()
            .any(|use_kind| *use_kind == LocalUse::Local(i32_id)),
        "{:?}",
        locals.node_uses
    );
}
