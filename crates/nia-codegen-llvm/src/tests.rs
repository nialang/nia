// SPDX-License-Identifier: GPL-3.0-or-later
mod common;

mod basic;
mod calls_and_slices;
mod control_flow;
mod cross_module;
mod function_ir;
mod imports_and_aggregates;
mod layouts_and_literals;
mod low_level_and_const;
mod operators;
mod smoke;
mod structural_extensions;
mod traits;
mod values_and_assignments;
mod void_and_empty;

#[test]
fn missing_declaration_module_is_reported_as_backend_diagnostic() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let missing = module_ids.allocate();
    let modules = std::sync::Arc::new(nia_backend_ir::BackendModuleStore::new([]));
    let (index, _publisher) = crate::program_index::ProgramIndex::new(
        modules,
        std::sync::Arc::new(nia_ty::TypeStore::new()),
    );

    let diagnostics = super::validate_declaration_module(missing, &index)
        .expect_err("missing declaration module must be diagnosed");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_str(), "I0300");
    assert!(diagnostics[0].summary.contains("missing module"));
}

#[test]
fn readiness_coordinator_retries_units_only_after_exact_owner_publication() {
    let root = common::temp_dir("readiness_coordinator_retries_exact_owner");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module geom;
using entry::geom;

fn main() i32 {
    let mut point = geom::Point { x: 40, y: 2 };
    point.x + point.y
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("geom.nia"),
        r#"
pub struct Point {
    x: i32,
    y: i32,
}
"#,
    )
    .expect("write geom source");
    let codegen = common::codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let main_id = codegen
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main backend module")
        .id;
    let geom_id = codegen
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("geom.nia"))
        .expect("geom backend module")
        .id;
    let mut coordinator = crate::readiness::CodegenReadinessCoordinator::new(
        codegen.backend_lowering.program.module_store(),
        std::sync::Arc::clone(&codegen.type_store),
        std::sync::Arc::clone(&codegen.backend_lowering.owner_directory),
    );

    assert!(coordinator.publish(main_id).is_empty());
    let ready = coordinator.publish(geom_id);
    assert_eq!(ready.len(), 1);
    let crate::readiness::CodegenPartitionPreparation::Ready(prepared) = &ready[0] else {
        panic!("healthy pending unit became invalid")
    };
    assert!(matches!(
        prepared.partition.id,
        nia_backend_ir::CodegenUnitId::SourceModule { module_id, .. } if module_id == main_id
    ));
    assert_eq!(
        prepared.declarations.dependencies.modules(),
        &[main_id, geom_id]
    );
    let _ = coordinator.finish();
}

#[test]
fn closure_codegen_materializes_direct_entry_calls() {
    let root = common::temp_dir("closure_codegen_materialization_boundary");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    callback(1)
}
"#,
    )
    .expect("write test source");

    let codegen = common::codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = common::emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let module = codegen
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main backend module");
    let entry_symbol = &module.closure_entries[0].symbol;
    let ir = common::source_module_ir(&output, "main.nia");

    assert!(
        ir.contains(entry_symbol),
        "LLVM IR omitted generated closure entry `{entry_symbol}`: {ir}"
    );
    assert!(
        ir.contains("closure.call") || ir.contains("call i32"),
        "LLVM IR omitted direct closure call: {ir}"
    );
}

#[test]
fn curried_closure_codegen_returns_a_nested_capturing_state() {
    let root = common::temp_dir("curried_closure_codegen");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    let make = \x: i32, y: i32 -> \[x, y] z: i32 -> x * y + z;
    let add = make(2, 3);
    add(4)
}
"#,
    )
    .expect("write test source");

    let codegen = common::codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let module = codegen
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main backend module");
    assert_eq!(module.closure_entries.len(), 2);
    let outer = module
        .closure_entries
        .iter()
        .find(|entry| entry.params.len() == 2)
        .expect("outer curried closure entry");
    assert!(outer.function_body.locals.iter().all(|local| {
        local.kind != nia_function_ir::FunctionLocalKind::Param
            || local.id == outer.state_param
            || outer.params.contains(&local.id)
    }));

    let output = common::emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = common::source_module_ir(&output, "main.nia");
    for entry in &module.closure_entries {
        assert!(
            ir.contains(&entry.symbol),
            "LLVM IR omitted curried closure entry `{}`: {ir}",
            entry.symbol
        );
    }
}

#[test]
fn callable_view_codegen_materializes_dynamic_dispatch() {
    let root = common::temp_dir("callable_view_codegen_dynamic_dispatch");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    let view: &Fn(i32) i32 = &callback;
    view(1)
}
"#,
    )
    .expect("write test source");

    let codegen = common::codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = common::emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = common::source_module_ir(&output, "main.nia");

    assert!(
        ir.contains("callable.entry"),
        "LLVM IR omitted callable entry metadata: {ir}"
    );
    assert!(
        ir.contains("callable.call"),
        "LLVM IR omitted callable indirect dispatch: {ir}"
    );
}

