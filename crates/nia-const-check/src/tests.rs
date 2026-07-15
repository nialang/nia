use crate::{
    ConstCheck, ConstInput, ConstKey, ConstModuleInput, ConstModuleLowering, ConstProgramContext,
    ConstValueType, check_module_const, lower_module_const,
};
use nia_const_ir::{EarlyConstExpr, EarlyConstExprKind, EarlyConstTypeArg};
use nia_defs::{DefCollection, DefKind, ModuleId, collect_module_defs};
use nia_ids::GlobalDefId;
use nia_item_signatures::collect_item_signatures;
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_local_resolve::{LocalResolution, resolve_module_locals};
use nia_parser::parse_module_with_symbols;
use nia_sema_ir::SemanticUseTable;
use nia_source::SourcePath;
use nia_span::Span;
use nia_symbol::{SymbolId, stable_hash};
use nia_symbol_table::SymbolTable;
use nia_ty::{PrimitiveTy, TyKind};
use nia_type_lower::{TypeLowering, lower_module_types_with_id};
use nia_type_resolve::resolve_module_types_with_symbols;
use nia_value_resolve::resolve_module_values;
use std::collections::HashMap;

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

struct CheckedFixture {
    defs: DefCollection,
    locals: LocalResolution,
    lowered: TypeLowering,
    const_module: ConstModuleLowering,
    checked: ConstCheck,
}

fn check_source(source: &str) -> CheckedFixture {
    let symbols = SymbolTable::new();
    let (module, errors) = parse_module_with_symbols(source, symbols.clone());
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_names = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let lowered = lower_module_types_with_id(ModuleId(0), &module, &type_names);
    let signatures = collect_item_signatures(&module, &defs, &lowered);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let item_tree = ModuleItemTree::from_module(&module);
    let active_item_tree =
        ActiveModuleItemTree::new(item_tree.active_items_without_const(), Default::default());
    let semantic_uses =
        semantic_use_table(ModuleId(0), &values, &locals, &lowered, &active_item_tree);
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
    let checked = check_module_const(ConstInput {
        module: &const_module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        lowered: &lowered,
        signatures: &signatures,
        interner: &lowered.interner,
        normalized: &HashMap::new(),
        target: &target,
        source_path: &source_path,
        program: ConstProgramContext::empty(),
    });
    assert_eq!(
        lowered.interner.interner_id(),
        checked.interner.interner_id()
    );
    assert!(lowered.interner.is_prefix_of(&checked.interner));
    CheckedFixture {
        defs,
        locals,
        lowered,
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
fn records_explicit_types_for_const_bindings() {
    let fixture = check_source(
        r#"
const width: usize = 4;

fn main() i32 {
const local_width: usize = width;
static xs: [local_width]i32 = [1, 2, 3, 4];
xs[0]
}
"#,
    );
    assert!(
        fixture.const_module.diagnostics.is_empty(),
        "{:?}",
        fixture.const_module.diagnostics
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    let usize_ty = fixture.lowered.interner.primitive(PrimitiveTy::Usize);
    let width_def = fixture
        .defs
        .module_scope
        .values
        .get(&sym("width"))
        .expect("width def");
    let width = fixture
        .checked
        .typed_values
        .get(&ConstKey::Global(GlobalDefId {
            module_id: ModuleId(0),
            def_id: width_def,
        }))
        .expect("typed global const value");
    assert_eq!(width.ty, ConstValueType::Runtime(usize_ty));
    assert!(fixture.locals.locals.iter().any(|(local_id, local)| {
        local.kind == nia_local_resolve::LocalKind::ConstBinding
            && fixture
                .checked
                .typed_values
                .get(&ConstKey::Local(local_id))
                .is_some_and(|typed| typed.ty == ConstValueType::Runtime(usize_ty))
    }));
}

#[test]
fn evaluates_field_offset_builtin_at_const() {
    let fixture = check_source(
        r#"
extern struct Pair {
    a: u8,
    b: u32,
}

const OFF: usize = std::builtin::offset[Pair]("b");
"#,
    );
    assert!(
        fixture.const_module.diagnostics.is_empty(),
        "{:?}",
        fixture.const_module.diagnostics
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    let off_def = fixture
        .defs
        .module_scope
        .values
        .get(&sym("OFF"))
        .expect("OFF def");
    let typed = fixture
        .checked
        .typed_values
        .get(&ConstKey::Global(GlobalDefId {
            module_id: ModuleId(0),
            def_id: off_def,
        }))
        .expect("typed global const value");
    assert_eq!(
        typed.value,
        nia_const_eval::ConstValue::Int(nia_ty::IntConst::unsigned(4))
    );
}

#[test]
fn records_enum_backing_types_for_const_variant_values() {
    let fixture = check_source(
        r#"
enum Code: u8 {
ok = 1,
fail = 2,
}
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    let u8_ty = fixture.lowered.interner.primitive(PrimitiveTy::U8);
    let variants = fixture
        .defs
        .defs
        .iter()
        .filter_map(|(def_id, def)| (def.kind == DefKind::EnumVariant).then_some(def_id));
    for variant in variants {
        let typed = fixture
            .checked
            .typed_enum_values
            .get(&variant)
            .expect("typed enum variant value");
        assert_eq!(typed.ty, ConstValueType::Runtime(u8_ty));
        assert!(matches!(
            typed
                .ty
                .runtime()
                .and_then(|ty| fixture.lowered.interner.get(ty)),
            Some(TyKind::Primitive(PrimitiveTy::U8))
        ));
    }
}

#[test]
fn semantic_const_lowering_requires_resolved_function_locals() {
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
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_names = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let lowered = lower_module_types_with_id(ModuleId(0), &module, &type_names);
    let signatures = collect_item_signatures(&module, &defs, &lowered);
    let values = resolve_module_values(&module, &defs);
    let mut locals = resolve_module_locals(&module, &defs, &values);
    let removed_key = locals.node_local_defs.iter().find_map(|(key, local_id)| {
        let local = locals.locals.get(*local_id)?;
        (local.name.symbol() == Some(sym("y"))).then_some(key.clone())
    });
    let removed_key = removed_key.expect("local y node key");
    locals.node_local_defs.remove(&removed_key);
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
        semantic_use_table(ModuleId(0), &values, &locals, &lowered, &active_item_tree);
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
