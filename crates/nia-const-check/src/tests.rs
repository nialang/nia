use crate::{
    ConstCheck, ConstInput, ConstKey, ConstModuleInput, ConstModuleLowering, ConstProgramContext,
    ConstValueType, check_module_const, lower_module_const,
};
use nia_const_ir::{EarlyConstExpr, EarlyConstExprKind, EarlyConstTypeArg};
use nia_defs::{DefCollection, DefKind, ModuleId, collect_module_defs};
use nia_ids::{GlobalDefId, ModuleIdAllocator};
use nia_item_signatures::{ItemSignatureInput, ItemSignatureSource, collect_item_signatures};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_local_resolve::{LocalResolution, resolve_module_locals};
use nia_parser::parse_module_with_symbols;
use nia_sema_ir::SemanticUseTable;
use nia_source::SourcePath;
use nia_span::Span;
use nia_symbol::{SymbolId, stable_hash};
use nia_symbol_table::SymbolTable;
use nia_ty::{PrimitiveTy, TyKind, TypeStore};
use nia_type_lower::{
    TypeLowering, TypeLoweringContext, lower_module_types_from_item_tree_with_context,
    lower_module_types_with_context,
};
use nia_type_resolve::resolve_module_types_with_symbols;
use nia_value_resolve::resolve_module_values;
use std::collections::HashMap;

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

#[path = "tests/typed_values.rs"]
mod typed_values;

struct CheckedFixture {
    module_id: ModuleId,
    type_store: TypeStore,
    defs: DefCollection,
    locals: LocalResolution,
    const_module: ConstModuleLowering,
    checked: ConstCheck,
}

fn check_source(source: &str) -> CheckedFixture {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let symbols = SymbolTable::new();
    let (module, errors) = parse_module_with_symbols(source, symbols.clone());
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let type_names = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let item_tree = ModuleItemTree::from_module(&module);
    let type_store = TypeStore::new();
    let lowered = lower_module_types_from_item_tree_with_context(
        module_id,
        &item_tree,
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
    let active_item_tree =
        ActiveModuleItemTree::new(item_tree.active_items_without_const(), Default::default());
    let semantic_uses =
        semantic_use_table(module_id, &values, &locals, &lowered, &active_item_tree);
    let target = nia_target_config::TargetConfig::host();
    let source_path = SourcePath::new("/tmp/nia-const-check-test/main.nia");
    let const_module = lower_module_const(ConstModuleInput {
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
    let input = ConstInput {
        module: &const_module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        lowered: &lowered,
        signatures: &signatures,
        type_store: &type_store,
        normalization: &nia_type_normalize::TypeNormalization {
            normalized: HashMap::new(),
            diagnostics: Vec::new(),
        },
        target: &target,
        source_path: &source_path,
        program: ConstProgramContext::empty(),
    };
    let checked = check_module_const(input);
    CheckedFixture {
        module_id,
        type_store,
        defs,
        locals,
        const_module,
        checked,
    }
}

fn semantic_use_table(
    module_id: ModuleId,
    values: &nia_value_resolve::ValueResolution,
    locals: &LocalResolution,
    lowered: &TypeLowering,
    active_item_tree: &ActiveModuleItemTree,
) -> SemanticUseTable {
    let mut builder = SemanticUseTable::builder();
    for (key, local_use) in &locals.node_uses {
        if let nia_local_resolve::LocalUse::Local(local_id) = local_use {
            builder.insert_node_local_value_use(key.clone(), *local_id);
        }
    }
    builder.extend_node_global_value_uses(
        values
            .node_qualified_values
            .iter()
            .map(|(key, global_id)| (key.clone(), *global_id)),
    );
    for (key, resolution) in &values.node_names {
        match resolution {
            nia_value_resolve::ValueNameResolution::Def(def_id) => {
                builder.insert_node_global_value_use(
                    key.clone(),
                    GlobalDefId {
                        module_id,
                        def_id: *def_id,
                    },
                );
            }
            nia_value_resolve::ValueNameResolution::External(global_id) => {
                builder.insert_node_global_value_use(key.clone(), *global_id);
            }
            nia_value_resolve::ValueNameResolution::Module
            | nia_value_resolve::ValueNameResolution::LocalDeferred
            | nia_value_resolve::ValueNameResolution::Error => {}
        }
    }
    builder.extend_node_local_defs(
        locals
            .node_local_defs
            .iter()
            .map(|(key, local_id)| (key.clone(), *local_id)),
    );
    builder
        .extend_node_type_uses(lowered.versioned_type_uses_from_active_item_tree(active_item_tree));
    builder.finish()
}

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
