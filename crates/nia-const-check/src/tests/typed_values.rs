use super::*;
use nia_const_eval::ConstValue;
use nia_ty::IntConst;

#[test]
fn cyclic_const_dependencies_do_not_poison_independent_values() {
    let fixture = check_source(
        r#"
const first: i32 = second;
const second: i32 = first;
const independent: i32 = 40 + 2;
"#,
    );

    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary == "cyclic const dependency"),
        "{:?}",
        fixture.checked.diagnostics
    );
    let global_key = |name| {
        ConstKey::Global(GlobalDefId {
            module_id: fixture.module_id,
            def_id: fixture
                .defs
                .module_scope
                .values
                .get(&sym(name))
                .unwrap_or_else(|| panic!("missing const `{name}`")),
        })
    };
    assert!(!fixture.checked.values.contains_key(&global_key("first")));
    assert!(!fixture.checked.values.contains_key(&global_key("second")));
    assert_eq!(
        fixture.checked.values.get(&global_key("independent")),
        Some(&ConstValue::Int(IntConst::signed(42)))
    );
    assert!(
        fixture
            .checked
            .typed_values
            .contains_key(&global_key("independent")),
        "independent const must retain its inferred type after cycle recovery"
    );
}

#[test]
fn const_switches_share_matrix_exhaustiveness_and_usefulness_rules() {
    let accepted = check_source(
        r#"
const RESULT: i32 = match true {
        true => 1,
        false => 0,
};
"#,
    );
    assert!(
        accepted.const_module.diagnostics.is_empty(),
        "{:?}",
        accepted.const_module.diagnostics
    );
    assert!(
        accepted.checked.diagnostics.is_empty(),
        "{:?}",
        accepted.checked.diagnostics
    );

    let rejected = check_source(
        r#"
const A: i32 = match true {
        true => 1,
};

const B: i32 = match false {
        _ => 0,
        false => 1,
};
"#,
    );
    assert!(
        rejected.checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("non-exhaustive const matched, missing pattern: `false`")
        }),
        "{:?}",
        rejected.checked.diagnostics
    );
    assert!(
        rejected.checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("const match pattern is unreachable")
        }),
        "{:?}",
        rejected.checked.diagnostics
    );
}

