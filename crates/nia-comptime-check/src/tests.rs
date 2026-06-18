use crate::{
    ComptimeCheck, ComptimeInput, ComptimeKey, ComptimeModuleInput, ComptimeModuleLowering,
    ComptimeProgramContext, ComptimeValueType, check_module_comptime, lower_module_comptime,
};
use nia_comptime_ir::{EarlyComptimeExpr, EarlyComptimeExprKind, EarlyComptimeTypeArg};
use nia_defs::{DefCollection, DefKind, ModuleId, collect_module_defs};
use nia_ids::GlobalDefId;
use nia_item_signatures::collect_item_signatures;
use nia_local_resolve::{LocalResolution, resolve_module_locals};
use nia_parser::parse_module;
use nia_sema_ir::SemanticUseTable;
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};
use nia_type_lower::{TypeLowering, lower_module_types_with_id};
use nia_type_resolve::resolve_module_types;
use nia_value_resolve::resolve_module_values;
use std::collections::HashMap;

struct CheckedFixture {
    defs: DefCollection,
    locals: LocalResolution,
    lowered: TypeLowering,
    comptime_module: ComptimeModuleLowering,
    checked: ComptimeCheck,
}

fn check_source(source: &str) -> CheckedFixture {
    let (module, errors) = parse_module(source);
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_names = resolve_module_types(&module, &defs);
    let lowered = lower_module_types_with_id(ModuleId(0), &module, &type_names);
    let signatures = collect_item_signatures(&module, &defs, &lowered);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let semantic_uses = semantic_use_table(ModuleId(0), &values, &locals, &lowered);
    let target = nia_target_config::TargetConfig::host();
    let comptime_module = lower_module_comptime(ComptimeModuleInput {
        module: &module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        const_exprs: &lowered.const_exprs,
    });
    let checked = check_module_comptime(ComptimeInput {
        module: &comptime_module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        signatures: &signatures,
        interner: &lowered.interner,
        normalized: &HashMap::new(),
        target: &target,
        program: ComptimeProgramContext::empty(),
    });
    CheckedFixture {
        defs,
        locals,
        lowered,
        comptime_module,
        checked,
    }
}

fn semantic_use_table(
    module_id: ModuleId,
    values: &nia_value_resolve::ValueResolution,
    locals: &LocalResolution,
    lowered: &TypeLowering,
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
    builder.extend_node_type_uses(
        lowered
            .node_type_uses
            .iter()
            .map(|(key, ty)| (key.clone(), *ty)),
    );
    builder.finish()
}

#[test]
fn records_explicit_types_for_comptime_bindings() {
    let fixture = check_source(
        r#"
comptime let width: usize = 4;

fn main() i32 {
comptime let local_width: usize = width;
let xs: [local_width]i32 = [1, 2, 3, 4];
xs[0]
}
"#,
    );
    assert!(
        fixture.comptime_module.diagnostics.is_empty(),
        "{:?}",
        fixture.comptime_module.diagnostics
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
        .get("width")
        .expect("width def");
    let width = fixture
        .checked
        .typed_values
        .get(&ComptimeKey::Global(GlobalDefId {
            module_id: ModuleId(0),
            def_id: width_def,
        }))
        .expect("typed global comptime value");
    assert_eq!(width.ty, ComptimeValueType::Runtime(usize_ty));
    assert!(fixture.locals.locals.iter().any(|(local_id, local)| {
        local.kind == nia_local_resolve::LocalKind::ComptimeBinding
            && fixture
                .checked
                .typed_values
                .get(&ComptimeKey::Local(local_id))
                .is_some_and(|typed| typed.ty == ComptimeValueType::Runtime(usize_ty))
    }));
}

#[test]
fn records_enum_backing_types_for_comptime_variant_values() {
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
        assert_eq!(typed.ty, ComptimeValueType::Runtime(u8_ty));
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
fn semantic_comptime_lowering_requires_resolved_function_locals() {
    let (module, errors) = parse_module(
        r#"
comptime fn add_one(x: usize) usize {
let y = x + 1;
y
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_names = resolve_module_types(&module, &defs);
    let lowered = lower_module_types_with_id(ModuleId(0), &module, &type_names);
    let values = resolve_module_values(&module, &defs);
    let mut locals = resolve_module_locals(&module, &defs, &values);
    let removed_key = locals.node_local_defs.iter().find_map(|(key, local_id)| {
        let local = locals.locals.get(*local_id)?;
        (local.name == "y").then_some(key.clone())
    });
    let removed_key = removed_key.expect("local y node key");
    locals.node_local_defs.remove(&removed_key);
    let removed_span = locals
        .locals
        .iter()
        .find_map(|(_, local)| (local.name == "y").then_some(local.span))
        .expect("local y span");
    let semantic_uses = semantic_use_table(ModuleId(0), &values, &locals, &lowered);

    let comptime_module = lower_module_comptime(ComptimeModuleInput {
        module: &module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        const_exprs: &lowered.const_exprs,
    });

    assert!(
        comptime_module.diagnostics.iter().any(|diagnostic| {
            diagnostic.primary_span() == Some(removed_span)
                && diagnostic.summary == "failed to resolve comptime local binding"
        }),
        "{:?}",
        comptime_module.diagnostics
    );
}

#[test]
fn layout_builtin_requires_resolved_type_arg() {
    let expr = EarlyComptimeExpr {
        span: Span::new(0, 1),
        kind: EarlyComptimeExprKind::LayoutBuiltin {
            builtin: nia_ids::LayoutBuiltin::Size,
            type_arg: EarlyComptimeTypeArg {
                span: Span::new(0, 1),
                ty_span: Span::new(0, 1),
                ty: None,
            },
        },
    };

    let err = nia_comptime_ir::resolve_expr(expr).expect_err("layout builtin should not resolve");
    assert_eq!(err.message, "failed to resolve comptime type argument");
}
