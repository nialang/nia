use super::*;
use nia_const_eval::ConstValue;
use nia_ty::IntConst;

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
const shiftOverflow: usize = 1usize << 32usize;
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
    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("shift count is out of range in const expression")),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn evaluates_scalar_union_reinterpretation_and_field_switching() {
    let fixture = check_source(
        r#"
union Bits {
    integer: u32,
    float: f32,
}

union SignedBits {
    unsigned: u32,
    signed: i32,
}

const fn floatBits() u32 {
    let bits: Bits = { float: 1.0 };
    bits.integer
}

const fn switchedBits() f32 {
    let mut bits: Bits = { float: 0.0 };
    bits.integer = 1065353216;
    bits.float
}

const FLOAT_BITS: u32 = floatBits();
const SWITCHED: f32 = switchedBits();
const SIGNED_BITS: SignedBits = { unsigned: 4294967295 };
const SIGNED: i32 = SIGNED_BITS.signed;
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
    assert_eq!(
        const_value(&fixture, "FLOAT_BITS"),
        ConstValue::Int(IntConst::unsigned(1065353216))
    );
    assert_eq!(const_value(&fixture, "SWITCHED"), ConstValue::Float(1.0));
    assert_eq!(
        const_value(&fixture, "SIGNED"),
        ConstValue::Int(IntConst::from_i128(-1))
    );
}

#[test]
fn scalar_union_reinterpretation_uses_artifact_endianness() {
    let source = r#"
union Narrow {
    wide: u32,
    narrow: u16,
}

const BITS: Narrow = { wide: 287454020 };
const VALUE: u16 = BITS.narrow;
"#;
    let mut little_target = nia_target_config::TargetConfig::host();
    little_target.endian = "little".to_string();
    let little = check_source_for_target(source, little_target);
    assert!(
        little.checked.diagnostics.is_empty(),
        "{:?}",
        little.checked.diagnostics
    );
    assert_eq!(
        const_value(&little, "VALUE"),
        ConstValue::Int(IntConst::unsigned(13124))
    );

    let mut big_target = nia_target_config::TargetConfig::host();
    big_target.endian = "big".to_string();
    let big = check_source_for_target(source, big_target);
    assert!(
        big.checked.diagnostics.is_empty(),
        "{:?}",
        big.checked.diagnostics
    );
    assert_eq!(
        const_value(&big, "VALUE"),
        ConstValue::Int(IntConst::unsigned(4386))
    );
}

#[test]
fn scalar_array_union_reinterpretation_uses_artifact_endianness() {
    let source = r#"
const WIDTH: usize = 4;

union Bytes {
    word: u32,
    bytes: [WIDTH]u8,
}

const DATA: Bytes = { word: 287454020 };
const VALUE: [WIDTH]u8 = DATA.bytes;
"#;
    let mut little_target = nia_target_config::TargetConfig::host();
    little_target.endian = "little".to_string();
    let little = check_source_for_target(source, little_target);
    assert!(
        little.checked.diagnostics.is_empty(),
        "{:?}",
        little.checked.diagnostics
    );
    assert_eq!(
        const_value(&little, "VALUE"),
        ConstValue::Array(
            [68, 51, 34, 17]
                .map(|value| ConstValue::Int(IntConst::unsigned(value)))
                .into()
        )
    );

    let mut big_target = nia_target_config::TargetConfig::host();
    big_target.endian = "big".to_string();
    let big = check_source_for_target(source, big_target);
    assert!(
        big.checked.diagnostics.is_empty(),
        "{:?}",
        big.checked.diagnostics
    );
    assert_eq!(
        const_value(&big, "VALUE"),
        ConstValue::Array(
            [17, 34, 51, 68]
                .map(|value| ConstValue::Int(IntConst::unsigned(value)))
                .into()
        )
    );
}

#[test]
fn nested_scalar_array_union_fields_preserve_element_layout() {
    let mut target = nia_target_config::TargetConfig::host();
    target.endian = "little".to_string();
    let fixture = check_source_for_target(
        r#"
union MatrixBytes {
    matrix: [2][2]u16,
    bytes: [8]u8,
}

const DATA: MatrixBytes = { matrix: [[4386, 13124], [21862, 30600]] };
const BYTES: [8]u8 = DATA.bytes;
"#,
        target,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "BYTES"),
        ConstValue::Array(
            [34, 17, 68, 51, 102, 85, 136, 119]
                .map(|value| ConstValue::Int(IntConst::unsigned(value)))
                .into()
        )
    );
}