#[test]
fn records_explicit_types_for_const_bindings() {
    let fixture = check_source(
        r#"
const width: usize = 4;

fn main() i32 {
const local_width: usize = width;
static xs: [i32; local_width] = [1, 2, 3, 4];
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
fn preserves_float_suffixes_after_fraction_and_exponent() {
    let fixture = check_source(
        r#"
const fraction = 1.0f32;
const exponent = 1e3f32;
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    let f32_ty = fixture
        .type_store
        .append_for_module(fixture.module_id)
        .intern(TyKind::Primitive(PrimitiveTy::F32));
    for name in ["fraction", "exponent"] {
        let def_id = fixture
            .defs
            .module_scope
            .values
            .get(&sym(name))
            .unwrap_or_else(|| panic!("missing const `{name}`"));
        let typed = fixture
            .checked
            .typed_values
            .get(&ConstKey::Global(GlobalDefId {
                module_id: fixture.module_id,
                def_id,
            }))
            .unwrap_or_else(|| panic!("missing typed const `{name}`"));
        assert_eq!(typed.ty, ConstValueType::Runtime(f32_ty), "{name}");
    }
}

#[test]
fn reference_expected_type_reaches_nested_const_array_elements() {
    let fixture = check_source(
        r#"
union Item {
    pointer: &usize,
    integer: usize,
}

union Slot {
    pointer: &[Item; 2],
    integer: usize,
}

const ITEMS: Slot = Slot {
    pointer: &[
        Item { pointer: &11usize },
        Item { pointer: &13usize },
    ],
};
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
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
fn evaluates_const_generic_field_offset_after_instantiation() {
    let fixture = check_source(
        r#"
struct Packet[N: usize] {
    marker: u8,
    values: [u32; N],
}

const fn marker_offset[N: usize]() usize {
    std::builtin::offset[Packet[N]]("marker")
}

const OFF: usize = marker_offset[3]();
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
        const_value(&fixture, "OFF"),
        ConstValue::Int(IntConst::unsigned(12))
    );
}

#[test]
fn substitutes_const_generic_function_return_array_length() {
    let fixture = check_source(
        r#"
const fn identity[N: usize](value: [u8; N]) [u8; N] {
    value
}

const RESULT: [u8; 3] = identity[3]([7, 7, 7]);
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "RESULT"),
        ConstValue::Array(vec![
            ConstValue::Int(IntConst::unsigned(7)),
            ConstValue::Int(IntConst::unsigned(7)),
            ConstValue::Int(IntConst::unsigned(7)),
        ])
    );
}

#[test]
fn const_generic_integer_rejects_unsupported_target_pointer_width() {
    let mut target = nia_target_config::TargetConfig::host();
    target.pointer_width = 256;
    let fixture = check_source_for_target(
        r#"
const fn identity[N: usize]() usize {
    N
}

const VALUE: usize = identity[1]();
"#,
        target,
    );
    assert!(
        fixture.checked.diagnostics.iter().any(|diagnostic| {
            diagnostic.summary
                == "const generic integer argument requires a supported target pointer width"
        }),
        "{:?}",
        fixture.checked.diagnostics
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
fn enum_discriminants_use_the_artifact_pointer_width() {
    let mut target = nia_target_config::TargetConfig::host();
    target.pointer_width = 32;
    let fixture = check_source_for_target(
        r#"
enum Word: usize {
    Largest = 4294967295,
    TooLarge,
}
"#,
        target,
    );

    assert_eq!(
        fixture
            .checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("out of range for backing type"))
            .count(),
        1,
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
fn pointer_union_relocations_follow_artifact_pointer_width() {
    for pointer_width in [32, 64] {
        let mut target = nia_target_config::TargetConfig::host();
        target.pointer_width = pointer_width;
        let fixture = check_source_for_target(
            r#"
union Slot {
    pointer: &usize,
    integer: usize,
}

const fn roundTrip() usize {
    let value: usize = 21;
    let slot = Slot { pointer: &value };
    slot.pointer.*
}

const RESULT: usize = roundTrip();
"#,
            target,
        );
        assert!(
            fixture.const_module.diagnostics.is_empty(),
            "pointer width {pointer_width}: {:?}",
            fixture.const_module.diagnostics
        );
        assert!(
            fixture.checked.diagnostics.is_empty(),
            "pointer width {pointer_width}: {:?}",
            fixture.checked.diagnostics
        );
        assert!(matches!(
            const_value(&fixture, "RESULT"),
            ConstValue::Int(value) if value.bits() == 21
        ));
    }
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
    let bits = Bits { float: 1.0 };
    bits.integer
}

const fn switchedBits() f32 {
    let mut bits = Bits { float: 0.0 };
    bits.integer = 1065353216;
    bits.float
}

const FLOAT_BITS: u32 = floatBits();
const SWITCHED: f32 = switchedBits();
const SIGNED_BITS: SignedBits = SignedBits { unsigned: 4294967295 };
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

const BITS: Narrow = Narrow { wide: 287454020 };
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
    bytes: [u8; WIDTH],
}

const DATA: Bytes = Bytes { word: 287454020 };
const VALUE: [u8; WIDTH] = DATA.bytes;
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
    matrix: [[u16; 2]; 2],
    bytes: [u8; 8],
}

const DATA: MatrixBytes = MatrixBytes { matrix: [[4386, 13124], [21862, 30600]] };
const BYTES: [u8; 8] = DATA.bytes;
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
    bytes: [u8; std::builtin::size[usize]()],
}

const DATA: WordBytes = WordBytes { word: 16909060 };
const BYTES: [u8; std::builtin::size[usize]()] = DATA.bytes;
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
    bytes: [u8; 4],
}

union Flags {
    raw: [u8; 2],
    values: [bool; 2],
}

const DATA: Bytes = Bytes { bytes: [4, 3, 2, 1] };
const WORD: u32 = DATA.word;
const INVALID: Flags = Flags { raw: [0, 2] };
const VALUES: [bool; 2] = INVALID.values;
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
    values: [T; 2],
    bytes: [u8; 8],
}

const fn encode[T](values: [T; 2]) [u8; 8] {
    let pair = PairBytes[T] { values: values };
    pair.bytes
}

const BYTES: [u8; 8] = encode[u32]([287454020, 1432778632]);
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
    prefix: [u8; 5],
    bytes: [u8; 8],
}

const DATA: PaddedBytes = PaddedBytes { value: Padded { marker: 170, word: 287454020 } };
const ROUND_TRIP: Padded = DATA.value;
const PREFIX: [u8; 5] = DATA.prefix;
const INVALID_PADDING_READ: [u8; 8] = DATA.bytes;
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
    prefix: [u8; 5],
}

struct Flags {
    first: bool,
    second: bool,
}

union FlagBytes {
    raw: [u8; 2],
    flags: Flags,
}

const fn encode[T](value: Pair[T]) [u8; 5] {
    let slot = PairBytes[T] { value: value };
    slot.prefix
}

const BYTES: [u8; 5] = encode[u32](Pair[u32] { marker: 170, value: 287454020 });
const INVALID: FlagBytes = FlagBytes { raw: [0, 2] };
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
    values: [U; N],
}

