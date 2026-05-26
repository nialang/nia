// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_defs::{
    DefKind, ModuleId, VisibleExtensionMethod, VisibleExtensionMethods, collect_module_defs,
};
use nia_item_signatures::collect_item_signatures;
use nia_local_resolve::resolve_module_locals;
use nia_parser::parse_module;
use nia_type_lower::lower_module_types;
use nia_type_resolve::resolve_module_types;

fn pipeline(source: &str) -> BodyCheck {
    let (module, parse_errors) = parse_module(source);
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let type_resolved = resolve_module_types(&module, &defs);
    assert!(
        type_resolved.diagnostics.is_empty(),
        "{:?}",
        type_resolved.diagnostics
    );
    let lowered = lower_module_types(&module, &type_resolved);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let values = nia_value_resolve::resolve_module_values(&module, &defs);
    assert!(values.diagnostics.is_empty(), "{:?}", values.diagnostics);
    let locals = resolve_module_locals(&module, &defs, &values);
    assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
    let signatures = collect_item_signatures(&module, &defs, &lowered);
    assert!(
        signatures.diagnostics.is_empty(),
        "{:?}",
        signatures.diagnostics
    );
    let mut extensions = VisibleExtensionMethods::default();
    for item in &module.items {
        let nia_ast::ItemKind::Extend(extend) = &item.kind else {
            continue;
        };
        let Some(target_ty) = lowered.type_uses.get(&extend.target.span).copied() else {
            continue;
        };
        let Some(nia_ty::TyKind::Nominal {
            def_id: target,
            args,
        }) = lowered.interner.get(target_ty)
        else {
            continue;
        };
        for method in &extend.methods {
            let Some(method_id) = defs.def_spans.get(method.function.span) else {
                continue;
            };
            let Some(method_def) = defs.defs.get(method_id) else {
                continue;
            };
            if method_def.kind != DefKind::Method {
                continue;
            }
            extensions.insert(
                *target,
                args.clone(),
                VisibleExtensionMethod {
                    name: method_def.name.clone(),
                    def_id: GlobalDefId {
                        module_id: ModuleId(0),
                        def_id: method_id,
                    },
                },
            );
        }
    }
    let normalization = TypeNormalization {
        interner: lowered.interner.clone(),
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let layouts = nia_layout::compute_layouts(
        &defs,
        &lowered.interner,
        &signatures,
        nia_layout::TargetDataLayout::LP64,
    );
    check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        module: &module,
        defs: &defs,
        all_defs: std::slice::from_ref(&defs),
        values: &values,
        locals: &locals,
        lowered: &lowered,
        signatures: &signatures,
        normalization: &normalization,
        layouts: &layouts,
        extensions: &extensions,
        program_signatures: ProgramSignatureMaps {
            functions: &HashMap::new(),
            globals: &HashMap::new(),
            structs: &HashMap::new(),
            unions: &HashMap::new(),
            enums: &HashMap::new(),
        },
    })
}