#[test]
fn scalar_array_union_layout_builtin_lengths_use_artifact_pointer_width() {
    let mut target = nia_target_config::TargetConfig::host();
    target.pointer_width = 32;
    target.endian = "little".to_string();
    let fixture = check_source_for_target(
        r#"
union WordBytes {
    word: usize,
    bytes: [std::builtin::size[usize]()]u8,
}

const DATA: WordBytes = { word: 16909060 };
const BYTES: [std::builtin::size[usize]()]u8 = DATA.bytes;
"#,
        target,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "BYTES"),
        ConstValue::Array(
            [4, 3, 2, 1]
                .map(|value| ConstValue::Int(IntConst::unsigned(value)))
                .into()
        )
    );
}

#[test]
fn scalar_array_union_writes_round_trip_and_validate_elements() {
    let mut target = nia_target_config::TargetConfig::host();
    target.endian = "little".to_string();
    let fixture = check_source_for_target(
        r#"
union Bytes {
    word: u32,
    bytes: [4]u8,
}

union Flags {
    raw: [2]u8,
    values: [2]bool,
}

const DATA: Bytes = { bytes: [4]u8[4, 3, 2, 1] };
const WORD: u32 = DATA.word;
const INVALID: Flags = { raw: [2]u8[0, 2] };
const VALUES: [2]bool = INVALID.values;
"#,
        target,
    );
    assert_eq!(
        const_value(&fixture, "WORD"),
        ConstValue::Int(IntConst::unsigned(16909060))
    );
    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("const union field has an invalid bool representation")),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn generic_scalar_array_union_is_encoded_after_element_substitution() {
    let mut target = nia_target_config::TargetConfig::host();
    target.endian = "little".to_string();
    let fixture = check_source_for_target(
        r#"
union PairBytes[T] {
    values: [2]T,
    bytes: [8]u8,
}

const fn encode[T](values: [2]T) [8]u8 {
    let pair: PairBytes[T] = { values: values };
    pair.bytes
}

const BYTES: [8]u8 = encode[u32]([2]u32[287454020, 1432778632]);
"#,
        target,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "BYTES"),
        ConstValue::Array(
            [68, 51, 34, 17, 136, 119, 102, 85]
                .map(|value| ConstValue::Int(IntConst::unsigned(value)))
                .into()
        )
    );
}

#[test]
fn nominal_struct_union_fields_preserve_padding_initialization() {
    let mut target = nia_target_config::TargetConfig::host();
    target.endian = "little".to_string();
    let fixture = check_source_for_target(
        r#"
struct Padded {
    marker: u8,
    word: u32,
}

union PaddedBytes {
    value: Padded,
    prefix: [5]u8,
    bytes: [8]u8,
}

const DATA: PaddedBytes = { value: { marker: 170, word: 287454020 } };
const ROUND_TRIP: Padded = DATA.value;
const PREFIX: [5]u8 = DATA.prefix;
const INVALID_PADDING_READ: [8]u8 = DATA.bytes;
"#,
        target,
    );
    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic
                .summary
                .contains("const union field reads uninitialized storage")),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "PREFIX"),
        ConstValue::Array(
            [68, 51, 34, 17, 170]
                .map(|value| ConstValue::Int(IntConst::unsigned(value)))
                .into()
        )
    );
    assert!(matches!(
        const_value(&fixture, "ROUND_TRIP"),
        ConstValue::Struct(_)
    ));
    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("const union field reads uninitialized storage")),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn generic_nominal_struct_union_fields_use_substituted_layout_and_validate_fields() {
    let mut target = nia_target_config::TargetConfig::host();
    target.endian = "little".to_string();
    let fixture = check_source_for_target(
        r#"
struct Pair[T] {
    marker: u8,
    value: T,
}

union PairBytes[T] {
    value: Pair[T],
    prefix: [5]u8,
}

struct Flags {
    first: bool,
    second: bool,
}

union FlagBytes {
    raw: [2]u8,
    flags: Flags,
}

const fn encode[T](value: Pair[T]) [5]u8 {
    let slot: PairBytes[T] = { value: value };
    slot.prefix
}

const BYTES: [5]u8 = encode[u32]({ marker: 170, value: 287454020 });
const INVALID: FlagBytes = { raw: [2]u8[0, 2] };
const FLAGS: Flags = INVALID.flags;
"#,
        target,
    );
    assert_eq!(
        const_value(&fixture, "BYTES"),
        ConstValue::Array(
            [68, 51, 34, 17, 170]
                .map(|value| ConstValue::Int(IntConst::unsigned(value)))
                .into()
        )
    );
    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("const union field has an invalid bool representation")),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn const_generic_nominal_struct_union_fields_use_concrete_array_layout() {
    let mut target = nia_target_config::TargetConfig::host();
    target.endian = "little".to_string();
    let fixture = check_source_for_target(
        r#"
struct Packet[T, N: usize, U] {
    marker: T,
    values: [N]U,
}

struct Flagged[Enabled: bool] {
    value: u8,
}

union PacketBytes[T, N: usize, U] {
    value: Packet[T, N, U],
    prefix: [5]u8,
    all: [6]u8,
}

const WIDTH: usize = 2;
const ENABLED: bool = true;
const DATA: PacketBytes[u8, WIDTH, u16] = {
    value: { marker: 170, values: [2]u16[4386, 13124] },
};
const PREFIX: [5]u8 = DATA.prefix;
const ROUND_TRIP: Packet[u8, 2, u16] = DATA.value;
const FLAGGED: Flagged[ENABLED] = { value: 7 };
const FLAGGED_LITERAL: Flagged[true] = FLAGGED;
const INVALID_PADDING_READ: [6]u8 = DATA.all;
"#,
        target,
    );
    assert!(
        fixture.const_module.diagnostics.is_empty(),
        "{:?}",
        fixture.const_module.diagnostics
    );
    assert!(
        fixture
            .checked
            .array_lengths
            .values()
            .any(|value| *value == 2),
        "{:?}",
        fixture.checked.array_lengths
    );
    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic
                .summary
                .contains("const union field reads uninitialized storage")),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "PREFIX"),
        ConstValue::Array(
            [0x22, 0x11, 0x44, 0x33, 0xaa]
                .map(|value| ConstValue::Int(IntConst::unsigned(value)))
                .into()
        )
    );
    assert!(matches!(
        const_value(&fixture, "ROUND_TRIP"),
        ConstValue::Struct(_)
    ));
    assert!(matches!(
        const_value(&fixture, "FLAGGED_LITERAL"),
        ConstValue::Struct(_)
    ));
    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("const union field reads uninitialized storage")),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn scalar_union_rejects_reads_of_uninitialized_storage() {
    let fixture = check_source(
        r#"
union Partial {
    narrow: u16,
    wide: u32,
}

const BITS: Partial = { narrow: 1 };
const INVALID: u32 = BITS.wide;
"#,
    );
    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("const union field reads uninitialized storage")),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn evaluates_generic_scalar_union_after_substitution() {
    let fixture = check_source(
        r#"
union Slot[T] {
    value: T,
    bits: u32,
}

const fn reinterpret[T](value: T) u32 {
    let slot: Slot[T] = { value: value };
    slot.bits
}

const BITS: u32 = reinterpret[u32](1065353216);
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "BITS"),
        ConstValue::Int(IntConst::unsigned(1065353216))
    );
}

