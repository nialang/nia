// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn public_const_values_are_visible_through_import_closure() {
    let root = temp_dir("public_const_values_are_visible_through_import_closure");
    write(
        &root.join("main.nia"),
        r#"
module facade;
module defs;
using entry::facade;

fn main() i32 {
    facade::answer
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
using entry::defs;
pub using defs::answer;
"#,
    );
    write(
        &root.join("defs.nia"),
        r#"
pub const answer: i32 = 42;
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main_module = program
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module");
    assert!(main_module.globals.is_empty());
}

#[test]
fn const_values_drive_static_global_integer_initializers() {
    let root = temp_dir("const_values_drive_static_global_integer_initializers");
    write(
        &root.join("main.nia"),
        r#"
const base = 20;
static mut value: i32 = base + 2;
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main_module = program
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module");
    let value = main_module
        .globals
        .iter()
        .find(|global| global.name == sym("value"))
        .expect("value global");
    assert_eq!(value.init, Some(static_int(22)));
}

#[test]
fn scalar_const_values_drive_static_initializers() {
    let root = temp_dir("scalar_const_values_drive_static_initializers");
    write(
        &root.join("main.nia"),
        r#"
module values;
using entry::values;

const localFloat: f32 = 1.5f32;
const localBool: bool = true;
const localChar: char = 'A';
const localByte: u8 = b'B';

static mut single: f32 = localFloat;
static mut enabled: bool = localBool;
static mut letter: char = localChar;
static mut byte: u8 = localByte;
static mut double: f64 = values::importedFloat;
static mut disabled: bool = values::importedBool;

fn nested() i32 {
    const local: f32 = 3.5f32;
    static mut value: f32 = local;
    value as i32
}
"#,
    );
    write(
        &root.join("values.nia"),
        r#"
pub const importedFloat: f64 = -2.25f64;
pub const importedBool: bool = false;
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main_module = program
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module");
    let global_init = |name| {
        main_module
            .globals
            .iter()
            .find(|global| global.name == sym(name))
            .unwrap_or_else(|| panic!("{name} global"))
            .init
            .clone()
    };
    assert_eq!(global_init("single"), Some(StaticInit::Float("1.5".into())));
    assert_eq!(global_init("enabled"), Some(StaticInit::Bool(true)));
    assert_eq!(
        global_init("letter"),
        Some(StaticInit::Int(IntConst::unsigned(65)))
    );
    assert_eq!(
        global_init("byte"),
        Some(StaticInit::Int(IntConst::unsigned(66)))
    );
    assert_eq!(
        global_init("double"),
        Some(StaticInit::Float("-2.25".into()))
    );
    assert_eq!(global_init("disabled"), Some(StaticInit::Bool(false)));
    assert_eq!(global_init("value"), Some(StaticInit::Float("3.5".into())));
}

#[test]
fn aggregate_const_values_drive_static_initializers() {
    let root = temp_dir("aggregate_const_values_drive_static_initializers");
    write(
        &root.join("main.nia"),
        r#"
module values;
using entry::values;

struct Config {
    bytes: [u8; 2],
    enabled: bool,
}

struct Packet[N: usize] { bytes: [u8; N] }

const numbers: [i32; 3] = [1, 2, 3];
const config: Config = Config { bytes: [4u8, 5u8], enabled: true };
const packet: Packet[2] = Packet[2] { bytes: [9u8, 10u8] };

static mut numberCopy: [i32; 3] = numbers;
static mut configCopy: Config = config;
static mut packetCopy: Packet[2] = packet;
static mut directPacket: Packet[2] = Packet[2] { bytes: [11u8, 12u8] };
static mut importedCopy: [f32; 2] = values::imported;
static mut importedConfigCopy: values::ImportedConfig = values::importedConfig;

fn nested() i32 {
    const local: [i32; 2] = [6, 7];
    static mut copy: [i32; 2] = local;
    copy[0] + copy[1]
}
"#,
    );
    write(
        &root.join("values.nia"),
        r#"
pub const imported: [f32; 2] = [1.5f32, -2.25f32];
pub struct ImportedConfig { count: i32, active: bool }
pub const importedConfig: ImportedConfig = ImportedConfig { count: 8, active: false };
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main_module = program
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module");
    let global_init = |name| {
        main_module
            .globals
            .iter()
            .find(|global| global.name == sym(name))
            .unwrap_or_else(|| panic!("{name} global"))
            .init
            .clone()
    };
    assert_eq!(
        global_init("numberCopy"),
        Some(StaticInit::Array(vec![
            static_int(1),
            static_int(2),
            static_int(3),
        ]))
    );
    let Some(StaticInit::Struct(fields)) = global_init("configCopy") else {
        panic!("configCopy struct initializer");
    };
    assert_eq!(fields.len(), 2);
    assert!(
        fields
            .iter()
            .any(|field| field.value == StaticInit::Bool(true))
    );
    assert!(fields.iter().any(|field| {
        field.value
            == StaticInit::Array(vec![
                StaticInit::Int(IntConst::unsigned(4)),
                StaticInit::Int(IntConst::unsigned(5)),
            ])
    }));
    for name in ["packetCopy", "directPacket"] {
        let Some(StaticInit::Struct(fields)) = global_init(name) else {
            panic!("{name} struct initializer");
        };
        assert!(matches!(
            fields.as_slice(),
            [field] if matches!(&field.value, StaticInit::Array(values) if values.len() == 2)
        ));
    }
    assert_eq!(
        global_init("importedCopy"),
        Some(StaticInit::Array(vec![
            StaticInit::Float("1.5".into()),
            StaticInit::Float("-2.25".into()),
        ]))
    );
    assert_eq!(
        global_init("copy"),
        Some(StaticInit::Array(vec![static_int(6), static_int(7)]))
    );
    let Some(StaticInit::Struct(fields)) = global_init("importedConfigCopy") else {
        panic!("importedConfigCopy struct initializer");
    };
    assert_eq!(fields.len(), 2);
    assert!(fields.iter().any(|field| field.value == static_int(8)));
    assert!(
        fields
            .iter()
            .any(|field| field.value == StaticInit::Bool(false))
    );
}

#[test]
fn const_values_drive_array_lengths() {
    let root = temp_dir("const_values_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
pub const width: usize = 2 + 2;

fn main() i32 {
    const local_width: usize = width;
    let mut values: [i32; local_width] = [1, 2, 3, 4];
    values[3]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_functions_drive_array_lengths() {
    let root = temp_dir("const_functions_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn width(base: usize) usize {
    let extra: usize = 2;
    return base + extra;
}

const n: usize = width(2);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main_module = program
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module");
    assert!(
        main_module
            .functions
            .iter()
            .all(|function| function.name != sym("width")),
        "{:?}",
        main_module.functions
    );
}

#[test]
fn const_function_if_expression_drives_array_lengths() {
    let root = temp_dir("const_function_if_expression_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn width(use_wide: bool) usize {
    if use_wide {
        let word: usize = 8;
        word
    } else {
        let word: usize = 4;
        word
    }
}

const bits: usize = 64usize;
const n: usize = width(bits == 64);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_if_branch_return_drives_array_lengths() {
    let root = temp_dir("const_function_if_branch_return_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn word_bytes(bits: usize) usize {
    if bits == 64 {
        return 8;
    } else {
        return 4;
    }
}

const bits: usize = 64usize;
const n: usize = word_bytes(bits);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_return_expression_propagates_nested_return() {
    let root = temp_dir("const_function_return_expression_propagates_nested_return");
    write(
        &root.join("main.nia"),
        r#"
const fn width(use_wide: bool) usize {
    return if use_wide {
        return 8;
    } else {
        4
    };
}

const n: usize = width(true);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_match_expression_drives_array_lengths() {
    let root = temp_dir("const_function_match_expression_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn word_bytes(bits: usize) usize {
    match bits {
        16 => 2,
        32 => 4,
        64 => 8,
        _ => 16,
    }
}

const bits: usize = 64usize;
const n: usize = word_bytes(bits);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_tuple_patterns_drive_array_lengths() {
    let root = temp_dir("const_tuple_patterns_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn select(value: (usize, (bool, usize))) usize {
    match value {
        (0, (false, _)) => 1,
        (left, (true, right)) => left + right,
        (_, (_, fallback)) => fallback,
    }
}

const n: usize = select((3usize, (true, 5usize)));

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_match_ranges_and_return_arms_drive_array_lengths() {
    let root = temp_dir("const_function_match_ranges_and_return_arms_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn bucket(value: usize) usize {
    match value {
        0..4 => return 4,
        4..8 => 8,
        _ => return 16,
    }
}

const n: usize = bucket(6);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_if_pattern_optional_payload_drives_array_lengths() {
    let root = temp_dir("const_function_if_pattern_optional_payload_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn unwrap(value: ?usize) usize {
    match value {
        ?payload => {
            payload
        },
        null => {
            1
        },
    }
}

const some: ?usize = ?8usize;
const n: usize = unwrap(some);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_if_pattern_error_payload_drives_array_lengths() {
    let root = temp_dir("const_function_if_pattern_error_payload_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn unwrap(value: usize!usize) usize {
    match value {
        !payload => {
            payload
        },
        err! => {
            err
        },
    }
}

const ok: usize!usize = !8;
const n: usize = unwrap(ok);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_if_uses_const_function_condition() {
    let root = temp_dir("function_body_if_uses_const_function_condition");
    write(
        &root.join("main.nia"),
        r#"
const fn is_native_word(bits: usize) bool {
    bits == 64usize
}

fn main() i32 {
    const native: bool = is_native_word(64usize);
    if native {
        1
    } else {
        0
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_const_functions_are_ordinary_const_values() {
    let root = temp_dir("imported_const_functions_are_ordinary_const_values");
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

const width: usize = config::width(2);

fn main() i32 {
    let mut values: [i32; width] = [0; width];
    values.len() as i32
}
"#,
    );
    write(
        &root.join("config.nia"),
        r#"
pub const fn width(base: usize) usize {
    base + 2
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_is_available_at_comptime_and_runtime() {
    let root = temp_dir("const_function_is_available_at_comptime_and_runtime");
    write(
        &root.join("main.nia"),
        r#"
const fn width(value: usize) usize {
    value * 2
}

const arrayLen: usize = width(5);

fn main(input: usize) i32 {
    let values: [u8; arrayLen] = [0; arrayLen];
    width(input) as i32 + values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_receiver_method_is_available_at_comptime_and_runtime() {
    let root = temp_dir("const_receiver_method_is_available_at_comptime_and_runtime");
    write(
        &root.join("main.nia"),
        r#"
struct Width {
    value: usize,
}

extend Width {
    const fn doubled(self) usize {
        self.value * 2
    }
}

const compileTimeWidth: usize = Width{value: 5}.doubled();

fn main(input: usize) i32 {
    let values: [u8; compileTimeWidth] = [0; compileTimeWidth];
    Width{value: input}.doubled() as i32 + values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_associated_function_is_available_at_comptime_and_runtime() {
    let root = temp_dir("const_associated_function_is_available_at_comptime_and_runtime");
    write(
        &root.join("main.nia"),
        r#"
struct Width {
    value: usize,
}

extend Width {
    const fn fromValue(value: usize) Width {
        Self { value }
    }
}

const compileTimeWidth: usize = Width::fromValue(5).value;

fn main(input: usize) i32 {
    let values: [u8; compileTimeWidth] = [0; compileTimeWidth];
    Width::fromValue(input).value as i32 + values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_methods_infer_extension_target_at_comptime_and_runtime() {
    let root = temp_dir("generic_const_methods_infer_extension_target_at_comptime_and_runtime");
    write(
        &root.join("main.nia"),
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    const fn value(self) T {
        self.value
    }

    const fn fromValue(value: T) Box[T] {
        Self { value }
    }
}

const receiverValue: usize = Box[usize]{value: 4}.value();
const associatedValue: usize = Box[usize]::fromValue(6).value();
const arrayLen: usize = receiverValue + associatedValue;

fn main(input: usize) i32 {
    let values: [u8; arrayLen] = [0; arrayLen];
    Box[usize]::fromValue(input).value() as i32 + values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_method_generics_preserve_extension_target_generics() {
    let root = temp_dir("const_method_generics_preserve_extension_target_generics");
    write(
        &root.join("main.nia"),
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    const fn convert[U](self, value: U) U {
        let _ = self;
        value
    }
}

const result: bool = Box[i32] { value: 1 }.convert[bool](true);

fn main() i32 {
    if result { 0 } else { 1 }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_calls_infer_underscore_generic_slots() {
    let root = temp_dir("const_calls_infer_underscore_generic_slots");
    write(
        &root.join("main.nia"),
        r#"
const fn choose[T, U](first: T, second: U) U {
    let _ = first;
    second
}

const result: bool = choose[i32, _](1, true);

const fn count[T, N: usize](values: [T; N]) usize {
    let _ = values;
    N
}

const length: usize = count[_, _]([1i32, 2i32, 3i32]);
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_method_generics_cannot_shadow_extension_target_generics() {
    let root = temp_dir("const_method_generics_cannot_shadow_extension_target_generics");
    write(
        &root.join("main.nia"),
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    const fn convert[T](self, value: T) T {
        let _ = self;
        value
    }
}

const result: bool = Box[i32] { value: 1 }.convert[bool](true);
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains(
                "method generic parameter cannot shadow enclosing generic parameter `T`"
            )),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trait_method_generics_cannot_shadow_trait_generics() {
    let root = temp_dir("trait_method_generics_cannot_shadow_trait_generics");
    write(
        &root.join("main.nia"),
        r#"
trait Carrier[T] {
    fn convert[T](self, value: T) T;
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains(
                "method generic parameter cannot shadow enclosing generic parameter `T`"
            )),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn imported_const_functions_and_methods_work_at_comptime_and_runtime() {
    let root = temp_dir("imported_const_functions_and_methods_work_at_comptime_and_runtime");
    write(
        &root.join("main.nia"),
        r#"
module widths;
using entry::widths;

const arrayLen: usize = widths::double(2) + widths::Width::fromValue(3).doubled();

fn main(input: usize) i32 {
    let values: [u8; arrayLen] = [0; arrayLen];
    widths::double(input) as i32
        + widths::Width::fromValue(input).doubled() as i32
        + values.len() as i32
}
"#,
    );
    write(
        &root.join("widths.nia"),
        r#"
pub struct Width {
    value: usize,
}

pub const fn double(value: usize) usize {
    value * 2
}

extend Width {
    pub const fn fromValue(value: usize) Width {
        Self { value }
    }

    pub const fn doubled(self) usize {
        self.value * 2
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_expression_rejects_runtime_only_function() {
    let root = temp_dir("const_expression_rejects_runtime_only_function");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeWidth(value: usize) usize {
    value * 2
}

const arrayLen: usize = runtimeWidth(5);
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const expression can only call `const fn`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_struct_values_drive_field_access() {
    let root = temp_dir("const_struct_values_drive_field_access");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

const p: Point = Point{x: 2, y: 3};
const width: usize = p.x + p.y;

fn main() i32 {
    let mut values: [i32; width] = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_local_const_struct_values_drive_field_access() {
    let root = temp_dir("function_local_const_struct_values_drive_field_access");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

fn main() i32 {
    const p: Point = Point{x: 4, y: 2};
    const width: usize = p.x + p.y;
    let mut values: [i32; width] = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_array_values_drive_index_access() {
    let root = temp_dir("const_array_values_drive_index_access");
    write(
        &root.join("main.nia"),
        r#"
const widths: [usize; 3] = [2, 4, 8];
const width: usize = widths[1];

fn main() i32 {
    let mut values: [i32; width] = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_local_nominal_const_array_index_drives_field_access() {
    let root = temp_dir("function_local_nominal_const_array_index_drives_field_access");
    write(
        &root.join("main.nia"),
        r#"
struct Config { width: usize }

fn main() i32 {
    const configs = [Config { width: 2usize }, Config { width: 4usize }];
    const width: usize = configs[1].width;
    let mut values: [i32; width] = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_struct_array_fields_are_ordinary_values() {
    let root = temp_dir("const_struct_array_fields_are_ordinary_values");
    write(
        &root.join("main.nia"),
        r#"
struct Config {
    widths: [usize; 3],
}

const config: Config = Config{widths: [2, 4, 8]};
const width: usize = config.widths[2];

fn main() i32 {
    let mut values: [i32; width] = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_functions_accept_array_values() {
    let root = temp_dir("const_functions_accept_array_values");
    write(
        &root.join("main.nia"),
        r#"
const fn pick(widths: [usize; 3], index: usize) usize {
    widths[index]
}

const widths: [usize; 3] = [2, 4, 8];
const width: usize = pick(widths, 2);

fn main() i32 {
    let mut values: [i32; width] = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_array_slices_are_ordinary_const_values() {
    let root = temp_dir("const_array_slices_are_ordinary_const_values");
    write(
        &root.join("main.nia"),
        r#"
const fn pair_sum(values: [usize; 2]) usize {
    values[0] + values[1]
}

const values: [usize; 4] = [1, 2, 3, 4];
const middle: [usize; 2] = values[1..3];
const prefix: [usize; 2] = values[..2];
const suffix: [usize; 2] = values[2..];
const direct: usize = pair_sum(values[1..=2]);
const n: usize = pair_sum(middle) + pair_sum(prefix) + pair_sum(suffix) + direct;

fn main() i32 {
    let mut array: [i32; n] = [0; n];
    array.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_try_propagates_optional_values() {
    let root = temp_dir("const_function_try_propagates_optional_values");
    write(
        &root.join("main.nia"),
        r#"
const fn add_one(value: ?usize) ?usize {
    let unwrapped: usize = value.?;
    if unwrapped == 7 {
        ?(unwrapped + 1)
    } else {
        ?1
    }
}

const some: ?usize = add_one(?7usize);
const none: ?usize = add_one(null);
const width: usize = match some {
    ?payload => {
        payload
    },
    null => {
        1
    },
};
const fallback: usize = match none {
    ?payload => {
        payload
    },
    null => {
        2
    },
};

fn main() i32 {
    let mut values: [i32; width + fallback] = [0; width + fallback];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_try_propagates_error_values() {
    let root = temp_dir("const_function_try_propagates_error_values");
    write(
        &root.join("main.nia"),
        r#"
const fn add_one(value: usize!usize) usize!usize {
    !(value.? + 1)
}

const ok: usize!usize = add_one(!7usize);
const err: usize!usize = add_one(3usize!);
const width: usize = match ok {
    !payload => {
        payload
    },
    err! => {
        0
    },
};
const fallback: usize = match err {
    !payload => {
        payload
    },
    err_payload! => {
        2
    },
};

fn main() i32 {
    let mut values: [i32; width + fallback] = [0; width + fallback];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_try_converts_errors_through_const_into_error() {
    let root = temp_dir("const_function_try_converts_errors_through_const_into_error");
    write(
        &root.join("main.nia"),
        r#"
trait IntoError[Target] {
    const fn intoError(self) Target;
}

enum SourceError: i32 {
    Failed = 1,
    _,
}

enum TargetError: i32 {
    Converted = 2,
    Unknown = 3,
    _,
}

extend SourceError : IntoError[TargetError] {
    const fn intoError(self) TargetError {
        match self {
            SourceError::Failed => TargetError::Converted,
            _ => TargetError::Unknown,
        }
    }
}

const fn propagate(value: SourceError!(usize, usize)) TargetError!(usize, usize) {
    !(value.?)
}

const success: TargetError!(usize, usize) = propagate(!(2usize, 3usize));
const failure: TargetError!(usize, usize) = propagate(SourceError::Failed!);
const successWidth: usize = match success {
    !payload => payload.0 + payload.1,
    cause! => 0,
};
const failureWidth: usize = match failure {
    !payload => payload.0 + payload.1,
    cause! => match cause {
        TargetError::Converted => 4usize,
        _ => 0,
    },
};

fn main() i32 {
    let values: [i32; successWidth + failureWidth] = [0; successWidth + failureWidth];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_try_instantiates_generic_into_error_witness() {
    let root = temp_dir("const_function_try_instantiates_generic_into_error_witness");
    write(
        &root.join("main.nia"),
        r#"
trait IntoError[Target] {
    const fn intoError(self) Target;
}

struct SourceError[T] {
    value: T,
}

enum TargetError: i32 {
    Converted = 1,
    _,
}

extend[T] SourceError[T] : IntoError[TargetError] {
    const fn intoError(self) TargetError {
        TargetError::Converted
    }
}

const fn propagate(value: SourceError[i32]!usize) TargetError!usize {
    !(value.?)
}

const failure = propagate(SourceError[i32] { value: 7 }!);
const width: usize = match failure {
    !value => value,
    TargetError::Converted! => 4,
    cause! => 0,
};

fn main() i32 {
    let values: [i32; width] = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
