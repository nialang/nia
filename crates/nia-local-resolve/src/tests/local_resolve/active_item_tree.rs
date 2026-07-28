use super::*;

#[test]
fn resolves_locals_from_active_item_tree_only() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
@[if false]
fn skipped() i32 {
missing
}
@[if true]
fn selected() i32 {
let mut value = 1;
value
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let tree = ModuleItemTree::from_module(&module);
    let active = tree.active_items(&mut BoolResolver(false)).unwrap();
    let defs = collect_module_defs_from_active_item_tree(module_id, &active);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let values = resolve_module_values_from_active_item_tree(
        &active,
        &defs,
        ValueProgramDefsContext::empty(),
        &nia_defs::PublicSurfaces::default(),
        &nia_defs::ModuleUsingScope::default(),
    );
    assert!(values.diagnostics.is_empty(), "{:?}", values.diagnostics);
    let locals = resolve_module_locals_from_active_item_tree_with_origins(
        &active,
        &defs,
        &values,
        None,
        &nia_node_id::NodeOriginTable::default(),
    );
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    assert!(
        locals
            .locals
            .iter()
            .any(|(_, local)| local.name.symbol() == Some(sym("value")))
    );
}

#[test]
fn filtered_local_resolution_preserves_full_tree_local_ids() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
fn unused(a: i32) i32 {
let mut x = a;
x
}

fn used(b: i32) i32 {
let mut y = b;
y
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let tree = ModuleItemTree::from_module(&module);
    let full = tree.active_items(&mut BoolResolver(true)).unwrap();
    let defs = collect_module_defs_from_active_item_tree(module_id, &full);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let values = resolve_module_values_from_active_item_tree(
        &full,
        &defs,
        ValueProgramDefsContext::empty(),
        &nia_defs::PublicSurfaces::default(),
        &nia_defs::ModuleUsingScope::default(),
    );
    assert!(values.diagnostics.is_empty(), "{:?}", values.diagnostics);
    let full_locals = resolve_module_locals_from_active_item_tree_with_origins(
        &full,
        &defs,
        &values,
        None,
        &nia_node_id::NodeOriginTable::default(),
    );

    let mut filtered = full.clone();
    for item in &mut filtered.items {
        if let ItemTreeNodeKind::Function(function) = &mut item.kind
            && function.name == sym("unused")
        {
            function.body = None;
        }
    }
    let filtered_values = resolve_module_values_from_active_item_tree(
        &filtered,
        &defs,
        ValueProgramDefsContext::empty(),
        &nia_defs::PublicSurfaces::default(),
        &nia_defs::ModuleUsingScope::default(),
    );
    let filtered_locals = resolve_module_locals_from_filtered_active_item_tree_with_origins(
        &filtered,
        &full,
        &defs,
        &filtered_values,
        None,
        &nia_node_id::NodeOriginTable::default(),
    );
    assert!(
        filtered_locals.diagnostics.is_empty(),
        "{:?}",
        filtered_locals.diagnostics
    );

    for name in ["b", "y"] {
        let full_id = local_id_by_name(&full_locals, name);
        let filtered_id = local_id_by_name(&filtered_locals, name);
        assert_eq!(filtered_id, full_id, "local id changed for {name}");
    }
    let unused_x = local_id_by_name(&full_locals, "x");
    assert!(
        !filtered_locals
            .node_uses
            .values()
            .any(|use_kind| *use_kind == LocalUse::Local(unused_x)),
        "{:?}",
        filtered_locals.node_uses
    );
}

fn local_id_by_name(locals: &LocalResolution, name: &str) -> LocalId {
    locals
        .locals
        .iter()
        .find_map(|(id, local)| (local.name.symbol() == Some(sym(name))).then_some(id))
        .unwrap_or_else(|| panic!("expected local `{name}`"))
}

struct BoolResolver(bool);

impl nia_item_tree::ConditionResolver for BoolResolver {
    fn resolve_condition(
        &mut self,
        cond: &nia_ast::ConditionExpr,
    ) -> Result<bool, nia_item_tree::ItemTreeError> {
        match &cond.kind {
            nia_ast::ConditionExprKind::Bool(value) => Ok(*value),
            _ => Ok(self.0),
        }
    }
}