#[test]
fn evaluates_scalar_union_return_literal_with_result_context() {
    let fixture = check_source(
        r#"
union Bits {
    integer: u32,
    float: f32,
}

const fn makeBits() Bits {
    { float: 1.0 }
}

const BITS: Bits = makeBits();
const VALUE: u32 = BITS.integer;
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "VALUE"),
        ConstValue::Int(IntConst::unsigned(1065353216))
    );
}

#[test]
fn evaluates_scalar_union_literals_in_call_and_assignment_contexts() {
    let fixture = check_source(
        r#"
union Bits {
    integer: u32,
    float: f32,
}

const fn readBits(bits: Bits) u32 {
    bits.integer
}

const fn replaceBits() u32 {
    let mut bits: Bits = { integer: 0 };
    bits = { float: 1.0 };
    bits.integer
}

const CALL_BITS: u32 = readBits({ float: 1.0 });
const ASSIGN_BITS: u32 = replaceBits();
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    for name in ["CALL_BITS", "ASSIGN_BITS"] {
        assert_eq!(
            const_value(&fixture, name),
            ConstValue::Int(IntConst::unsigned(1065353216))
        );
    }
}

#[test]
fn scalar_union_rejects_invalid_scalar_representations() {
    let fixture = check_source(
        r#"
union Tiny {
    value: u8,
    other: u8,
}

union BoolBits {
    raw: u8,
    flag: bool,
}

const OUT_OF_RANGE: Tiny = { value: 256 };
const BOOL_BITS: BoolBits = { raw: 2 };
const INVALID_BOOL: bool = BOOL_BITS.flag;
"#,
    );
    for message in [
        "const union integer field value is out of range",
        "const union field has an invalid bool representation",
    ] {
        assert!(
            fixture
                .checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains(message)),
            "missing {message:?}: {:?}",
            fixture.checked.diagnostics
        );
    }
}

fn const_value(fixture: &CheckedFixture, name: &str) -> ConstValue {
    let def_id = fixture
        .defs
        .module_scope
        .values
        .get(&sym(name))
        .expect("const def");
    fixture
        .checked
        .values
        .get(&ConstKey::Global(GlobalDefId {
            module_id: fixture.module_id,
            def_id,
        }))
        .expect("const value")
        .clone()
}
