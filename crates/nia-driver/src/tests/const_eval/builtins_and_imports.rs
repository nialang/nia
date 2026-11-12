// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn conditional_attribute_prunes_unselected_function_body_statement() {
    let root = temp_dir("conditional_attribute_prunes_unselected_function_body_statement");
    write(
        &root.join("main.nia"),
        r#"
fun main() i32 {
    @[if false]
    _ = missing_name;
    1
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn conditional_attribute_accepts_target_fields() {
    let root = temp_dir("conditional_attribute_accepts_target_fields");
    write(
        &root.join("main.nia"),
        r#"
@[if pointerWidth == 64 or pointerWidth == 32]
fun selected() i32 { 1 }

fun main() i32 { selected() }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn user_struct_fields_are_ordinary_const_values() {
    let root = temp_dir("user_struct_fields_are_ordinary_const_values");
    write(
        &root.join("main.nia"),
        r#"
struct Target { pointer_width: usize }
struct Config { target: Target }
const builtin = Config { target: Target { pointer_width: 64usize } };
const bits: usize = builtin.target.pointer_width;
const word_bytes: usize = bits / 8;

fun main() i32 {
    let mut bytes: [u8; word_bytes] = [0; word_bytes];
    bytes.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn removed_builtin_struct_is_rejected() {
    let root = temp_dir("removed_builtin_struct_is_rejected");
    write(
        &root.join("main.nia"),
        r#"
fun main() i32 {
    let builtin = @builtin();
    _ = builtin;
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("expected expression")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn builtin_functions_require_explicit_std_builtin_path_or_using() {
    let root = temp_dir("builtin_functions_require_explicit_std_builtin_path_or_using");
    std::fs::create_dir_all(root.join("assets")).expect("create assets dir");
    std::fs::write(root.join("assets/payload.bin"), b"nia").expect("write payload");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::{align, embed, offset, size};

struct Pair {
    tag: u8,
    value: u32,
}

const word_size: usize = size[usize]();
const word_align: usize = align[usize]();
const value_offset: usize = offset[Pair]("value");
const payload = embed("assets/payload.bin");
const payload_len: usize = payload.len();

fun main() usize {
    word_size + word_align + value_offset + payload_len
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_traits_require_explicit_std_builtin_using() {
    let root = temp_dir("builtin_traits_require_explicit_std_builtin_using");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Iterator;

struct Counter {
    next_value: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    fun next(&mut self) ?usize {
        if self.next_value >= self.end {
            null
        } else {
            let value = self.next_value;
            self.next_value += 1usize;
            ?value
        }
    }
}

fun count[I](iter: I) usize
where I: Iterator {
    let mut values = iter;
    let mut total = 0usize;
    let mut done = false;
    while not done {
        match values.next() {
            ?value => {
                _ = value;
                total += 1usize;
            },
            null => {
                done = true;
            },
        }
    }
    total
}

fun main() usize {
    count(Counter { next_value: 0usize, end: 3usize })
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn local_type_alias_can_use_builtin_trait_names_without_ambient_conflict() {
    let root = temp_dir("local_type_alias_can_use_builtin_trait_names_without_ambient_conflict");
    write(
        &root.join("main.nia"),
        r#"
type Ptr[T] = &T;

fun read(value: Ptr[u8]) u8 {
    value.*
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_rejects_nominal_array_field_assignment_type_mismatch() {
    let root = temp_dir("const_function_rejects_nominal_array_field_assignment_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
struct Target { os: [char; 5], pointer_width: usize }
struct Config { target: Target }

const fun width() usize {
    let mut config = Config { target: Target { os: "linux", pointer_width: 64usize } };
    config.target.os = true;
    config.target.pointer_width
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("expected array type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_nominal_bool_field_assignment_type_mismatch() {
    let root = temp_dir("const_function_rejects_nominal_bool_field_assignment_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
struct Config { enabled: bool }

const fun width() usize {
    let mut config = Config { enabled: true };
    config.enabled = 1usize;
    1usize
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("primitive type bool")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn old_target_predicate_builtins_are_rejected() {
    let root = temp_dir("old_target_predicate_builtins_are_rejected");
    write(
        &root.join("main.nia"),
        r#"
fun main() i32 {
    if @target_os("linux") {
        1
    } else {
        0
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("expected expression")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn imported_const_values_drive_array_lengths() {
    let root = temp_dir("imported_const_values_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
module config;
using root::config;

fun main() i32 {
    let mut values: [i32; config::width] = [1, 2, 3, 4];
    values[config::width - 1]
}
"#,
    );
    write(
        &root.join("config.nia"),
        r#"
pub const width: usize = 4;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn layout_builtins_are_const_values_for_concrete_types() {
    let root = temp_dir("layout_builtins_are_const_values_for_concrete_types");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    a: u8,
    b: i32,
}

const pair_size: usize = std::builtin::size[Pair]();
const pair_align: usize = std::builtin::align[Pair]();

fun main() i32 {
    let mut bytes: [u8; pair_size] = [0; pair_size];
    bytes.len() as i32 + pair_align as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn embed_reads_bytes_relative_to_source_file() {
    let root = temp_dir("embed_reads_bytes_relative_to_source_file");
    std::fs::create_dir_all(root.join("src/assets")).expect("create assets dir");
    std::fs::write(root.join("src/assets/payload.bin"), b"nia").expect("write payload");
    write(
        &root.join("src/main.nia"),
        r#"
const payload = std::builtin::embed("assets/payload.bin");

const fun score(bytes: [u8; 3]) usize {
    if bytes[0] == b'n' and bytes[1] == b'i' and bytes[2] == b'a' {
        bytes.len()
    } else {
        0usize
    }
}

const n: usize = score(payload);

fun main() i32 {
    let mut values: [u8; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("src/main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn embed_reports_missing_file() {
    let root = temp_dir("embed_reports_missing_file");
    write(
        &root.join("main.nia"),
        r#"
const payload = std::builtin::embed("missing.bin");

fun main() i32 {
    payload.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let errors = program
        .diagnostics
        .iter()
        .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
        .map(|d| d.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert!(
        errors.len() == 1 && errors[0].starts_with("failed to embed"),
        "{errors:?}"
    );
}

/// `embed` in a local constant has the type its bytes give it, as in a
/// global one. Body checking rejected every `embed` as runtime code, so a
/// local constant reported "can only be evaluated at const" although it is
/// a constant.
#[test]
fn embed_in_local_constants_has_its_file_type() {
    let root = temp_dir("embed_in_local_constants_has_its_file_type");
    std::fs::write(root.join("data.bin"), b"abc").expect("write payload");
    write(
        &root.join("main.nia"),
        r#"
using std::process;
const GLOBAL = std::builtin::embed("data.bin");
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    const LOCAL = std::builtin::embed("data.bin");
    const TYPED: [u8; 3] = std::builtin::embed("data.bin");
    if GLOBAL.len() != 3 or LOCAL.len() != 3 or TYPED[0] != 97 or TYPED[2] != 99 {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    );
    let source = root.join("main.nia");
    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("local constants embed their file");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run embed executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn embed_is_rejected_in_runtime_body() {
    let root = temp_dir("embed_is_rejected_in_runtime_body");
    std::fs::write(root.join("payload.bin"), [1u8, 2u8, 3u8]).expect("write payload");
    write(
        &root.join("main.nia"),
        r#"
fun main() i32 {
    let payload = std::builtin::embed("payload.bin");
    payload.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `embed` can only be evaluated at const")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unicode_scalar_conversion_evaluates_at_const() {
    let root = temp_dir("unicode_scalar_conversion_evaluates_at_const");
    write(
        &root.join("main.nia"),
        r#"
using std::unicode;

const scalar: ?char = unicode::fromScalarValue(0x03bb);
const surrogate: ?char = unicode::fromScalarValue(0xd800);
const width: usize = match scalar {
    ?value => if (value as u32) == 0x03bb { 3 } else { 1 },
    null => 1,
};
const invalidWidth: usize = match surrogate {
    ?value => if (value as u32) == 0xd800 { 1 } else { 1 },
    null => 2,
};

fun main() i32 {
    let values: [u8; width + invalidWidth] = [0; width + invalidWidth];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_builtins_resolve_through_every_import_spelling() {
    let root = temp_dir("const_builtins_resolve_through_every_import_spelling");
    for (index, prefix) in [
        "using std::builtin;\nconst N: usize = builtin::error(\"stop\");",
        "using std::builtin as b;\nconst N: usize = b::error(\"stop\");",
        "using std::builtin::error;\nconst N: usize = error(\"stop\");",
        "using std::builtin::{error as fail};\nconst N: usize = fail(\"stop\");",
        "using std::builtin::control;\nconst N: usize = control::error(\"stop\");",
    ]
    .into_iter()
    .enumerate()
    {
        for suffix in ["", "\nfun main() i32 { 0 }\n"] {
            let path = root.join(format!("main{index}.nia"));
            write(&path, &format!("{prefix}{suffix}"));
            for program in [
                check_program(path.to_string_lossy().into_owned()),
                check_entry_program(path.to_string_lossy().into_owned()),
            ] {
                let errors = program
                    .diagnostics
                    .iter()
                    .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
                    .map(|d| d.diagnostic.summary.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(errors, ["stop"], "{prefix}{suffix}");
            }
        }
    }
}

#[test]
fn const_enum_aliases_keep_context_across_modules_and_locals() {
    let root = temp_dir("const_enum_aliases_keep_context_across_modules_and_locals");
    write(
        &root.join("dep.nia"),
        r#"
pub enum Color { Red, Green }
pub type Shade = Color;
pub type Palette = [Shade; 2];
pub enum Maybe[T] { None, Some(T), Named { value: T } }
pub type Optional[T] = Maybe[T];
pub type Number = Optional[i32];
pub enum Failure { Oops }
pub type Outcome = Failure!Shade;
pub type Choice = ?Shade;
pub enum Packet[N: usize, T, M: usize] { Values([T; N]), Named { values: [T; M] }, Empty }
pub type PacketAlias[T] = Packet[1, T, 2];
pub const fun identity(value: Shade) Shade { value }
pub const fun red() Shade { .Red }
"#,
    );
    let path = root.join("main.nia");
    write(
        &path,
        r#"
module dep;
using std::builtin::error;
type Shade = dep::Shade;
const A: Shade = .Red;
const B: dep::Palette = [.Red, .Green];
const C: dep::Number = .Some(3);
const D: dep::Number = .None;
const E: dep::Shade = dep::identity(.Red);
const F: dep::Shade = dep::red();
const fun local() dep::Shade {
    const L: dep::Shade = .Red;
    L
}
const G: dep::Shade = local();
const H: dep::Optional[dep::Shade] = .Some(.Red);
const I: dep::Optional[dep::Shade] = .Named { value: .Green };
const J: dep::Outcome = .Oops!;
const K: dep::Outcome = !.Green;
const L: dep::Choice = ?.Red;
const CHECK: usize = if A == .Red and B[1] == .Green and E == .Red
    and F == .Red and .Red == G { 1 } else { error("wrong enum value") };
const PAYLOAD: i32 = match C { .Some(value) => value, .None => 0, .Named { value } => value };
const CHECK_PAYLOAD: usize = if PAYLOAD == 3 { 1 } else { error("wrong payload") };
const INNER: usize = match H { .Some(.Red) => 1, _ => 0 };
const NAMED: usize = match I { .Named { value: .Green } => 1, _ => 0 };
const CHECK_INNER: usize = if INNER + NAMED == 2 { 1 } else { error("wrong nested payload") };
const ERR: usize = match J { .Oops! => 1, !value => 0 };
const OK: usize = match K { error! => 0, !.Green => 1, _ => 0 };
const CHOICE: usize = match L { ?.Red => 1, _ => 0 };
const CHECK_WRAPPERS: usize = if ERR + OK + CHOICE == 3 { 1 } else { error("wrong wrapper payload") };
type Colors = dep::PacketAlias[dep::Shade];
const P: Colors = dep::Packet[1, dep::Shade, 2]::Values([.Red]);
const Q: Colors = dep::PacketAlias[dep::Shade]::Named { values: [.Red, .Green] };
const R: Colors = Colors::Empty;
const PCHECK: usize = match P { Colors::Values(values) => if values[0] == .Red { 1 } else { 0 }, _ => 0 };
const QCHECK: usize = match Q { Colors::Named { values } => if values[1] == .Green { 1 } else { 0 }, _ => 0 };
const RCHECK: usize = match R { Colors::Empty => 1, _ => 0 };
const CHECK_EXPLICIT: usize = if PCHECK + QCHECK + RCHECK == 3 { 1 } else { error("wrong explicit instance") };
"#,
    );
    for program in [
        check_program(path.to_string_lossy().into_owned()),
        check_entry_program(path.to_string_lossy().into_owned()),
    ] {
        assert_no_error_diagnostics(&program.diagnostics);
    }
}

#[test]
fn unknown_omitted_variants_preserve_enum_context_for_every_payload_form() {
    let root = temp_dir("unknown_omitted_variants_preserve_enum_context");
    write(
        &root.join("dep.nia"),
        "pub enum Maybe[T] { None, Some(T), Named { value: T } }\npub type Alias = Maybe[i32];\n",
    );
    for (name, expression) in [
        ("unit", ".Missing"),
        ("tuple", ".Missing(3)"),
        ("named", ".Missing { value: 3 }"),
    ] {
        let path = root.join("main.nia");
        write(
            &path,
            &format!(
                "module dep;\nconst C: dep::Alias = {expression};\nfun main() {{ let c: dep::Alias = {expression}; _ = c; }}\n"
            ),
        );
        for program in [
            check_program(path.to_string_lossy().into_owned()),
            check_entry_program(path.to_string_lossy().into_owned()),
        ] {
            let errors = program
                .diagnostics
                .iter()
                .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
                .map(|d| d.diagnostic.summary.as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                errors,
                [
                    "unknown variant `Missing` of enum `Alias`",
                    "unknown variant `Missing` of enum `Alias`",
                ],
                "{name}"
            );
        }
    }
}

#[test]
fn const_error_in_a_branch_typed_by_the_body_checker_is_compile_time() {
    // An omitted-member argument makes the body checker type the whole
    // initializer, including the `error` branch, in const context.
    let root = temp_dir("const_error_in_a_branch_typed_by_the_body_checker");
    write(
        &root.join("main.nia"),
        r#"
enum K { A, B }
const fun on(k: K) bool { match k { .A => true, .B => false } }
const A: usize = if on(.A) { 1 } else { std::builtin::error("unreachable branch") };
const B: usize = if on(.B) { 1 } else { std::builtin::error("taken branch") };
fun main() i32 { A as i32 }
"#,
    );
    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let errors = program
        .diagnostics
        .iter()
        .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
        .map(|d| d.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert_eq!(errors, ["taken branch"]);
}

#[test]
fn entry_module_without_runtime_items_is_still_checked() {
    let root = temp_dir("entry_module_without_runtime_items_is_still_checked");
    let path = root.join("main.nia");
    write(
        &path,
        "const N: usize = std::builtin::error(\"const only\");\nstruct S { field: Missing }\n",
    );
    let program = check_entry_program(path.to_string_lossy().into_owned());
    let summaries = program
        .diagnostics
        .iter()
        .map(|d| d.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert!(summaries.contains(&"const only"), "{summaries:?}");
    assert!(
        summaries.contains(&"unknown type `Missing`"),
        "{summaries:?}"
    );
}

#[test]
fn unknown_omitted_variant_names_the_expected_enum_once() {
    let root = temp_dir("unknown_omitted_variant_names_the_expected_enum_once");
    let path = root.join("main.nia");
    write(
        &path,
        r#"
enum Color { Red, Green }
const fun known(color: Color) bool { match color { .Red => true, .Green => false } }
const C: bool = known(.Blue);
fun main() i32 { let color: Color = .Blue; _ = color; 0 }
"#,
    );
    let program = check_program(path.to_string_lossy().into_owned());
    let errors = program
        .diagnostics
        .iter()
        .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
        .map(|d| d.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        errors,
        [
            "unknown variant `Blue` of enum `Color`",
            "unknown variant `Blue` of enum `Color`"
        ],
    );
}