struct Flagged[Enabled: bool] {
    value: u8,
}

union PacketBytes[T, N: usize, U] {
    value: Packet[T, N, U],
    prefix: [u8; 5],
    all: [u8; 6],
}

const WIDTH: usize = 2;
const ENABLED: bool = true;
const DATA: PacketBytes[u8, WIDTH, u16] = PacketBytes[u8, WIDTH, u16] {
    value: Packet[u8, WIDTH, u16] { marker: 170, values: [4386, 13124] },
};
const PREFIX: [u8; 5] = DATA.prefix;
const ROUND_TRIP: Packet[u8, 2, u16] = DATA.value;
const FLAGGED: Flagged[ENABLED] = Flagged[ENABLED] { value: 7 };
const FLAGGED_LITERAL: Flagged[true] = FLAGGED;
const INVALID_PADDING_READ: [u8; 6] = DATA.all;
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

const BITS: Partial = Partial { narrow: 1 };
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
    let slot = Slot[T] { value: value };
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
fn const_generic_inference_accepts_mutable_pointer_for_readonly_parameter() {
    let fixture = check_source(
        r#"
const fn read[T](value: &T) usize {
    let _unused = value;
    7
}

const fn run() usize {
    let mut value: i32 = 3;
    read(&mut value)
}

const RESULT: usize = run();
"#,
    );

    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "RESULT"),
        ConstValue::Int(IntConst::unsigned(7))
    );
}

#[test]
fn const_generic_inference_rejects_unrelated_trait_object_evidence() {
    let fixture = check_source(
        r#"
trait Left[T] {}
trait Right[T] {}

struct Marker {}

extend Marker : Right[i32] {}

const fn inspect[T](value: &Left[T]) usize {
    let _unused = value;
    7
}

const fn run() usize {
    let marker = Marker {};
    let object: &Right[i32] = &marker;
    inspect(object)
}

const RESULT: usize = run();
"#,
    );

    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("cannot infer const generic type argument `T`")),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn const_execution_inference_tries_later_associated_binding_candidates() {
    let fixture = check_source(
        r#"
trait Slot[T] {
    type Item;
}

trait Both : Slot[i32] + Slot[bool] {}

struct Store {}

extend Store : Slot[i32] {
    type Item = bool;
}

extend Store : Slot[bool] {
    type Item = i32;
}

extend Store : Both {}

const fn inspect[T, U](expected: U, value: &Both[
    [Self as Slot[T]]::Item = U,
    [Self as Slot[i32]]::Item = bool,
]) usize {
    let _ = (expected, value);
    7
}

const fn run() usize {
    let store = Store {};
    let value: &Both[
        [Self as Slot[i32]]::Item = bool,
        [Self as Slot[bool]]::Item = i32,
    ] = &store;
    inspect(0i32, value)
}

const RESULT: usize = run();
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "RESULT"),
        ConstValue::Int(IntConst::unsigned(7))
    );
}

