// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn generic_enum_payload_and_match_link_and_run() {
    let root = temp_dir("generic_enum_payload_and_match");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
using std::process;

trait Extract[T] {
    fun extract(&self, fallback: T) T;
}

enum Option[T] {
    Some(T),
    None,
}

enum Packet[T, N: usize] {
    Value(T),
    Bytes { data: [u8; N] },
    Nested { option: Option[T], data: [T; N] },
    Empty,
}

type NumberOption = Option[i32];
type NumberPacket = Packet[i32, 3];

extend[T] Option[T] : Extract[T] {
    fun extract(&self, fallback: T) T {
        match self.* {
            Option[T]::Some(value) => value,
            Option[T]::None => fallback,
        }
    }
}

fun read[T](value: Option[T], fallback: T) T where Option[T]: Extract[T] {
    value.extract(fallback)
}

fun packet_value(value: Packet[i32, 3]) i32 {
    match value {
        Packet[i32, 3]::Value(number) => number,
        Packet[i32, 3]::Bytes { data } => data[0] as i32,
        Packet[i32, 3]::Nested { option, data } => read[i32](option, 0) + data[0],
        Packet[i32, 3]::Empty => 0,
    }
}

fun packet_value64(value: Packet[i64, 4]) i64 {
    match value {
        Packet[i64, 4]::Value(number) => number,
        Packet[i64, 4]::Bytes { data } => data[0] as i64,
        Packet[i64, 4]::Nested { option, data } => read[i64](option, 0) + data[0],
        Packet[i64, 4]::Empty => 0,
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    let make: &fun(i32) Option[i32] = &Option[i32]::Some;
    let some = read[i32](make(9), 0);
    let make64: &fun(i64) Option[i64] = &Option[i64]::Some;
    let some64 = read[i64](make64(10), 0);
    let none = read[i32](Option[i32]::None, 7);
    let aliasMake: &fun(i32) NumberOption = &NumberOption::Some;
    let aliasSome = read[i32](aliasMake(13), 0);
    let aliasNone = match NumberOption::None {
        NumberOption::None => 14,
        NumberOption::Some(value) => value,
    };
    let aliasNamed = packet_value(NumberPacket::Nested {
        option: NumberOption::Some(5),
        data: [6, 7, 8],
    });
    let packet_make: &fun(i32) Packet[i32, 3] = &Packet[i32, 3]::Value;
    let packet = packet_value(packet_make(8));
    let bytes_make: &fun([u8; 3]) Packet[i32, 3] = &Packet[i32, 3]::Bytes;
    let bytes = packet_value(bytes_make([4, 5, 6]));
    let nested = packet_value(Packet[i32, 3]::Nested {
        option: Option[i32]::Some(5),
        data: [6, 7, 8],
    });
    let packet_make64: &fun(i64) Packet[i64, 4] = &Packet[i64, 4]::Value;
    let packet64 = packet_value64(packet_make64(12));
    _ = init;
    if some != 9
        or some64 != 10
        or none != 7
        or aliasSome != 13
        or aliasNone != 14
        or aliasNamed != 11
        or packet != 8
        or bytes != 4
        or nested != 11
        or packet64 != 12
    {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    );

    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("generic enum executable must link");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run generic enum executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn imported_generic_enum_instances_link_with_distinct_constructor_abis() {
    let root = temp_dir("imported_generic_enum_instances");
    write(
        &root.join("model.nia"),
        r#"
pub enum Packet[T, N: usize] {
    Value(T),
    Bytes { data: [u8; N] },
    Empty,
}

"#,
    );
    let source = root.join("main.nia");
    write(
        &source,
        r#"
module model;
using root::model;
using root::model::Packet;
using std::process;

const fun importedBytes() model::Packet[i32, 3] {
    model::Packet[i32, 3]::Bytes { data: [1, 2, 3] }
}

fun read32(value: Packet[i32, 3], fallback: i32) i32 {
    match value {
        Packet[i32, 3]::Value(number) => number,
        _ => fallback,
    }
}

fun read64(value: Packet[i64, 3], fallback: i64) i64 {
    match value {
        Packet[i64, 3]::Value(number) => number,
        _ => fallback,
    }
}

fun read64_4(value: Packet[i64, 4], fallback: i64) i64 {
    match value {
        Packet[i64, 4]::Value(number) => number,
        _ => fallback,
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    match importedBytes() {
        model::Packet[i32, 3]::Bytes { data } => {
            if data[0] != 1 or data[2] != 3 { return process::ExitCode(3)!; }
        },
        _ => return process::ExitCode(2)!,
    }
    let make32: &fun(i32) Packet[i32, 3] = &Packet[i32, 3]::Value;
    let make64: &fun(i64) Packet[i64, 3] = &Packet[i64, 3]::Value;
    let bytes32: &fun([u8; 3]) Packet[i32, 3] = &Packet[i32, 3]::Bytes;
    let bytes64: &fun([u8; 4]) Packet[i64, 4] = &Packet[i64, 4]::Bytes;
    let value32 = read32(make32(21), 0);
    let value64 = read64(make64(34), 0);
    let byte_value32 = read32(bytes32([1, 2, 3]), 0);
    let byte_value64 = read64_4(bytes64([4, 5, 6, 7]), 0);
    _ = init;
    if value32 != 21 or value64 != 34 or byte_value32 != 0 or byte_value64 != 0 {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    );

    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("imported generic enum executable must link");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run imported generic enum executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn generic_enum_error_constructors_keep_instances_through_propagation() {
    let root = temp_dir("generic_enum_error_constructors_through_propagation");
    write(
        &root.join("errors.nia"),
        r#"
pub enum Failure[E] {
    Memory(u32),
    Read(E),
}
"#,
    );
    let source = root.join("main.nia");
    write(
        &source,
        r#"
module errors;
using root::errors;
using root::errors::Failure;
using std::process;

extend[Value, Source, Target] Source!Value {
    fun mapError(self, mapper: &Fun(Source) Target) Target!Value {
        match self {
            !value => !value,
            error! => mapper(error)!,
        }
    }
}

fun omitted(value: u32!i32) Failure[u8]!i32 {
    let mapped = value.mapError(.Memory).?;
    !mapped
}

fun explicit(value: u32!i32) Failure[u8]!i32 {
    let mapped = value.mapError(Failure[u8]::Memory).?;
    !mapped
}

fun readValue(value: u8) Failure[u8] {
    let constructor: &fun(u8) Failure[u8] = &Failure[u8]::Read;
    constructor(value)
}

fun generic[E](value: E!i32) Failure[E]!i32 {
    let mapped = value.mapError(.Read).?;
    !mapped
}

enum ReadFault { End, Source(u32) }

struct Outer { value: u32 }
extend Failure[u8] : std::builtin::IntoError[Outer] {
    fun intoError(self) Outer {
        match self { .Memory(value) => .{ value }, .Read(value) => .{ value: value as u32 }, }
    }
}

fun converted(value: u32!i32) Outer!i32 {
    let mapped = value.mapError(Failure[u8]::Memory).?;
    !mapped
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    match omitted(!1) {
        !value => { if value != 1 { return process::ExitCode(1)!; } },
        error! => return process::ExitCode(2)!,
    }
    match explicit(!2) {
        !value => { if value != 2 { return process::ExitCode(3)!; } },
        error! => return process::ExitCode(4)!,
    }
    match omitted(41u32!) {
        Failure[u8]::Memory(value)! => { if value != 41 { return process::ExitCode(7)!; } },
        _ => return process::ExitCode(8)!,
    }
    match explicit(42u32!) {
        Failure[u8]::Memory(value)! => { if value != 42 { return process::ExitCode(9)!; } },
        _ => return process::ExitCode(10)!,
    }
    match generic[u8](43u8!) {
        Failure[u8]::Read(value)! => { if value != 43 { return process::ExitCode(11)!; } },
        _ => return process::ExitCode(12)!,
    }
    match generic[u64](12345678900u64!) {
        Failure[u64]::Read(value)! => { if value != 12345678900 { return process::ExitCode(13)!; } },
        _ => return process::ExitCode(14)!,
    }
    match generic[ReadFault](ReadFault::Source(123)!) {
        errors::Failure[ReadFault]::Read(ReadFault::Source(123))! => {},
        _ => return process::ExitCode(17)!,
    }
    match generic[ReadFault](ReadFault::End!) {
        root::errors::Failure[ReadFault]::Read(ReadFault::End)! => {},
        _ => return process::ExitCode(18)!,
    }
    match converted(44u32!) {
        error! => { if error.value != 44 { return process::ExitCode(15)!; } },
        _ => return process::ExitCode(16)!,
    }
    match readValue(7) {
        Failure[u8]::Read(value) => { if value != 7 { return process::ExitCode(5)!; } },
        _ => return process::ExitCode(6)!,
    }
    !()
}
"#,
    );

    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("generic enum constructors must preserve error instances");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run generic enum error constructor executable");
    assert_eq!(status.code(), Some(0));
}

/// A variant named through an alias builds the instance the alias names. An
/// untyped constant took the alias type itself, which only the module's
/// declared types normalize, so matching on it reported that `.A` was not a
/// struct or enum variant while the same value bound by `let` matched.
#[test]
fn untyped_constants_built_through_aliases_match_their_instance() {
    let root = temp_dir("untyped_constants_built_through_aliases");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
using std::process;
enum F[T] { A(T), B, C { x: T } }
type Q = F[i32];
type P[T] = F[T];
const TUPLE = Q::A(7);
const UNIT = Q::B;
const NAMED = Q::C { x: 9 };
const GENERIC = P[i32]::A(11);
fun code(value: F[i32]) i32 {
    match value { .A(v) => v, .B => 100, .C { x } => x + 1000 }
}
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    const LOCAL = Q::A(13);
    let direct = match TUPLE { .A(v) => v, .B => 0, .C { x } => x };
    if direct != 7 or code(UNIT) != 100 or code(NAMED) != 1009
        or code(GENERIC) != 11 or code(LOCAL) != 13
    {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    );
    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("constants built through aliases must keep their enum instance");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run alias-built constant executable");
    assert_eq!(status.code(), Some(0));
}

/// A generic type used without its arguments is reported once, in every
/// position, and produces no instance whose arguments do not match its
/// parameters. Each of these once stopped with an internal error from
/// layout-root collection.
#[test]
fn generic_types_without_arguments_are_reported_not_instantiated() {
    let root = temp_dir("generic_types_without_arguments_are_reported_not_instantiated");
    for (name, source) in [
        (
            "struct literal",
            "struct W[T] { x: T }\nfun main() i32 { let v = W { x: 1i32 }; v.x }\n",
        ),
        (
            "tuple struct",
            "struct T2[T](T)\nfun main() i32 { let v = T2(1i32); v.0 }\n",
        ),
        (
            "enum variant",
            "enum M[T] { Some(T), No }\nfun main() i32 { match M::Some(1i32) { .Some(x) => x, .No => 0 } }\n",
        ),
        (
            "annotation",
            "struct W[T] { x: T }\nfun main() i32 { let v: W = W[i32] { x: 1 }; _ = v; 0 }\n",
        ),
        (
            "parameter",
            "struct W[T] { x: T }\nfun f(w: W) i32 { _ = w; 0 }\nfun main() i32 { 0 }\n",
        ),
    ] {
        let path = root.join(format!("{}.nia", name.replace(' ', "_")));
        write(&path, source);
        let program = check_program(path.to_string_lossy().into_owned());
        let errors = program
            .diagnostics
            .iter()
            .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
            .map(|d| d.diagnostic.summary.as_str())
            .collect::<Vec<_>>();
        assert!(
            errors.len() == 1 && errors[0].starts_with("generic argument count mismatch for `"),
            "{name}: {errors:?}"
        );
    }
    // Written arguments and an expected type both determine the instance.
    let path = root.join("valid.nia");
    write(
        &path,
        r#"
struct W[T] { x: T }
struct T2[T](T)
enum M[T] { Some(T), No }
extend[T] W[T] { fun new(x: T) W[T] { W[T] { x } } }
fun fromExpected() M[i32] { .Some(4) }
fun main() i32 {
    let a = W[i32] { x: 1 };
    let b: W[i32] = .{ x: 2 };
    let c: W[i32] = W::new(3);
    let d = T2[i32](5);
    let e = M[i32]::Some(6);
    _ = e;
    _ = fromExpected();
    a.x + b.x + c.x + d.0
}
"#,
    );
    let program = check_program(path.to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}
