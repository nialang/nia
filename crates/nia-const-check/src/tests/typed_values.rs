use super::*;

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
    let usize_ty = fixture
        .type_store
        .append_for_module(fixture.module_id)
        .intern(TyKind::Primitive(PrimitiveTy::Usize));
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
            module_id: fixture.module_id,
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
            module_id: fixture.module_id,
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
    let u8_ty = fixture
        .type_store
        .append_for_module(fixture.module_id)
        .intern(TyKind::Primitive(PrimitiveTy::U8));
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
            typed.ty.runtime().and_then(|ty| fixture.type_store.get(ty)),
            Some(TyKind::Primitive(PrimitiveTy::U8))
        ));
    }
}

#[test]
fn rejects_payload_enum_tags_outside_the_backing_range() {
    let fixture = check_source(
        r#"
enum Packet {
    Data(i32) = 255,
    Next(i32),
    Negative { value: i32 } = -1,
}
"#,
    );
    assert_eq!(
        fixture
            .checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("out of range for backing type"))
            .count(),
        2,
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn const_integer_operations_use_target_pointer_width() {
    let mut target = nia_target_config::TargetConfig::host();
    target.pointer_width = 32;
    let fixture = check_source_for_target(
        r#"
const hiddenOverflow: usize = (4294967295usize + 1usize) - 1usize;
"#,
        target,
    );
    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("integer overflow in const addition")),
        "{:?}",
        fixture.checked.diagnostics
    );
}
