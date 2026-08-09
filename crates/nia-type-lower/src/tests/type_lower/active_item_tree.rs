use super::*;

#[test]
fn lowers_types_from_active_item_tree_only() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
@[if false]
fn skipped(value: MissingType) () {}
@[if true]
fn selected(value: i32) () {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let tree = ModuleItemTree::from_module(&module);
    let active = tree.active_items(&mut BoolResolver(false)).unwrap();
    let defs = collect_module_defs_from_active_item_tree(module_id, &active);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let resolved = resolve_module_types_from_active_item_tree(
        &active,
        &defs,
        TypeResolveProgramDefsContext::empty(),
        &nia_defs::PublicSurfaces::default(),
        &nia_defs::ModuleUsingScope::default(),
    );
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let type_store = nia_ty::TypeStore::new();
    let lowered = lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&type_store),
    );
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    assert!(lowered.type_uses.values().any(|ty| matches!(
        type_store.get(*ty),
        Some(TyKind::Primitive(PrimitiveTy::I32))
    )));
}

#[test]
fn lowering_variants_publish_to_one_canonical_store() {
    let (module, errors) = parse_module(
        r#"
struct Pair {
left: i32,
right: i32,
}

fn first(pair: &Pair) i32 {
pair.left
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let tree = ModuleItemTree::from_module(&module);
    let active = tree.active_items(&mut BoolResolver(false)).unwrap();
    let defs = collect_module_defs_from_active_item_tree(module_id, &active);
    let resolved = resolve_module_types_from_active_item_tree(
        &active,
        &defs,
        TypeResolveProgramDefsContext::empty(),
        &nia_defs::PublicSurfaces::default(),
        &nia_defs::ModuleUsingScope::default(),
    );
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let store = nia_ty::TypeStore::new();
    let declarations = lower_module_declaration_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&store),
    );
    let full = lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&store),
    );

    for ty in declarations.explicit_type_roots() {
        assert!(store.get(ty).is_some());
    }
    for ty in full.explicit_type_roots() {
        assert!(store.get(ty).is_some());
    }
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
