use super::*;

#[test]
fn semantic_const_lowering_requires_resolved_function_locals() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let symbols = SymbolTable::new();
    let (module, errors) = parse_module_with_symbols(
        r#"
const fn add_one(x: usize) usize {
let y = x + 1;
y
}
"#,
        symbols.clone(),
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let type_names = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let type_store = TypeStore::new();
    let lowered = lower_module_types_with_context(
        module_id,
        &module,
        &type_names,
        TypeLoweringContext::empty(&type_store),
    );
    let signatures = collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(&module),
        defs: &defs,
        lowered: &lowered,
        type_store: &type_store,
        symbols: None,
    });
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let removed_key = locals.node_local_defs.iter().find_map(|(key, local_id)| {
        let local = locals.locals.get(*local_id)?;
        (local.name.symbol() == Some(sym("y"))).then_some(key.clone())
    });
    let removed_key = removed_key.expect("local y node key");
    let mut locals = locals.into_builder();
    locals.remove_node_local_def(&removed_key);
    let locals = locals.finish();
    let removed_span = module
        .items
        .iter()
        .find_map(|item| {
            let nia_ast::ItemKind::Function(function) = &item.kind else {
                return None;
            };
            let body = function.body.as_ref()?;
            body.stmts.iter().find_map(|stmt| {
                let nia_ast::StmtKind::Binding(binding) = &stmt.kind else {
                    return None;
                };
                match &binding.pattern.kind {
                    nia_ast::PatternKind::Bind { name, .. } if *name == sym("y") => {
                        Some(binding.pattern.span)
                    }
                    _ => None,
                }
            })
        })
        .expect("local y pattern span");
    let item_tree = ModuleItemTree::from_module(&module);
    let active_item_tree =
        ActiveModuleItemTree::new(item_tree.active_items_without_const(), Default::default());
    let semantic_uses =
        semantic_use_table(module_id, &values, &locals, &lowered, &active_item_tree);
    let source_path = SourcePath::new("/tmp/nia-const-check-test/lowering.nia");

    let const_module = lower_module_const(ConstModuleInput {
        type_store: &type_store,
        defs_for_module: None,
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
        const_module.diagnostics.iter().any(|diagnostic| {
            diagnostic.primary_span() == Some(removed_span)
                && diagnostic.summary == "failed to resolve const local binding"
        }),
        "{:?}",
        const_module.diagnostics
    );
}

#[test]
fn layout_builtin_requires_resolved_type_arg() {
    let expr = EarlyConstExpr {
        span: Span::new(0, 1),
        kind: EarlyConstExprKind::LayoutBuiltin {
            builtin: nia_ids::LayoutBuiltin::Size,
            type_arg: EarlyConstTypeArg {
                span: Span::new(0, 1),
                ty_span: Span::new(0, 1),
                ty: None,
            },
        },
    };

    let err = nia_const_ir::resolve_expr(expr).expect_err("layout builtin should not resolve");
    assert_eq!(err.message, "failed to resolve const type argument");
}
