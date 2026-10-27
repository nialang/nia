use super::*;

#[test]
fn lowers_types_from_active_item_tree_only() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, errors) = parse_module(
        r#"
@[if false]
fun skipped(value: MissingType) () {}
@[if true]
fun selected(value: i32) () {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let tree = ModuleItemTree::from_module(&module);
    let active = tree.active_items(&mut BoolResolver(false)).unwrap();
    let defs =
        collect_module_defs_from_active_item_tree(module_id, &active).expect("collect definitions");
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
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let lowered = lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&type_store),
    )
    .expect("lower active module types");
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

fun first(pair: &Pair) i32 {
pair.left
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let tree = ModuleItemTree::from_module(&module);
    let active = tree.active_items(&mut BoolResolver(false)).unwrap();
    let defs =
        collect_module_defs_from_active_item_tree(module_id, &active).expect("collect definitions");
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
    let store = nia_ty::TypeStore::new().expect("create type store");
    let declarations = lower_module_declaration_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&store),
    )
    .expect("lower declaration module types");
    let full = lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&store),
    )
    .expect("lower full module types");

    for ty in declarations.explicit_type_roots() {
        assert!(store.get(ty).is_some());
    }
    for ty in full.explicit_type_roots() {
        assert!(store.get(ty).is_some());
    }
}

#[test]
fn declaration_lowering_includes_local_static_types_only() {
    let (module, errors) = parse_module(
        r#"
fun main() () {
    let ignored: bool = false;
    static retained: i32 = 0;
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let module_id = ModuleIdAllocator::new()
        .expect("create module ID allocator")
        .allocate()
        .expect("allocate module ID");
    let tree = ModuleItemTree::from_module(&module);
    let active = tree.active_items(&mut BoolResolver(false)).unwrap();
    let defs =
        collect_module_defs_from_active_item_tree(module_id, &active).expect("collect definitions");
    let resolved = resolve_module_declaration_types_from_active_item_tree(
        &active,
        &defs,
        TypeResolveProgramDefsContext::empty(),
        &nia_defs::PublicSurfaces::default(),
        &nia_defs::ModuleUsingScope::default(),
    );
    let store = nia_ty::TypeStore::new().expect("create type store");
    let lowered = lower_module_declaration_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&store),
    )
    .expect("lower declaration types");

    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    assert_eq!(lowered.type_uses.len(), 2);
    assert!(
        lowered
            .type_uses
            .values()
            .any(|ty| matches!(store.get(*ty), Some(TyKind::Primitive(PrimitiveTy::I32))))
    );
    assert!(
        !lowered
            .type_uses
            .values()
            .any(|ty| matches!(store.get(*ty), Some(TyKind::Primitive(PrimitiveTy::Bool))))
    );
}

#[test]
fn versioned_type_uses_include_const_generic_parameter_types() {
    let (module, errors) = parse_module(
        r#"
struct Buffer[N: usize] {
    values: [u8; N],
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let tree = ModuleItemTree::from_module(&module);
    let active = tree.active_items(&mut BoolResolver(false)).unwrap();
    let defs =
        collect_module_defs_from_active_item_tree(module_id, &active).expect("collect definitions");
    let resolved = resolve_module_types_from_active_item_tree(
        &active,
        &defs,
        TypeResolveProgramDefsContext::empty(),
        &nia_defs::PublicSurfaces::default(),
        &nia_defs::ModuleUsingScope::default(),
    );
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let lowered = lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&type_store),
    )
    .expect("lower active module types");
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);

    let ItemTreeNodeKind::Struct(item_struct) = &active.items[0].kind else {
        panic!("expected struct");
    };
    let GenericParamKind::Const { ty } = &item_struct.generics[0].kind else {
        panic!("expected const generic parameter");
    };
    let uses = lowered.versioned_type_uses_from_active_item_tree(&active);
    assert!(uses.iter().any(|(key, _)| key == &ty.node_key));
}

#[test]
fn declaration_and_full_lowering_share_const_expression_identity() {
    let (module, errors) = parse_module(
        r#"
fun first() [u8; 8 + 8] {
    let body: [u8; 32 + 32] = [0; 64];
    _ = body;
    [0; 16]
}

fun second() [u8; 10 + 10] {
    [0; 20]
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let module_id = ModuleIdAllocator::new()
        .expect("create module ID allocator")
        .allocate()
        .expect("allocate module ID");
    let tree = ModuleItemTree::from_module(&module);
    let active = tree.all_items_active();
    let defs =
        collect_module_defs_from_active_item_tree(module_id, &active).expect("collect definitions");
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

    let store = nia_ty::TypeStore::new().expect("create type store");
    let declarations = lower_module_declaration_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&store),
    )
    .expect("lower declaration types");
    let full = lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&store),
    )
    .expect("lower full types");

    let ItemKind::Function(second) = &module.items[1].kind else {
        panic!("expected second function");
    };
    let TypeKind::Array {
        len: ArrayLen::Expr(second_len),
        ..
    } = &second.return_type.as_ref().expect("return type").kind
    else {
        panic!("expected array return type");
    };
    let id_for_node = |lowering: &TypeLowering| {
        lowering
            .const_exprs
            .iter()
            .find_map(|(id, expr)| (expr.node_key == second_len.node_key).then_some(id.clone()))
    };
    let declaration_id = id_for_node(&declarations).expect("declaration array length");
    assert_eq!(
        Some(declaration_id),
        id_for_node(&full),
        "the same syntax node must keep its const-expression ID across lowering modes"
    );
    assert_eq!(declarations.const_exprs.len(), 2);
    assert_eq!(
        full.const_exprs.len(),
        3,
        "full lowering also publishes the interleaved body expression"
    );

    let other_store = nia_ty::TypeStore::new().expect("create second type store");
    let full_first = lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&other_store),
    )
    .expect("lower full types first");
    let declarations_after = lower_module_declaration_types_from_active_item_tree_with_context(
        module_id,
        &active,
        &resolved,
        TypeLoweringContext::empty(&other_store),
    )
    .expect("lower declaration types after full types");
    assert_eq!(
        id_for_node(&full_first),
        id_for_node(&declarations_after),
        "identity must be stable whichever lowering product requests the node first"
    );
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