#[test]
fn callable_effect_codegen_dispatches_functions_and_mutable_captures() {
    let root = common::temp_dir("callable_effect_codegen_dynamic_dispatch");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn ignore(value: i32) () {
    _ = value;
    ()
}

fn apply(callback: &mut Fn(i32) (), value: i32) () {
    callback(value);
}

fn main() i32 {
    let mut total = 0;
    apply(&ignore, 1);
    apply(&mut \[&mut total] value: i32 -> {
        total.* += value;
        ()
    }, 2);
    total
}
"#,
    )
    .expect("write test source");

    let codegen = common::codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = common::emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = common::source_module_ir(&output, "main.nia");
    assert!(
        ir.contains("callable.is_function")
            && ir.contains("callable.function:")
            && ir.contains("callable.closure:")
            && ir.contains("call void %callable.entry(i32")
            && ir.contains("call void %callable.entry(ptr %callable.context, i32"),
        "LLVM IR omitted effect-call dynamic dispatch: {ir}"
    );
}

#[test]
fn function_pointer_callable_codegen_materializes_function_branch() {
    let root = common::temp_dir("function_pointer_callable_codegen");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn add(value: i32) i32 { value + 1 }

fn main() i32 {
    let pointer: &fn(i32) i32 = &add;
    let callable: &Fn(i32) i32 = pointer;
    callable(2)
}
"#,
    )
    .expect("write test source");
    let codegen = common::codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = common::emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = common::source_module_ir(&output, "main.nia");
    assert!(
        ir.contains("callable.is_function"),
        "LLVM IR omitted function branch: {ir}"
    );
    assert!(
        ir.contains("callable.function.call"),
        "LLVM IR omitted function callable call: {ir}"
    );
}

#[test]
fn callable_view_codegen_passes_indirect_error_union_return_storage() {
    let root = common::temp_dir("callable_view_codegen_indirect_error_union_return");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main(flag: bool) bool!i32 {
    let callback = \value: i32 -> {
        if value == 1 {
            !42
        } else {
            true!
        }
    };
    let view: &Fn(i32) bool!i32 = &callback;
    view(if flag { 1 } else { 0 })
}
"#,
    )
    .expect("write test source");

    let codegen = common::codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = common::emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = common::source_module_ir(&output, "main.nia");

    assert!(
        ir.contains("callable.function:")
            && ir.contains("callable.closure:")
            && ir.contains("(ptr %0, i32")
            && ir.contains("(ptr %0, ptr %callable.context, i32"),
        "LLVM IR omitted callable indirect return storage: {ir}"
    );
}

#[test]
fn closure_function_pointer_codegen_materializes_adapter() {
    let root = common::temp_dir("closure_function_pointer_codegen_adapter");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn apply[T](value: T) T {
    let callback = \inner: T -> { inner };
    let pointer: &fn(T) T = &callback;
    pointer(value)
}

fn main() i32 {
    apply[i32](7)
}
"#,
    )
    .expect("write test source");

    let codegen = common::codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let module = codegen
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main backend module");
    let entry = module
        .closure_entries
        .iter()
        .find(|entry| {
            matches!(
                entry.key.owner,
                nia_backend_ir::BackendClosureEntryOwner::FunctionInstance(_)
            )
        })
        .expect("generic instance closure entry");
    let output = common::emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = common::source_module_ir(&output, "main.nia");
    let adapter_symbol = format!("{}__fn_adapter", entry.symbol);

    assert!(
        ir.contains(&adapter_symbol),
        "LLVM IR omitted no-capture adapter `{adapter_symbol}`: {ir}"
    );
    assert!(
        ir.contains("closure.fn.call"),
        "LLVM IR omitted adapter entry call: {ir}"
    );
}

#[test]
fn generic_closure_codegen_uses_the_concrete_instance_entry() {
    let root = common::temp_dir("generic_closure_codegen_instance_entry");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn apply[T](value: T) T {
    let callback = \[value] -> { value };
    callback()
}

fn main() i32 {
    apply[i32](7)
}
"#,
    )
    .expect("write test source");

    let codegen = common::codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let module = codegen
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main backend module");
    let entry = module
        .closure_entries
        .iter()
        .find(|entry| {
            matches!(
                entry.key.owner,
                nia_backend_ir::BackendClosureEntryOwner::FunctionInstance(_)
            )
        })
        .expect("generic instance closure entry");
    let output = common::emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = common::source_module_ir(&output, "main.nia");

    assert!(
        ir.contains(&entry.symbol),
        "LLVM IR omitted concrete generic closure entry `{}`: {ir}",
        entry.symbol
    );
    assert!(
        ir.contains("closure.call"),
        "LLVM IR omitted generic closure entry call: {ir}"
    );
}