#[test]
fn checks_return_tail_and_local_binding_types() {
    let checked = pipeline(
        r#"
fn add(a: i32, b: i32) i32 {
    var sum: i32 = a + b;
    sum
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_union_literals_and_field_access() {
    let checked = pipeline(
        r#"
union Bits[T] {
    i: i64,
    value: T,
}

fn main() i32 {
    var bits: Bits[i32] = { value: 10 };
    bits.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let bad_empty = pipeline(
        r#"
union Bits {
    i: i32,
}

fn main() i32 {
    var bits: Bits = {};
    0
}
"#,
    );
    assert!(
        bad_empty
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("exactly one field")),
        "{:?}",
        bad_empty.diagnostics
    );

    let bad_multi = pipeline(
        r#"
union Bits {
    i: i32,
    f: f32,
}

fn main() i32 {
    var bits: Bits = { i: 1, f: 2.0 };
    0
}
"#,
    );
    assert!(
        bad_multi
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("exactly one field")),
        "{:?}",
        bad_multi.diagnostics
    );
}

#[test]
fn checks_void_values_empty_structs_and_void_pointers() {
    let checked = pipeline(
        r#"
struct Empty {}

fn take_void(p: &void) {}
fn take_const_void(p: &const void) {}

fn main() {
    var unit: void = {};
    var empty: Empty = {};
    var value: i32 = 1;
    take_void(&value as &void);
    take_const_void(&const value as &const void);
    unit
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let bad_implicit = pipeline(
        r#"
fn main() {
    var value: i32 = 1;
    var ptr: &void = &value;
}
"#,
    );
    assert!(
        bad_implicit
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer")),
        "{:?}",
        bad_implicit.diagnostics
    );

    let bad_deref = pipeline(
        r#"
fn main() i32 {
    var value: i32 = 1;
    var ptr: &void = &value as &void;
    ptr.*
}
"#,
    );
    assert!(
        bad_deref
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("cannot dereference `&void`")),
        "{:?}",
        bad_deref.diagnostics
    );
}

#[test]
fn accepts_explicit_return_without_tail_expression() {
    let checked = pipeline(
        r#"
extern fn printf(fmt: &u8, ...);

fn main() i32 {
    var hello = "hello, world!\n\0";
    printf(&hello[0]);
    return 0;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn reports_mismatched_return_and_binding_types() {
    let checked = pipeline(
        r#"
fn bad(flag: bool) i32 {
    var x: bool = 1;
    flag
}
"#,
    );
    assert!(checked.diagnostics.len() >= 2, "{:?}", checked.diagnostics);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("function body"))
    );
}

#[test]
fn checks_integer_literal_ranges_against_expected_types() {
    let checked = pipeline(
        r#"
struct Bytes {
    first: u8,
}

fn take_byte(x: u8) u8 { x }

fn ret_good() u8 { 255 }
fn ret_bad() u8 { 256 }

fn main() u8 {
    var ok: u8 = 255;
    var too_large: u8 = 256;
    var negative: u8 = -1;
    var xs: [2]u8 = [0, 256];
    var b: Bytes = { first: 300 };
    _ = take_byte(128);
    _ = take_byte(999);
    ret_good()
}
"#,
    );
    let range_errors = checked
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.contains("out of range for U8"))
        .count();
    assert_eq!(range_errors, 6, "{:?}", checked.diagnostics);
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("type mismatch"))
    );
}

#[test]
fn checks_float_literals_against_expected_float_types() {
    let checked = pipeline(
        r#"
fn take32(x: f32) f32 { x }

fn main() f64 {
    var a: f32 = 1.5;
    var b: f64 = 1e3;
    var too_large: f32 = 1e100;
    var wrong: i32 = 1.5;
    _ = take32(2.5);
    _ = take32(1e100);
    b
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("out of range for F32")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn defaults_unconstrained_numeric_literals_and_honors_expected_types() {
    let checked = pipeline(
        r#"
fn take_byte(x: u8) u8 { x }
fn take32(x: f32) f32 { x }

fn main() i32 {
    var default_int = 10;
    var default_float = 1.5;
    var explicit_byte: u8 = 10;
    var negative_byte: i8 = -1;
    var explicit_float: f32 = 1.5;
    _ = take_byte(3);
    _ = take32(2.5);
    default_int
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        checked
            .local_types
            .values()
            .any(|ty| checked.interner.get(*ty)
                == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))),
        "{:?}",
        checked.local_types
    );
    assert!(
        checked
            .local_types
            .values()
            .any(|ty| checked.interner.get(*ty)
                == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::F64))),
        "{:?}",
        checked.local_types
    );
    assert!(
        checked
            .local_types
            .values()
            .any(|ty| checked.interner.get(*ty)
                == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::U8))),
        "{:?}",
        checked.local_types
    );
    assert!(
        checked
            .local_types
            .values()
            .any(|ty| checked.interner.get(*ty)
                == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::F32))),
        "{:?}",
        checked.local_types
    );
}