#[test]
fn const_execution_inference_does_not_reuse_associated_binding_candidates() {
    let fixture = check_source(
        r#"
trait Slot[T] {
    type Item;
}

trait Both : Slot[i32] + Slot[bool] {}

struct Store {}

extend Store : Slot[i32] {
    type Item = bool;
}

extend Store : Slot[bool] {
    type Item = char;
}

extend Store : Both {}

const fn inspect[T, U](expected: U, value: &Both[
    [Self as Slot[T]]::Item = U,
    [Self as Slot[i32]]::Item = bool,
]) usize {
    let _ = (expected, value);
    7
}

const fn run() usize {
    let store = Store {};
    let value: &Both[
        [Self as Slot[i32]]::Item = bool,
        [Self as Slot[bool]]::Item = char,
    ] = &store;
    inspect(0i32, value)
}

const RESULT: usize = run();
"#,
    );
    assert!(
        !fixture.checked.diagnostics.is_empty(),
        "inference must reject a pattern that can only match by reusing one actual binding"
    );
}

#[test]
fn const_generic_inference_rejects_evidence_below_mismatched_const_argument() {
    let fixture = check_source(
        r#"
struct Packet[T, N: usize] {
    value: T,
}

const fn inspect[T](packet: Packet[T, 3]) usize {
    let _unused = packet;
    7
}

const RESULT: usize = inspect(Packet[i32, 4] { value: 1 });
"#,
    );

    assert!(
        fixture
            .checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("cannot infer const generic type argument `T`")),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn const_generic_inference_rejects_evidence_below_mismatched_array_length() {
    let fixture = check_source(
        r#"
const fn inspect[N: usize](value: [[u8; N]; 2]) usize {
    let _ = value;
    7
}

const VALUE: [[u8; 3]; 3] = [[1u8, 2u8, 3u8]; 3];
const RESULT: usize = inspect(VALUE);
"#,
    );

    assert!(
        fixture.checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("cannot infer const generic argument `N`")
        }),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn const_generic_inference_rejects_evidence_below_mismatched_layout_operand() {
    let fixture = check_source(
        r#"
struct Packet[T] {
    value: T,
}

const fn inspect[T](value: [Packet[T]; std::builtin::size[i32]()]) usize {
    let _ = value;
    7
}

const VALUE: [Packet[bool]; std::builtin::size[bool]()] = [Packet[bool] { value: true }];
const RESULT: usize = inspect(VALUE);
"#,
    );

    assert!(
        fixture.checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("cannot infer const generic type argument `T`")
        }),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn const_generic_inference_rejects_evidence_below_mismatched_layout_constructor() {
    let fixture = check_source(
        r#"
struct Box[T] {
    value: T,
}

struct Other[T] {
    value: T,
}

struct Packet[T] {
    value: T,
}

const fn inspect[T, U](
    value: [Packet[U]; std::builtin::size[Box[T]]()],
    marker: T,
) usize {
    let _ = (value, marker);
    7
}

const VALUE: [Packet[bool]; std::builtin::size[Other[bool]]()] = [Packet[bool] { value: true }];
const RESULT: usize = inspect(VALUE, 1i32);
"#,
    );

    assert!(
        fixture.checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("cannot infer const generic type argument `U`")
        }),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn const_generic_inference_rejects_evidence_below_nested_layout_constructor() {
    let fixture = check_source(
        r#"
struct Box[T] {
    value: T,
}

struct Other[T] {
    value: T,
}

struct Packet[T] {
    value: T,
}

const fn inspect[T, U](
    value: [Packet[U]; std::builtin::size[Box[Box[T]]]()],
    marker: T,
) usize {
    let _ = (value, marker);
    7
}

const VALUE: [Packet[bool]; std::builtin::size[Box[Other[bool]]]()] =
    [Packet[bool] { value: true }];
const RESULT: usize = inspect(VALUE, 1i32);
"#,
    );

    assert!(
        fixture.checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("cannot infer const generic type argument `U`")
        }),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn const_generic_inference_through_layout_builtin_and_marker() {
    let fixture = check_source(
        r#"
const fn inspect[T](value: [u8; std::builtin::size[T]()], marker: T) usize {
    _ = (value, marker);
    7
}

const RESULT: usize = inspect([1u8, 2u8, 3u8, 4u8], 1i32);
"#,
    );
    assert!(
        fixture.checked.diagnostics.iter().all(|diagnostic| {
            !diagnostic
                .summary
                .contains("cannot infer const generic type argument `T`")
        }),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn const_generic_inference_through_layout_builtin_operand() {
    let fixture = check_source(
        r#"
const fn inspect[T](value: [u8; std::builtin::size[T]()]) usize {
    let _unused = value;
    7
}

const VALUE: [u8; std::builtin::size[i32]()] = [1u8, 2u8, 3u8, 4u8];
const RESULT: usize = inspect(VALUE);
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
}

#[test]
fn evaluates_nominal_scalar_union_return_literal() {
    let fixture = check_source(
        r#"
union Bits {
    integer: u32,
    float: f32,
}

const fn makeBits() Bits {
    Bits { float: 1.0 }
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
fn evaluates_nominal_scalar_union_literals_in_calls_and_assignments() {
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
    let mut bits = Bits { integer: 0 };
    bits = Bits { float: 1.0 };
    bits.integer
}

const CALL_BITS: u32 = readBits(Bits { float: 1.0 });
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

const OUT_OF_RANGE: Tiny = Tiny { value: 256 };
const BOOL_BITS: BoolBits = BoolBits { raw: 2 };
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

#[test]
fn vector_union_fields_follow_artifact_endianness() {
    for (endian, expected) in [
        ("little", [0x22, 0x11, 0x44, 0x33]),
        ("big", [0x11, 0x22, 0x33, 0x44]),
    ] {
        let mut target = nia_target_config::TargetConfig::host();
        target.endian = endian.to_string();
        let fixture = check_source_for_target(
            r#"
@[builtin("splat")]
const fn splat[V](value: u16) V;

@[builtin("insert")]
const fn insert[V](vector: V, index: usize, value: u16) V;

union VectorBytes {
    vector: u16x2,
    bytes: [u8; 4],
}

const VECTOR: u16x2 = insert[u16x2](splat[u16x2](4386), 1, 13124);
const STORAGE: VectorBytes = VectorBytes { vector: VECTOR };
const BYTES: [u8; 4] = STORAGE.bytes;
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
                expected
                    .map(|value| ConstValue::Int(IntConst::unsigned(value as u128)))
                    .into()
            )
        );
    }
}

#[test]
fn usize_vector_union_fields_follow_artifact_pointer_width() {
    for (pointer_width, bytes_len) in [(32, 8), (64, 16)] {
        let mut target = nia_target_config::TargetConfig::host();
        target.pointer_width = pointer_width;
        target.endian = "little".to_string();
        let source = format!(
            r#"
@[builtin("splat")]
const fn splat[V](value: usize) V;

@[builtin("insert")]
const fn insert[V](vector: V, index: usize, value: usize) V;

union VectorBytes {{
    vector: usizex2,
    bytes: [u8; {bytes_len}],
}}

const VECTOR: usizex2 = insert[usizex2](splat[usizex2](1), 1, 2);
const STORAGE: VectorBytes = VectorBytes {{ vector: VECTOR }};
const BYTES: [u8; {bytes_len}] = STORAGE.bytes;
"#,
        );
        let fixture = check_source_for_target(&source, target);
        assert!(
            fixture.checked.diagnostics.is_empty(),
            "{:?}",
            fixture.checked.diagnostics
        );
        let ConstValue::Array(bytes) = const_value(&fixture, "BYTES") else {
            panic!("usize vector bytes must be an array");
        };
        assert_eq!(bytes.len(), bytes_len);
        assert_eq!(bytes[0], ConstValue::Int(IntConst::unsigned(1)));
        assert_eq!(
            bytes[pointer_width as usize / 8],
            ConstValue::Int(IntConst::unsigned(2))
        );
    }
}

#[test]
fn float_vector_union_fields_preserve_lane_bits() {
    let fixture = check_source(
        r#"
@[builtin("splat")]
const fn splat[V](value: f32) V;

@[builtin("insert")]
const fn insert[V](vector: V, index: usize, value: f32) V;

union VectorWords {
    vector: f32x2,
    words: [u32; 2],
}

const VECTOR: f32x2 = insert[f32x2](splat[f32x2](1.0), 1, -2.5);
const STORAGE: VectorWords = VectorWords { vector: VECTOR };
const WORDS: [u32; 2] = STORAGE.words;
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "WORDS"),
        ConstValue::Array(vec![
            ConstValue::Int(IntConst::unsigned(0x3f80_0000)),
            ConstValue::Int(IntConst::unsigned(0xc020_0000)),
        ])
    );
}

#[test]
fn const_execution_const_inference_matches_associated_bindings_by_trait_args() {
    let fixture = check_source(
        r#"
trait Slot[T] {
    type Item;
}

trait Both[A, B] : Slot[A] + Slot[B] {}

struct Store {}

extend Store : Slot[i32] {
    type Item = [u8; 2];
}

extend Store : Slot[bool] {
    type Item = [u8; 4];
}

extend Store : Both[i32, bool] {}

const fn inspect[A, B, N: usize, M: usize](value: &Both[A, B,
    [Self as Slot[A]]::Item = [u8; N],
    [Self as Slot[B]]::Item = [u8; M],
]) [u8; N] {
    let _ = value;
    [7, 7]
}

const fn run() [u8; 2] {
    let store = Store {};
    let value: &Both[i32, bool,
        [Self as Slot[bool]]::Item = [u8; 4],
        [Self as Slot[i32]]::Item = [u8; 2],
    ] = &store;
    inspect(value)
}

const RESULT: [u8; 2] = run();
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "RESULT"),
        ConstValue::Array(vec![
            ConstValue::Int(IntConst::unsigned(7)),
            ConstValue::Int(IntConst::unsigned(7)),
        ])
    );
}

#[test]
fn const_execution_infers_array_length_nested_in_tuple() {
    let fixture = check_source(
        r#"
const fn inspect[N: usize](value: ([u8; N], bool)) usize {
    let _ = value;
    7
}

const RESULT: usize = inspect(([1u8, 2u8], true));
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "RESULT"),
        ConstValue::Int(IntConst::unsigned(7))
    );
}

#[test]
fn const_execution_inference_preserves_generic_array_lengths_in_associated_bindings() {
    let fixture = check_source(
        r#"
trait Slot {
    type Item;
}

struct Store {}

extend Store : Slot {
    type Item = [u8; 4];
}

const fn inspect[N: usize](value: &Slot[Item = [u8; N]]) usize {
    let _ = value;
    7
}

const fn run() usize {
    let store = Store {};
    let value: &Slot[Item = [u8; 4]] = &store;
    inspect(value)
}

const RESULT: usize = run();
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(
        const_value(&fixture, "RESULT"),
        ConstValue::Int(IntConst::unsigned(7))
    );
}

#[test]
fn const_inference_does_not_commit_prefix_of_mismatched_associated_binding() {
    let fixture = check_source(
        r#"
trait Slot {
    type Item;
}

struct Store {}
struct Box[T] {
    value: T,
}
struct Other[T] {
    value: T,
}
struct Pair[A, B] {
    first: A,
    second: B,
}

extend Store : Slot {
    type Item = Pair[[u8; 4], Other[bool]];
}

const fn inspect[T, N: usize, M: usize](
    value: &Slot[Item = Pair[[u8; N], Box[[u8; M]]]],
    marker: T,
) [u8; N] {
    let _ = (value, marker);
    [0u8, 0u8, 0u8, 0u8]
}

const fn run() [u8; 4] {
    let store = Store {};
    let value: &Slot[Item = Pair[[u8; 4], Other[bool]]] = &store;
    inspect(value, 1i32)
}

const RESULT: [u8; 4] = run();
"#,
    );

    assert!(
        fixture.checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("cannot infer const generic argument `N`")
        }),
        "{:?}",
        fixture.checked.diagnostics
    );
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
