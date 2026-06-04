// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use crate::{
    LlvmCodegenOptions, catch_llvm_codegen_ice, emit_llvm_ir, emit_llvm_ir_with_options,
};
pub(super) use nia_backend_ir::{
    BackendEnum, BackendEnumVariant, BackendField, BackendFunction, BackendFunctionInstance,
    BackendGlobal, BackendLayouts, BackendModule, BackendParam, BackendProgram, BackendStruct,
    BackendTraitObjectVtable, BackendTraitObjectVtableEntry, BackendTraitObjectVtableFunction,
    BackendTraitObjectVtableKey, BackendUnion,
};
pub(super) use nia_body_ir::{TypedBody, TypedExpr, TypedExprKind, TypedLocal, TypedLocalKind};
pub(super) use nia_comptime_check::ComptimeCheck;
pub(super) use nia_function_ir::{
    FunctionBlock, FunctionBlockId, FunctionBody, FunctionCallee, FunctionExpr, FunctionExprKind,
    FunctionFieldInit, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionScope,
    FunctionScopeId, FunctionTerminator,
};
pub(super) use nia_ids::{
    BuiltinTraitMethod, ConstExprId, DefId, GlobalConstExprId, GlobalDefId, LocalId, ModuleId,
};
pub(super) use nia_layout::{FieldLayout, StructLayout, TypeLayout};
pub(super) use nia_span::Span;
pub(super) use nia_static_ir::{StaticFieldInit, StaticInit};
pub(super) use nia_ty::{ArrayLenTy, BuiltinTrait, PrimitiveTy, TraitId, TyKind};

pub(super) struct EmitSmokeCase {
    pub(super) name: &'static str,
    pub(super) root: &'static str,
    pub(super) files: &'static [(&'static str, &'static str)],
}

pub(super) fn emit_smoke_cases() -> &'static [EmitSmokeCase] {
    &[
        EmitSmokeCase {
            name: "function_flow_defer_switch",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
extern fn log(x: i32);

enum State: u8 {
    Start,
    Stop,
    _,
}

fn classify(state: State) i32 {
    defer log(1);
    switch state {
        State::Start => return 10,
        State::Stop => return 20,
        _ => return 30,
    }
    0
}

fn main() i32 {
    var total = 0;
    for i in 0..4 {
        defer log(i);
        if i == 1 {
            continue;
        }
        if i == 3 {
            break;
        }
        total += i;
    }
    classify(State::Start) + total
}
"#,
            )],
        },
        EmitSmokeCase {
            name: "generic_cross_module_using_reexports",
            root: "main.nia",
            files: &[
                (
                    "main.nia",
                    r#"
import .facade;

using facade::{Box, make_box, read_box};

fn main() i32 {
    var box: Box[i32] = make_box(40);
    read_box(& box) + facade::answer
}
"#,
                ),
                (
                    "facade.nia",
                    r#"
import .impl;

pub using impl::{Box, make_box, read_box, answer};
"#,
                ),
                (
                    "impl.nia",
                    r#"
pub comptime let answer: i32 = 2;

pub struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    pub fn get(& self) T {
        self.value
    }
}

pub fn make_box[T](value: T) Box[T] {
    { value: value }
}

pub fn read_box(box: & Box[i32]) i32 {
    box.get()
}
"#,
                ),
            ],
        },
        EmitSmokeCase {
            name: "static_data_layout_addresses",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
struct Header {
    tag: u8,
    count: i64,
    flag: u8,
}

let header: Header = { tag: 1, count: 2, flag: 3 };
let bytes = c"ok";
let byte_ptr: & u8 = & bytes[0];
var global: i32 = 5;
let global_ptr: &i32 = &global;

fn main() i32 {
    global_ptr.* + header.tag as i32 + header.flag as i32 + byte_ptr.* as i32
}
"#,
            )],
        },
        EmitSmokeCase {
            name: "slices_arrays_and_coercions",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
fn sum(xs: & [i32]) i32 {
    var out = 0;
    for i in 0usize..xs.len() {
        out += xs[i];
    }
    out
}

fn fill(xs: &mut [i32]) i32 {
    xs[0] = 9;
    xs[0]
}

fn main() i32 {
    var xs: [4]i32 = [1, 2, 3, 4];
    var part = & xs[1..=2];
    sum(part) + sum([5, 6]) + fill([0, 1])
}
"#,
            )],
        },
        EmitSmokeCase {
            name: "optional_error_union_switch_patterns",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
fn maybe(flag: bool) ?i32 {
    if flag {
        ?4
    } else {
        null
    }
}

fn wrap(flag: bool) i32!i32 {
    if flag {
        !7
    } else {
        3!
    }
}

fn read_optional(value: ?i32) i32 {
    switch value {
        ?x => x,
        null => 0,
    }
}

fn read_error(value: i32!i32) i32 {
    switch value {
        !x => x,
        e! => e,
    }
}

fn main() i32 {
    read_optional(maybe(true)) + read_error(wrap(false))
}
"#,
            )],
        },
        EmitSmokeCase {
            name: "structural_associated_function_pointers",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
type Ptr[T] = &T;

extend[T] Ptr[T] {
    fn is_null(self) bool {
        self as usize == 0
    }

    fn zero() usize {
        0usize
    }
}

fn main(ptr: &i32) i32 {
    var is_null: &fn(&i32) bool = & [&i32]::is_null;
    var zero: &fn() usize = & [&i32]::zero;
    if is_null(ptr) or [&i32]::is_null(ptr) {
        zero() as i32
    } else {
        0
    }
}
"#,
            )],
        },
        EmitSmokeCase {
            name: "union_open_enum_and_comptime_lengths",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
comptime let width: usize = 2 + 2;

union Bits {
    i: i32,
    f: f32,
}

enum Flag: u32 {
    A = 1,
    B = 2,
    _,
}

fn main(flag: Flag) i32 {
    var values: [width]i32 = [10, 20, 30, 40];
    var bits: Bits = { i: values[0] };
    switch flag {
        Flag::A => return bits.i,
        _ => return Flag::B as u32 as i32,
    }
    0
}
"#,
            )],
        },
    ]
}

pub(super) fn write_smoke_case(root: &std::path::Path, case: &EmitSmokeCase) {
    for (relative, source) in case.files {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create smoke case parent directory");
        }
        std::fs::write(path, source).expect("write smoke case source");
    }
}

pub(super) fn temp_dir(name: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("nia_codegen_llvm_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

pub(super) fn assert_substrings_in_order(haystack: &str, needles: &[&str]) {
    let mut offset = 0usize;
    for needle in needles {
        let Some(index) = haystack[offset..].find(needle) else {
            panic!("missing `{needle}` after byte offset {offset}");
        };
        offset += index + needle.len();
    }
}