#[test]
fn infers_binary_numeric_literals_from_the_other_operand() {
    let checked = pipeline(
        r#"
const a = 10;
const ptr = &const a;

fn main(x: usize) bool {
    var forward = a as usize == 0;
    var reverse = 0 == a as usize;
    var sum: usize = 1 + x;
    var expected_sum: usize = 1 + 2;
    var shifted: usize = 1 << 2;
    forward and reverse and sum == x + 1 and expected_sum == 3 and shifted == 4
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_if_branch_numeric_literals_from_expected_and_peer_types() {
    let checked = pipeline(
        r#"
fn from_return(flag: bool) usize {
    if flag { 1 } else { 2 }
}

fn main(flag: bool, x: usize) usize {
    var from_binding: usize = if flag { 0 } else { x };
    var from_peer = if flag { 1 } else { x };
    from_return(flag) + from_binding + from_peer
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn numeric_literal_suffixes_are_not_accepted() {
    let (_module, parse_errors) = parse_module(
        r#"
fn main() i32 {
    var bad_int: i32 = 1u8;
    var bad_float: f64 = 1.0f32;
    0
}
"#,
    );
    assert!(
        parse_errors
            .iter()
            .any(|error| error.message.contains("expected `;` after binding")),
        "{:?}",
        parse_errors
    );
}

#[test]
fn checks_cast_rules_and_function_item_pointers() {
    let checked = pipeline(
        r#"
enum Color: u8 {
    Red,
}

fn id(x: i32) i32 { x }
fn gid[T](x: T) T { x }

fn main(ptr: &const u8, other: &const i32, flag: bool) i32 {
    var a: i64 = 1 as i64;
    var b: f64 = 1 as f64;
    var c: i32 = Color::Red as i32;
    var addr: usize = ptr as usize;
    var ptr2: &i32 = addr as &i32;
    var ptr3: &i32 = ptr as &i32;
    var bad1: bool = 1 as bool;
    var bad2: i32 = ptr as i32;
    var bad3: i32 = flag as i32;
    var fn_value = id;
    var bad_fn_ptr = &id;
    var fn_ptr = &const id;
    var generic_ptr = &const gid[i32];
    _ = fn_ptr(1);
    _ = generic_ptr(2);
    a as i32 + c + ptr2.* + ptr3.*
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("invalid cast"))
            .count(),
        3,
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("function values are not supported"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("function pointers must be formed with `&const`"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_simple_calls_to_module_functions() {
    let checked = pipeline(
        r#"
fn id(x: i32) i32 { x }
fn main() i32 {
    id(1)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_direct_call_argument_count_and_types() {
    let checked = pipeline(
        r#"
fn add(a: i32, b: i32) i32 { a + b }

fn main(flag: bool) i32 {
    _ = add(flag, 1);
    _ = add(1);
    0
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("argument count mismatch"))
    );
}

#[test]
fn checks_aggregate_literals_from_call_argument_context() {
    let checked = pipeline(
        r#"
struct Item {
    value: i32,
}

fn take(items: &const [Item]) i32 {
    @len(items) as i32
}

fn main() i32 {
    take([
        { value: 1 },
        { value: 2 },
    ])
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_explicit_generic_function_calls() {
    let checked = pipeline(
        r#"
fn id[T](value: T) T { value }
fn pair[T](left: T, right: T) T { left }

fn main(flag: bool) i32 {
    var x: i32 = id[i32](1);
    _ = id[i32](flag);
    _ = id[i32, bool](1);
    _ = pair[bool](true, false);
    x
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("generic argument count mismatch")
    }));
}

#[test]
fn infers_generic_function_type_arguments_from_call_arguments() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

fn id[T](value: T) T { value }
fn unbox[T](box: Box[T]) T { box.value }
fn deref_id[T](value: &T) T { value.* }
fn choose[T](left: T, right: T) T { left }

fn main(box: Box[i32], ptr: &const i32, flag: bool) i32 {
    var a: i32 = id(1);
    var b: i32 = unbox(box);
    var c: i32 = deref_id(ptr);
    _ = choose(1, flag);
    a + b + c
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("conflicting inferred type for generic parameter `T`")
    }));
}

#[test]
fn infers_generic_function_type_arguments_from_expected_return_type() {
    let checked = pipeline(
        r#"
fn id[T](value: T) T { value }
fn choose[T](left: T, right: T) T { left }

fn from_return() i32 {
    id(1)
}

fn main() i32 {
    var a: i32 = id(1);
    var b: usize = id(1);
    var c: i32 = choose(id(1), 2);
    _ = id(1);
    a + b as i32 + c + from_return()
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("function body")),
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("cannot infer generic parameter `T`"))
            .count(),
        0,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_fixed_prefix_of_variadic_extern_calls() {
    let checked = pipeline(
        r#"
extern fn printf(fmt: &u8, ...);

fn main(flag: bool) i32 {
    _ = printf(flag, 1);
    var s = "hello\0";
    printf(&s[0], &s[..]);
    var sp = &(&s[..]);
    printf(&s[0], sp.*);
    0
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("variadic argument")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("argument count mismatch"))
    );
}

#[test]
fn records_size_and_align_builtin_values() {
    let checked = pipeline(
        r#"
struct Pair {
    a: u8,
    b: i32,
}

fn main() usize {
    var size = @size[Pair]();
    var align = @align[Pair]();
    size + align
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        checked
            .builtin_values
            .values()
            .any(|value| *value == BuiltinValue::Usize(8))
    );
    assert!(
        checked
            .builtin_values
            .values()
            .any(|value| *value == BuiltinValue::Usize(4))
    );
}

#[test]
fn accepts_layout_builtins_as_array_lengths() {
    let checked = pipeline(
        r#"
struct Pair {
    a: u8,
    b: i32,
}

fn take_size(xs: [@size[Pair]()]u8) u8 {
    xs[0]
}

fn main() u8 {
    var exact: [@size[Pair]()]u8 = [b'\0'; 8];
    var aligned: [@align[Pair]()]u8 = [b'\0'; 4];
    _ = take_size(exact);
    aligned[0]
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_slices_len_ptr_and_indexing() {
    let checked = pipeline(
        r#"
fn read(xs: &const [i32]) i32 {
    xs[0]
}

fn write(xs: &[i32]) i32 {
    xs[0] = 10;
    xs[0]
}

fn main() i32 {
    var xs: [4]i32 = [1, 2, 3, 4];
    var s = &const xs[..];
    var t = &const xs[1..=2];
    var p = &const xs[0];
    var single = &const p[..];
    _ = @ptr(s);
    @len(xs) as i32 + @len(s) as i32 + @len(t) as i32 + @len(single) as i32 + read(s)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_bare_range_index_and_readonly_slice_assignment() {
    let checked = pipeline(
        r#"
fn main(xs: &const [i32]) i32 {
    var y = xs[..];
    xs[0] = 1;
    0
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("range index expression must be borrowed")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("slice is const") }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_array_literal_element_types() {
    let checked = pipeline(
        r#"
fn main(flag: bool) i32 {
    var xs: [2]i32 = [1, flag];
    var ys: [3]i32 = [flag; 3];
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("array literal element"))
            .count(),
        2
    );
}

#[test]
fn checks_array_literal_lengths_and_inferred_lengths() {
    let checked = pipeline(
        r#"
fn take_pair(xs: [2]i32) i32 {
    xs[0]
}

fn main() i32 {
    var inferred: [2]i32 = [1, 2];
    var repeated: [3]i32 = [1; 3];
    var too_many: [2]i32 = [1, 2, 3];
    var bad_repeat: [2]i32 = [1; 3];
    take_pair([1, 2])
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("array literal length mismatch"))
            .count(),
        2
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
}

#[test]
fn reports_invalid_array_repeat_count() {
    let checked = pipeline(
        r#"
fn main() i32 {
    var bad: [2]i32 = [1; 1 / 0];
    0
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("array repeat count is not a valid constant")
            && diagnostic.message.contains("division by zero")
    }));
}

#[test]
fn treats_string_literals_as_u8_arrays() {
    let checked = pipeline(
        r#"
fn main() i32 {
    var exact: [4]u8 = "nia\0";
    var inferred: [_]u8 = "hi\n";
    var multiline: [11]u8 =
        \\hello
        \\world
    ;
    var wrong: [3]u8 = "nia\0";
    var byte: u8 = b'a';
    var ch: char = 'a';
    var bad_byte: u8 = 'a';
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("binding initializer"))
            .count(),
        2
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("exact"))
    );
}

#[test]
fn checks_index_and_address_of_array_elements() {
    let checked = pipeline(
        r#"
extern fn puts(ptr: &u8) i32;

const hello = "hello\0";

fn main(flag: bool) i32 {
    var xs: [2]u8 = [1, 2];
    var p: &u8 = &xs[0];
    var c: &u8 = &hello[0];
    _ = puts(&hello[0]);
    _ = xs[flag];
    0
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("index"))
    );
}

#[test]
fn checks_array_to_slice_coercions_and_rejects_address_of_rvalues() {
    let checked = pipeline(
        r#"
fn take(xs: &const [i32]) i32 {
    @len(xs) as i32
}

fn mutate(xs: &[i32]) i32 {
    xs[0] = 9;
    xs[0]
}

fn bytes(xs: &const [u8]) i32 {
    @len(xs) as i32
}

fn main() i32 {
    var ro: &const [i32] = [1, 2, 3];
    var rw: &[i32] = [4, 5];
    var arr: [2]i32 = [6, 7];
    var from_place: &const [i32] = arr;
    var from_string: &const [u8] = "hi\0";
    _ = take([1, 2, 3]);
    _ = mutate([4, 5]);
    _ = bytes("hi\0");

    var bad_int: &i32 = &10;
    var bad_sum: &i32 = &(1 + 2);
    var bad_call: &i32 = &make();
    var bad_slice: &const [i32] = &const [1, 2, 3][..];
    0
}

fn make() i32 {
    42
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("reference target is not assignable"))
            .count()
            >= 3,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("slice target is not addressable")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_struct_literal_fields() {
    let checked = pipeline(
        r#"
struct Pair {
    left: i32,
    right: bool,
}

fn main() i32 {
    var bad: Pair = { left: true, left: 1, extra: 1 };
    var inferred: Pair = { left: 1, right: false };
    0
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("struct literal field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown struct field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("duplicate struct field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("missing struct field"))
    );
}

#[test]
fn checks_struct_field_access() {
    let checked = pipeline(
        r#"
struct Pair[T] {
    left: T,
    right: bool,
}

fn main(pair: Pair[i32], ptr: &const Pair[i32]) i32 {
    var x: i32 = pair.left;
    var y: bool = ptr.right;
    _ = pair.missing;
    pair.right
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown struct field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("function body"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
}

#[test]
fn rejects_implicit_discard_of_non_void_expression_statements() {
    let checked = pipeline(
        r#"
fn value() i32 { 1 }
fn effect() {}
extern fn abort() !;

fn main() i32 {
    value();
    _ = value();
    effect();
    abort();
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("non-void expression result"))
            .count(),
        1
    );
}

#[test]
fn checks_function_pointer_fields_as_void_calls() {
    let checked = pipeline(
        r#"
struct Vtable {
    print: &const fn(&i32)
}

fn print_i32(value: &i32) {}

const vtable: Vtable = { print: &const print_i32 };

fn main() i32 {
    var x = 1;
    vtable.print(&x);
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_assignment_targets_and_const_bindings() {
    let checked = pipeline(
        r#"
const global_const: i32 = 1;
var global_mut: i32 = 0;

struct Cell {
    value: i32,
}

fn main(param: i32, read: &const i32, write: &i32, cell: Cell, read_cell: &const Cell, write_cell: &Cell) i32 {
    const local_const = 1;
    var local_mut = 1;
    local_mut = 2;
    param = 3;
    global_mut = 4;
    local_const = 5;
    global_const = 6;
    read.* = 7;
    write.* = 8;
    cell.value = 9;
    read_cell.value = 10;
    write_cell.value = 11;
    0
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("local is const"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("global is const"))
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("pointer is const"))
            .count(),
        2
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("local_mut"))
    );
}

#[test]
fn checks_method_calls_and_receiver_matching() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn get(&const self) T {
        self.value
    }

    fn set(&self, value: T) {
        self.value = value;
    }
}

fn main(ro: &const Box[i32], rw: &Box[i32]) i32 {
    var box: Box[i32] = { value: 1 };
    var x: i32 = box.get();
    var y: i32 = ro.get();
    rw.set(2);
    ro.set(3);
    box.set(true);
    box.get(1);
    x + y
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("receiver cannot be matched through `&const T`")
    }));
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("argument count mismatch"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
}

#[test]
fn accepts_local_binding_declarations() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
    y: i32,
}

extend Point {
    fn inspect(&const self) i32 { self.x }
    fn init(&self) {}
    fn deinit(&self) {}
}

fn main() {
    var p: Point;
    p.init();
    defer p.deinit();
    const origin: Point;
    _ = origin.inspect();
    const n: i32;
    var copied: i32 = n;
    var borrowed: &const i32 = &const n;
    _ = copied;
    _ = borrowed;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_mutating_const_uninitialized_bindings() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

extend Point {
    fn init(&self) {}
}

fn main() {
    const origin: Point;
    origin.init();
    const n: i32;
    n = 1;
    _ = &n;
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("receiver is not assignable")
                || diagnostic
                    .message
                    .contains("reference target is not assignable")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("local is const")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_explicit_generic_method_calls() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn replace[U](&const self, value: U) U {
        value
    }

    fn get(&const self) T {
        self.value
    }
}

fn main(flag: bool) i32 {
    var box: Box[i32] = { value: 1 };
    var x: i32 = box.replace[i32](2);
    var y: bool = box.replace[bool](flag);
    var z: i32 = box.get();
    _ = box.replace[i32](flag);
    _ = box.replace();
    _ = box.get[i32]();
    x + z
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("generic argument count mismatch for method"))
            .count(),
        2
    );
}

#[test]
fn checks_function_pointer_calls() {
    let checked = pipeline(
        r#"
fn main(cb: &const fn(i32, bool) i64, variadic: &const fn(i32, ...) void, flag: bool) i64 {
    var x: i64 = cb(1, flag);
    _ = cb(flag, flag);
    _ = cb(1);
    variadic(flag, 1);
    x
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("call argument"))
            .count(),
        2
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("argument count mismatch"))
            .count(),
        1
    );
}

#[test]
fn checks_associated_function_calls() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

extend Point {
    fn new(x: i32) Point {
        { x: x }
    }

    fn get(&const self) i32 {
        self.x
    }
}

fn main(flag: bool) i32 {
    var p = Point::new(1);
    var value: i32 = Point::get(&p);
    _ = Point::new(flag);
    _ = Point::new();
    _ = Point::get();
    p::get();
    value
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("argument count mismatch"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("receiver method `get` requires")
    }));
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("qualified access is not a value expression")
    }));
}

#[test]
fn checks_generic_type_prefix_associated_function_calls() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }

    fn empty() Box[T] {
        { value: 0 }
    }
}

fn main(flag: bool) i32 {
    var a: Box[i32] = Box[i32]::make(1);
    _ = Box[i32]::make(flag);
    _ = Box[i32, bool]::make(1);
    _ = Box::empty();
    a.value
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("generic argument count mismatch for `Box`")
    }));
}

#[test]
fn checks_lowercase_generic_type_prefix_associated_function_calls() {
    let checked = pipeline(
        r#"
struct box[T] {
    value: T,
}

extend[T] box[T] {
    fn make(value: T) box[T] {
        { value: value }
    }
}

fn main() i32 {
    var a: box[i32] = box[i32]::make(1);
    a.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_enum_variants_and_switch_exhaustiveness() {
    let checked = pipeline(
        r#"
enum Color {
    Red,
    Green,
    Blue,
}

enum Other {
    One,
}

fn full(c: Color) i32 {
    switch c {
        Color::Red => return 1,
        Color::Green => return 2,
        Color::Blue => return 3,
    }
    0
}

fn missing(c: Color) i32 {
    switch c {
        Color::Red => return 1,
    }
    0
}

fn with_default(c: Color) i32 {
    switch c {
        Color::Red => return 1,
        _ => return 0,
    }
    0
}

fn bad(c: Color) i32 {
    switch c {
        Other::One => return 1,
        Color::Missing => return 2,
        _ => return 0,
    }
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("non-exhaustive enum switch"))
            .count(),
        1
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("switch pattern"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown enum variant"))
    );
}

#[test]
fn infers_switch_pattern_numeric_literals_from_target_type() {
    let checked = pipeline(
        r#"
fn classify(value: usize) i32 {
    switch value {
        0 => return 0,
        1 + 1 => return 2,
        _ => return 3,
    }
    4
}

fn bad(value: u8) i32 {
    switch value {
        256 => return 1,
        _ => return 0,
    }
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("out of range for U8"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("type mismatch in switch pattern")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_implicit_enum_integer_mixing() {
    let checked = pipeline(
        r#"
enum Color: u8 {
    Red,
    Green,
}

fn main() i32 {
    var same = Color::Red == Color::Green;
    var n: i32 = Color::Red;
    var explicit: i32 = Color::Red as i32;
    var bad_add = Color::Red + Color::Green;
    var bad_order = Color::Red < Color::Green;
    if same { explicit } else { n }
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("arithmetic operator"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("ordered comparison"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("explicit"))
    );
}

#[test]
fn checks_open_enum_integer_casts_and_switch_exhaustiveness() {
    let checked = pipeline(
        r#"
enum Flag {
    A,
    B,
    _,
}

enum Closed {
    A,
    B,
}

fn open_cast() Flag {
    3 as Flag
}

fn closed_cast() Closed {
    3 as Closed
}

fn missing_open_default(flag: Flag) i32 {
    switch flag {
        Flag::A => return 1,
        Flag::B => return 2,
    }
    0
}

fn with_open_default(flag: Flag) i32 {
    switch flag {
        Flag::A => return 1,
        Flag::B => return 2,
        _ => return 0,
    }
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("invalid cast"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("non-exhaustive open enum switch"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("i32 to Flag")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_shift_operators() {
    let checked = pipeline(
        r#"
fn main(flag: bool) i32 {
    var x = 1 << 3;
    var y = x >> 1;
    var z = x << flag;
    var bad = flag << 1;
    y + z + bad
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("shift operator requires integer right operand")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("shift operator requires integer left operand")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_global_initializers_and_inferred_global_types() {
    let checked = pipeline(
        r#"
var counter = 1;
var flag = true;
const limit = 10;
var bad: bool = 1;

fn main() i32 {
    counter = counter + 1;
    limit = 11;
    if flag { counter } else { limit }
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("global initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("global is const"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("global type is not available"))
    );
}

#[test]
fn checks_inline_asm_configuration() {
    let checked = pipeline(
        r#"
fn main() void {
    var ret: i64 = 0;
    @asm({
        code: "syscall",
        outputs: { rax: ret },
        inputs: { rax: 39 },
        clobbers: ["rcx", "r11", "memory"],
        options: ["volatile"],
    });
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let bad = pipeline(
        r#"
fn main() void {
    @asm({
        code: 1,
        outputs: { rax: 10 },
        clobbers: [1],
        options: ["unknown"],
        extra: 0,
    });
}
"#,
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("field `code`")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("inline assembly output")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("clobbers")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown `@asm` option")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown `@asm` field")),
        "{:?}",
        bad.diagnostics
    );

    let bare_option = pipeline(
        r#"
fn main() void {
    var volatile = 0;
    @asm({
        code: "nop",
        options: [volatile],
    });
}
"#,
    );
    assert!(
        bare_option
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("string literals")),
        "{:?}",
        bare_option.diagnostics
    );

    let aggregate_operand = pipeline(
        r#"
struct Pair { x: i64 }

fn main() void {
    var pair: Pair = { x: 1 };
    @asm({
        code: "nop",
        inputs: { rax: pair },
        outputs: { rax: pair },
    });
}
"#,
    );
    assert!(
        aggregate_operand
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("inline assembly")
                && diagnostic.message.contains("aggregate type"))
            .count()
            >= 2,
        "{:?}",
        aggregate_operand.diagnostics
    );
}
