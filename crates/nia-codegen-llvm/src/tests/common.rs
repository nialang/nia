// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use crate::{
    LlvmCodegenOptions, LlvmCodegenOutput, LlvmObjectOutput, catch_llvm_codegen_ice,
};
pub(super) use nia_backend_ir::{
    BackendConstFacts, BackendEnum, BackendEnumVariant, BackendField, BackendFunction,
    BackendFunctionInstance, BackendGlobal, BackendLayouts, BackendModule, BackendParam,
    BackendProgram, BackendStruct, BackendTraitObjectVtable, BackendTraitObjectVtableEntry,
    BackendTraitObjectVtableFunction, BackendTraitObjectVtableKey, BackendUnion, CodegenUnitId,
};
pub(super) use nia_body_ir::{
    LocalName, TypedBody, TypedExpr, TypedExprKind, TypedLocal, TypedLocalKind,
};
pub(super) use nia_diagnostic::{Diagnostic, DiagnosticCategory, codes};
pub(super) use nia_function_ir::{
    FunctionBlock, FunctionBlockId, FunctionBody, FunctionCallee, FunctionExpr, FunctionExprKind,
    FunctionFieldInit, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionScope,
    FunctionScopeId, FunctionTerminator,
};
pub(super) use nia_ids::{
    BuiltinTraitMethod, ConstExprId, DefId, GlobalConstExprId, GlobalDefId, LocalId, ModuleId,
};
pub(super) use nia_layout::{FieldLayout, StructLayout, TypeLayout};
pub(super) use nia_mangle::mangle_symbol_id;
pub(super) use nia_opt::NiaOptimizationLevel;
pub(super) use nia_span::Span;
pub(super) use nia_static_ir::{StaticFieldInit, StaticInit};
pub(super) use nia_symbol::{SymbolId, known, stable_hash};
pub(super) use nia_ty::{ArrayLenTy, BuiltinTrait, PrimitiveTy, TraitId, TyKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

pub(super) fn local_name(text: &str) -> LocalName {
    LocalName::named(sym(text))
}

pub(super) fn has_internal_diagnostic(
    diagnostics: &[Diagnostic],
    code: codes::DiagnosticCodeDef,
    text: &str,
) -> bool {
    diagnostics.iter().any(|diagnostic| {
        diagnostic.category == DiagnosticCategory::Internal
            && diagnostic.code.as_str() == code.as_str()
            && diagnostic.summary.contains(text)
            && diagnostic.primary_span().is_some()
    })
}

pub(super) fn codegen_program(entry_path: impl Into<String>) -> nia_compiler_query::CodegenProgram {
    codegen_program_with_options(entry_path, NiaOptimizationLevel::default())
}

pub(super) fn codegen_program_with_options(
    entry_path: impl Into<String>,
    optimization: NiaOptimizationLevel,
) -> nia_compiler_query::CodegenProgram {
    codegen_program_request(
        nia_loader_query::LoadRequest::new(entry_path)
            .with_module_map(nia_imports::ModuleMap::new()),
        optimization,
    )
}

pub(super) fn codegen_freestanding_executable_with_options(
    entry_path: impl Into<String>,
    optimization: NiaOptimizationLevel,
) -> nia_compiler_query::CodegenProgram {
    codegen_program_request(
        nia_loader_query::LoadRequest::new(entry_path)
            .with_module_map(nia_imports::ModuleMap::new())
            .with_entry_runtime(nia_loader_query::EntryRuntime::Freestanding),
        optimization,
    )
}

fn codegen_program_request(
    request: nia_loader_query::LoadRequest,
    optimization: NiaOptimizationLevel,
) -> nia_compiler_query::CodegenProgram {
    let loader = nia_loader_query::LoaderDatabase::new(request);
    let compiler = nia_compiler_query::CompilerDatabase::new(
        nia_compiler_query::CompileRequest::new(loader.clone()).with_optimization(optimization),
    );
    compiler.codegen_program()
}

pub(super) fn emit_llvm_ir(
    lowering: &Arc<nia_backend_lower::BackendLowering>,
    type_store: &Arc<nia_ty::TypeStore>,
) -> LlvmCodegenOutput {
    crate::emit_llvm_ir(
        Arc::clone(lowering),
        Arc::clone(type_store),
        &nia_query::QuerySession::new(),
    )
}

pub(super) fn emit_llvm_ir_with_options(
    lowering: &Arc<nia_backend_lower::BackendLowering>,
    type_store: &Arc<nia_ty::TypeStore>,
    options: LlvmCodegenOptions,
) -> LlvmCodegenOutput {
    crate::emit_llvm_ir_with_options(
        Arc::clone(lowering),
        Arc::clone(type_store),
        &nia_query::QuerySession::new(),
        options,
    )
}

pub(super) fn emit_native_objects(
    lowering: &Arc<nia_backend_lower::BackendLowering>,
    type_store: &Arc<nia_ty::TypeStore>,
    options: LlvmCodegenOptions,
) -> LlvmObjectOutput {
    crate::emit_native_objects(
        Arc::clone(lowering),
        Arc::clone(type_store),
        &nia_query::QuerySession::new(),
        options,
    )
}

pub(super) fn emit_owned_llvm_ir(
    program: BackendProgram,
    type_store: nia_ty::TypeStore,
) -> LlvmCodegenOutput {
    let codegen_partitions = program.codegen_partition_plan();
    emit_llvm_ir(
        &Arc::new(nia_backend_lower::BackendLowering {
            program,
            codegen_partitions,
            optimization: nia_opt::OptimizationPolicy::default(),
            optimization_report: nia_backend_lower::BackendOptimizationReport::default(),
            diagnostics: Vec::new(),
        }),
        &Arc::new(type_store),
    )
}

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

struct Counter {
    current: i32,
    end: i32,
}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

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
    let mut total = 0;
    let mut iter = Counter { current: 0, end: 4 };
    for i in iter {
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
module facade;
module impl;
using entry::facade;

using facade::{Box, make_box, read_box};

fn main() i32 {
    let mut box: Box[i32] = make_box(40);
    read_box(& box) + facade::answer
}
"#,
                ),
                (
                    "facade.nia",
                    r#"
using entry::impl;

pub using impl::{Box, make_box, read_box, answer};
"#,
                ),
                (
                    "impl.nia",
                    r#"
pub const answer: i32 = 2;

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

static header: Header = { tag: 1, count: 2, flag: 3 };
static bytes: [3]u8 = b"ok\0";
static byte_ptr: & u8 = &bytes[0];
static mut global: i32 = 5;
static global_ptr: &i32 = &global;

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
    let mut out = 0;
    let mut i = 0usize;
    while i < xs.len() {
        out += xs[i];
        i += 1;
    }
    out
}

fn fill(xs: &mut [i32]) i32 {
    xs[0] = 9;
    xs[0]
}

fn main() i32 {
    let mut xs: [4]i32 = [1, 2, 3, 4];
    let mut part = & xs[1..=2];
    sum(part) + sum(&[5, 6]) + fill(&mut [0, 1])
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
    if ?x = value {
        x
    } or null {
        0
    }
}

fn read_error(value: i32!i32) i32 {
    if !x = value {
        x
    } or e! {
        e
    }
}

fn main() i32 {
    read_optional(maybe(true)) + read_error(wrap(false))
}
"#,
            )],
        },
        EmitSmokeCase {
            name: "cross_module_error_union_conversion_optional_payload",
            root: "main.nia",
            files: &[
                (
                    "main.nia",
                    r#"
module facade;
module convert;
using entry::facade;

fn main() i32 {
    facade::run()
}
"#,
                ),
                (
                    "facade.nia",
                    r#"
using entry::convert;

pub struct Item {
    value: i32,
}

fn read() convert::A!?Item {
    !(?Item { value: 7 })
}

pub fn run() i32 {
    if !maybe = read().as_b() {
        if ?item = maybe {
            item.value
        } or null {
            0
        }
    } or err! {
        err as i32
    }
}
"#,
                ),
                (
                    "convert.nia",
                    r#"
pub enum A: i32 {
    Bad = 1,
    _
}

pub enum B: i32 {
    Other = 2,
    _
}

fn to_b(error: A) B {
    _ = error;
    B::Other
}

extend[T] A!T {
    pub fn as_b(self) B!T {
        if !value = self {
            !value
        } or err! {
            to_b(err)!
        }
    }
}
"#,
                ),
            ],
        },
        EmitSmokeCase {
            name: "structural_associated_function_pointers",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
type RawPtr[T] = &T;

extend[T] RawPtr[T] {
    fn is_null(self) bool {
        self as usize == 0
    }

    fn zero() usize {
        0usize
    }
}

fn main(ptr: &i32) i32 {
    let is_null: &fn(&i32) bool = & [&i32]::is_null;
    let zero: &fn() usize = & [&i32]::zero;
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
            name: "union_open_enum_and_const_lengths",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
const width: usize = 2 + 2;

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
    let mut values: [width]i32 = [10, 20, 30, 40];
    let mut bits: Bits = { i: values[0] };
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
    let id = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.push(format!(
        "nia_codegen_llvm_{name}_{}_{:?}_{id}",
        std::process::id(),
        std::thread::current().id()
    ));
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

pub(super) fn mangled_symbol(ir: &str, sigil: char, module: u32, name: &str) -> String {
    let name = expected_backend_symbol_suffix(name);
    find_mangled_symbol(ir, sigil, module, &name)
        .unwrap_or_else(|| panic!("missing mangled symbol `{sigil}nia__m{module}__d...__{name}`"))
}

pub(super) fn mangled_symbol_any_module(ir: &str, sigil: char, name: &str) -> String {
    let name = expected_backend_symbol_suffix(name);
    find_mangled_symbol_any_module(ir, sigil, &name)
        .unwrap_or_else(|| panic!("missing mangled symbol `{sigil}nia__m...__d...__{name}`"))
}

pub(super) fn assert_contains_mangled_symbol(ir: &str, sigil: char, module: u32, name: &str) {
    let _ = mangled_symbol(ir, sigil, module, name);
}

pub(super) fn backend_symbol_suffix(name: &str) -> String {
    expected_backend_symbol_suffix(name)
}

pub(super) fn assert_not_contains_mangled_symbol(ir: &str, sigil: char, module: u32, name: &str) {
    let name = expected_backend_symbol_suffix(name);
    if let Some(symbol) = find_mangled_symbol(ir, sigil, module, &name) {
        panic!("unexpected mangled symbol `{symbol}` in IR:\n{ir}");
    }
}

fn expected_backend_symbol_suffix(name: &str) -> String {
    let Some((base, rest)) = name.split_once("__") else {
        return mangle_symbol_id(sym(name));
    };
    format!("{}__{rest}", mangle_symbol_id(sym(base)))
}

fn find_mangled_symbol(ir: &str, sigil: char, module: u32, name: &str) -> Option<String> {
    let prefix = format!("{sigil}nia__m{module}__d");
    find_mangled_symbol_with_prefix(ir, &prefix, name)
}

fn find_mangled_symbol_any_module(ir: &str, sigil: char, name: &str) -> Option<String> {
    let prefix = format!("{sigil}nia__m");
    for (start, _) in ir.match_indices(&prefix) {
        let token = symbol_token(&ir[start..]);
        let Some((_, rest)) = token.split_once("__d") else {
            continue;
        };
        let Some((_, symbol_name)) = rest.split_once("__") else {
            continue;
        };
        if symbol_name == name {
            return Some(token.to_string());
        }
    }
    None
}

fn find_mangled_symbol_with_prefix(ir: &str, prefix: &str, name: &str) -> Option<String> {
    for (start, _) in ir.match_indices(&prefix) {
        let token = symbol_token(&ir[start..]);
        let rest = if let Some(rest) = token.strip_prefix(prefix) {
            rest
        } else {
            continue;
        };
        let Some((_, symbol_name)) = rest.split_once("__") else {
            continue;
        };
        if symbol_name == name {
            return Some(token.to_string());
        }
    }
    None
}

fn symbol_token(text: &str) -> &str {
    let end = text
        .char_indices()
        .find_map(|(index, ch)| (!is_symbol_char(ch)).then_some(index))
        .unwrap_or(text.len());
    &text[..end]
}

fn is_symbol_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '@' | '%')
}
