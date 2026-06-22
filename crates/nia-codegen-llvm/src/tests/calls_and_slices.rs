// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_assignment_to_field_through_indexed_mut_slice() {
    let root = temp_dir("emits_assignment_to_field_through_indexed_mut_slice");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Item {
    state: i32,
}

fn set(items: &mut [Item], index: usize, state: i32) void {
    items[index].state = state;
}

fn main() i32 {
    var items: [2]Item = [
        { state: 1 },
        { state: 2 },
    ];
    set(&mut items[..], 1usize, 9);
    items[1].state
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn emits_direct_function_calls() {
    let root = temp_dir("emits_direct_function_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn add(a: i32, b: i32) i32 {
    a + b
}

fn main() i32 {
    add(20, 22)
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define i32 @nia__m0__d"));
    assert!(ir.contains("call i32 @nia__m0__d"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_extern_function_definitions_with_unmangled_symbols() {
    let root = temp_dir("emits_extern_function_definitions_with_unmangled_symbols");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn add(a: i32, b: i32) i32 {
    a + b
}

fn main() i32 {
    add(40, 2)
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define i32 @add("), "{ir}");
    assert_not_contains_mangled_symbol(ir, '@', 0, "add");
    assert!(ir.contains("call i32 @add"), "{ir}");
}

#[test]
fn emits_extern_struct_return_calls_with_c_abi_direct_return() {
    let root = temp_dir("emits_extern_struct_return_calls_with_c_abi_direct_return");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern struct NiaString {
    ptr: &u8,
    len: usize,
}

extern fn make_string() NiaString;

fn main() NiaString {
    make_string()
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let nia_string = mangled_symbol(ir, '%', 0, "NiaString");
    assert!(
        ir.contains(&format!("declare {nia_string} @make_string()")),
        "{ir}"
    );
    assert!(
        ir.contains(&format!("call {nia_string} @make_string()")),
        "{ir}"
    );
    assert!(ir.contains(&format!("store {nia_string}")), "{ir}");
    assert!(!ir.contains("call void @make_string"), "{ir}");
}

#[test]
fn emits_freestanding_start_entry_as_extern_start_calling_entry_main() {
    let root = temp_dir("emits_freestanding_start_entry_as_extern_start_calling_entry_main");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    )
    .expect("write test source");

    let checked = check_freestanding_executable_with_options(
        main.to_string_lossy().into_owned(),
        NiaOptimizationLevel::default(),
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(ir.contains("define void @_start("), "{ir}");
    assert!(ir.contains("define void @nia__m0__"), "{ir}");
    assert!(ir.contains("call void @nia__m0__"), "{ir}");
}

#[test]
fn emits_std_file_writer_through_process_io_capability() {
    let root = temp_dir("emits_std_file_writer_through_process_io_capability");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    var buffer: [0]u8 = [];
    var stdout = io::FileWriter::stdout(init.io(), &mut buffer[..]);
    if let !ok = stdout.write_all(b"nia\n") {
        _ = ok;
    } else error! {
        return (1 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let checked = check_freestanding_executable_with_options(
        main.to_string_lossy().into_owned(),
        NiaOptimizationLevel::default(),
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(ir.contains("syscall"), "{ir}");
    assert!(ir.contains("write_some"), "{ir}");
    assert!(ir.contains("FileWriter"), "{ir}");
}

#[test]
fn emits_std_buffered_file_writer_flush_through_process_io() {
    let root = temp_dir("emits_std_buffered_file_writer_flush_through_process_io");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    var buffer: [64]u8 = [0; 64];
    var raw_buffer: [0]u8 = [];
    var raw = io::FileWriter::stdout(init.io(), &mut raw_buffer[..]);
    var stdout = io::BufferedWriter[io::FileWriter]::init(&mut raw, &mut buffer[..]);
    if let !ok = stdout.write_all(b"nia\n") {
        _ = ok;
    } else error! {
        return (1 as process::ExitCode)!;
    }
    if let !ok = stdout.flush() {
        _ = ok;
    } else error! {
        return (2 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let checked = check_freestanding_executable_with_options(
        main.to_string_lossy().into_owned(),
        NiaOptimizationLevel::default(),
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(ir.contains("BufferedWriter"), "{ir}");
    assert!(ir.contains("flush"), "{ir}");
    assert!(ir.contains("write_some"), "{ir}");
}

#[test]
fn emits_ref_receiver_method_with_struct_arg_and_nested_tagged_payload() {
    let root = temp_dir("emits_ref_receiver_method_with_struct_arg_and_nested_tagged_payload");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Layout {
    len: usize,
    align: usize,
}

struct Z {}

enum Error: i32 {
    Bad = 1,
    _
}

extend Layout {
    fn init(len: usize, align: usize) Error!Layout {
        !{ len: len, align: align }
    }

    fn len(&self) usize {
        self.len
    }
}

extend Z {
    fn init() Z {
        {}
    }

    fn alloc(&self, layout: Layout) Error!?Layout {
        if layout.len() == 0 {
            !null
        } else {
            !(?layout)
        }
    }
}

fn main() i32 {
    var z = Z::init();
    var layout: Layout;
    if let !value = Layout::init(7, 1) {
        layout = value;
    } else error! {
        return 1;
    }
    if let !maybe = z.alloc(layout) {
        if let ?value = maybe {
            return value.len() as i32;
        } else null {
            return 2;
        }
    } else error! {
        return 3;
    }
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call void @nia__m0__d"), "{ir}");
    assert!(ir.contains("tagged.payload"), "{ir}");
}

#[test]
fn emits_slice_readruction_len_ptr_and_indexing() {
    let root = temp_dir("emits_slice_readruction_len_ptr_and_indexing");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn first(xs: & [i32]) i32 {
    xs[0]
}

fn main() i32 {
    var xs: [4]i32 = [1, 2, 3, 4];
    var s = & xs[1..=2];
    var p = s.get_ptr_read();
    var single = & p[..];
    first(s) + s.len() as i32 + single.len() as i32
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("insertvalue"));
    assert!(ir.contains("extractvalue"));
    assert!(ir.contains("getelementptr"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_get_ptr_methods_on_slice_parameters() {
    let root = temp_dir("emits_get_ptr_methods_on_slice_parameters");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn read_ptr(xs: &[u8]) &u8 {
    xs.get_ptr_read()
}

fn write_ptr(xs: &mut [u8]) &mut u8 {
    xs.get_ptr()
}

fn main() usize {
    var ro: [2]u8 = [1, 2];
    var rw: [2]u8 = [3, 4];
    var read = read_ptr(&ro[..]);
    var write = write_ptr(&mut rw[..]);
    read.* as usize + write.* as usize
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn emits_array_to_slice_coercions() {
    let root = temp_dir("emits_array_to_slice_coercions");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn first(xs: & [i32]) i32 {
    xs[0]
}

fn first_byte(xs: & [u8]) i32 {
    xs[0] as i32
}

fn overwrite(xs: &mut [i32]) i32 {
    xs[1] = 9;
    xs[1]
}

fn main() i32 {
    var xs: [3]i32 = [1, 2, 3];
    var borrow = & xs[..];
    var literal: & [i32] = &[4, 5, 6];
    let bytes = b"hi";
    first(borrow) + first(literal) + first(&xs) + first(&[7, 8]) + first_byte(bytes) + overwrite(&mut xs) + overwrite(&mut [6, 7])
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("@.nia.static.array"), "{ir}");
    assert!(ir.contains("insertvalue"), "{ir}");
    assert!(ir.contains("getelementptr"), "{ir}");
}

#[test]
fn emits_zero_length_array_slice_without_indexing_empty_storage() {
    let root = temp_dir("emits_zero_length_array_slice_without_indexing_empty_storage");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() usize {
    var bytes: [0]u8 = [];
    var slice = &mut bytes[..];
    slice.len()
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("%zst.local = alloca i8"), "{ir}");
    assert!(!ir.contains("getelementptr {}, ptr %zst.local"), "{ir}");
}

#[test]
fn emits_global_string_pointer_call() {
    let root = temp_dir("emits_global_string_pointer_call");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn puts(s: & u8) i32;
let hello: [6]u8 = b"hello\0".*;

fn main() i32 {
    _ = puts(&hello[0]);
    0
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("declare i32 @puts"));
    assert!(ir.contains("c\"hello\\00\""));
    assert!(ir.contains("call i32 @puts"));
    assert!(ir.contains("@nia__m0__d"));
}

#[test]
fn emits_address_of_checked_places_from_function_ir() {
    let root = temp_dir("emits_address_of_checked_places_from_function_ir");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    a: i32,
    b: i32,
}

fn read(ptr: & i32) i32 {
    ptr.*
}

fn main(i: usize) i32 {
    var pair: Pair = { a: 10, b: 20 };
    var xs: [2]i32 = [30, 40];
    read(& pair.b) + read(& xs[i])
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("getelementptr"), "{ir}");
    let read = mangled_symbol(ir, '@', 0, "read");
    assert!(ir.contains(&format!("call i32 {read}")), "{ir}");
}

#[test]
fn emits_explicit_byte_string_first_element_pointers() {
    let root = temp_dir("emits_explicit_byte_string_first_element_pointers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn puts(s: & u8) i32;

fn first(ptr: &mut u8) i32 {
    ptr.* = b'J';
    ptr.* as i32
}

fn main() i32 {
    var mutable: [8]u8 = b"mutable\0".*;
    let hello = b"hello\0";
    let world = b"world\0";
    let multiline = (
        b\\multi
        \\line
    );
    var direct: & u8 = &(hello.*[0]);
    var writable: &mut u8 = &mut mutable[0];
    _ = puts(&(world.*[0]));
    _ = puts(&(multiline.*[0]));
    first(writable) + direct.* as i32
}
"#,
    )
    .expect("write test source");

    let checked = check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("declare i32 @puts"));
    assert!(ir.contains("@.nia.static.array"));
    assert!(ir.contains("getelementptr"));
    assert!(ir.contains("call i32 @puts"));
    assert!(ir.contains("[6 x i8] c\"hello\\00\""), "{ir}");
    assert!(ir.contains("[8 x i8] c\"mutable\\00\""), "{ir}");
}
