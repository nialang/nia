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
    let mut items: [2]Item = [
        { state: 1 },
        { state: 2 },
    ];
    set(&mut items[..], 1usize, 9);
    items[1].state
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define i32 @nia__s"));
    assert!(ir.contains("call i32 @nia__s"));
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define i32 @add("), "{ir}");
    assert_not_contains_mangled_symbol(ir, '@', "add");
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let nia_string = mangled_symbol(ir, '%', "NiaString");
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

    let codegen = codegen_freestanding_executable_with_options(
        main.to_string_lossy().into_owned(),
        NiaOptimizationLevel::default(),
    );
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(ir.contains("define void @_start("), "{ir}");
    assert!(ir.contains("define void @nia__s"), "{ir}");
    assert!(ir.contains("call void @nia__s"), "{ir}");
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
    let mut buffer: [0]u8 = [];
    let mut stdout = io::FileWriter::stdout(init.io(), &mut buffer[..]);
    switch stdout.write_all(&b"nia\n") {
        !ok => {
            _ = ok;
        },
        error! => {
            return (1 as process::ExitCode)!;
        },
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_freestanding_executable_with_options(
        main.to_string_lossy().into_owned(),
        NiaOptimizationLevel::default(),
    );
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(ir.contains("syscall"), "{ir}");
    assert!(ir.contains(&backend_symbol_suffix("write_some")), "{ir}");
    assert!(ir.contains(&backend_symbol_suffix("FileWriter")), "{ir}");
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
    let mut buffer: [64]u8 = [0; 64];
    let mut raw_buffer: [0]u8 = [];
    let mut raw = io::FileWriter::stdout(init.io(), &mut raw_buffer[..]);
    let mut stdout = io::BufferedWriter[io::FileWriter]::init(&mut raw, &mut buffer[..]);
    switch stdout.write_all(&b"nia\n") {
        !ok => {
            _ = ok;
        },
        error! => {
            return (1 as process::ExitCode)!;
        },
    }
    switch stdout.flush() {
        !ok => {
            _ = ok;
        },
        error! => {
            return (2 as process::ExitCode)!;
        },
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_freestanding_executable_with_options(
        main.to_string_lossy().into_owned(),
        NiaOptimizationLevel::default(),
    );
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        ir.contains(&backend_symbol_suffix("BufferedWriter")),
        "{ir}"
    );
    assert!(ir.contains(&backend_symbol_suffix("flush")), "{ir}");
    assert!(ir.contains(&backend_symbol_suffix("write_some")), "{ir}");
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
    let mut z = Z::init();
    let mut layout: Layout;
    switch Layout::init(7, 1) {
        !value => {
            layout = value;
        },
        error! => {
            return 1;
        },
    }
    switch z.alloc(layout) {
        !maybe => {
            switch maybe {
                ?value => {
                    return value.len() as i32;
                },
                null => {
                    return 2;
                },
            }
        },
        error! => {
            return 3;
        },
    }
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call void @nia__s"), "{ir}");
    assert!(ir.contains("tagged.payload"), "{ir}");
}

#[test]
fn emits_slice_len_ptr_and_indexing() {
    let root = temp_dir("emits_slice_len_ptr_and_indexing");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn first(xs: & [i32]) i32 {
    xs[0]
}

fn main() i32 {
    let mut xs: [4]i32 = [1, 2, 3, 4];
    let mut s = & xs[1..=2];
    let mut p = s.ptr();
    let mut single = & p[..];
    first(s) + s.len() as i32 + single.len() as i32
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("insertvalue"));
    assert!(ir.contains("extractvalue"));
    assert!(ir.contains("getelementptr"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_ptr_methods_on_slice_parameters() {
    let root = temp_dir("emits_ptr_methods_on_slice_parameters");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn read_ptr(xs: &[u8]) &u8 {
    xs.ptr()
}

fn write_ptr(xs: &mut [u8]) &mut u8 {
    xs.ptr_mut()
}

fn main() usize {
    let mut ro: [2]u8 = [1, 2];
    let mut rw: [2]u8 = [3, 4];
    let mut read = read_ptr(&ro[..]);
    let mut write = write_ptr(&mut rw[..]);
    read.* as usize + write.* as usize
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
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

fn first_char(xs: & [char]) i32 {
    if xs[0] == 'r' { 1 } else { 0 }
}

fn overwrite(xs: &mut [i32]) i32 {
    xs[1] = 9;
    xs[1]
}

fn main() i32 {
    let mut xs: [3]i32 = [1, 2, 3];
    let mut borrow = & xs[..];
    let mut literal: & [i32] = &[4, 5, 6];
    let bytes = b"hi";
    first(borrow) + first(literal) + first(&xs) + first(&[7, 8]) + first(&([_]i32[10, 11, 12])) + first_byte(&bytes) + first_byte(&b"ok") + first_char(&"run") + overwrite(&mut xs) + overwrite(&mut [6, 7])
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("%bytes = alloca [2 x i8]"), "{ir}");
    assert!(ir.contains("store [2 x i8] c\"hi\""), "{ir}");
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
    let mut bytes: [0]u8 = [];
    let mut slice = &mut bytes[..];
    slice.len()
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
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
static hello: [6]u8 = b"hello\0";

fn main() i32 {
    _ = puts(&hello[0]);
    0
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("declare i32 @puts"));
    assert!(ir.contains("c\"hello\\00\""));
    assert!(ir.contains("call i32 @puts"));
    assert!(ir.contains("@nia__s"));
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
    let mut pair: Pair = { a: 10, b: 20 };
    let mut xs: [2]i32 = [30, 40];
    read(& pair.b) + read(& xs[i])
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("getelementptr"), "{ir}");
    let read = mangled_symbol(ir, '@', "read");
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
    let mut mutable: [8]u8 = b"mutable\0";
    let hello = b"hello\0";
    let world = b"world\0";
    let multiline = (
        b\\multi
        \\line
    );
    let mut direct: & u8 = &hello[0];
    let mut writable: &mut u8 = &mut mutable[0];
    _ = puts(&world[0]);
    _ = puts(&multiline[0]);
    first(writable) + direct.* as i32
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("declare i32 @puts"));
    assert!(ir.contains("getelementptr"));
    assert!(ir.contains("call i32 @puts"));
    assert!(ir.contains("[6 x i8] c\"hello\\00\""), "{ir}");
    assert!(ir.contains("[8 x i8] c\"mutable\\00\""), "{ir}");
}
