use super::*;

#[test]
fn resolves_params_and_local_bindings() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
static mut global = 1;

fn add(a: i32, b: i32) i32 {
let mut sum = a + b + global;
sum
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
            .any(|use_kind| matches!(use_kind, LocalUse::Local(_)))
    );
    assert!(
        locals
            .node_uses
            .values()
            .any(|use_kind| matches!(use_kind, LocalUse::ModuleValue))
    );
}

#[test]
fn lexical_locals_shadow_module_values() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let source = r#"
static mut value = 1;

fn id(value: i32) i32 {
value
}
"#;
    let (module, errors) = parse_module(source);
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    assert!(
        locals
            .node_uses
            .values()
            .any(|use_kind| matches!(*use_kind, LocalUse::Local(_)))
    );
}

#[test]
fn if_pattern_payload_locals_shadow_external_values_in_field_lhs() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let external_module_id = module_ids.allocate();
    let source = r#"
struct S {
start: i32,
}

fn value(input: ?S) ?i32 {
match input {
    ?range => {
        ?range.start
    },
    null => {
        null
    },
}
}
"#;
    let (module, errors) = parse_module(source);
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let values = resolve_module_values(&module, &defs);
    let mut values = values.into_builder();
    for item in &module.items {
        if let nia_ast::ItemKind::Function(function) = &item.kind
            && function.name == sym("value")
            && let Some(body) = &function.body
            && let Some(expr) = &body.tail
            && let ExprKind::IfPattern(if_pattern) = &expr.kind
            && let Some(arm_expr) = if_pattern.then_branch.tail.as_deref()
            && let ExprKind::OptionalSome { expr: some_expr } = &arm_expr.kind
            && let ExprKind::Field { lhs, .. } = &some_expr.kind
            && let ExprKind::Ident(name) = &lhs.kind
            && *name == sym("range")
        {
            values.insert_node_name(
                lhs.node_key.clone(),
                ValueNameResolution::External(GlobalDefId {
                    module_id: external_module_id,
                    def_id: DefId(1),
                }),
            );
        }
    }
    let values = values.finish();
    let locals = resolve_module_locals(&module, &defs, &values);
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    let range_id = locals
        .locals
        .iter()
        .find_map(|(id, local)| (local.name.symbol() == Some(sym("range"))).then_some(id))
        .expect("expected if pattern payload local");
    assert!(
        locals
            .node_uses
            .values()
            .any(|use_kind| *use_kind == LocalUse::Local(range_id)),
        "{:?}",
        locals.node_uses
    );
}
