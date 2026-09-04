// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_lexical_and_forwarded_caller_locations() {
    let root = temp_dir("emits_lexical_and_forwarded_caller_locations");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"using std::{SourceLocation, callerLocation};

@[trackCaller]
fn leaf() SourceLocation {
    callerLocation()
}

@[trackCaller]
fn middle() SourceLocation {
    leaf()
}

fn lexical() SourceLocation {
    callerLocation()
}

fn main() u32 {
    middle().line() + lexical().line()
}
"#,
    )
    .expect("write caller-location source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(ir.contains("@nia__source_location__"), "{ir}");
    assert!(ir.contains("@nia__source_file__"), "{ir}");
    assert!(ir.contains(main.to_string_lossy().as_ref()), "{ir}");

    let leaf = mangled_symbol(&ir, '@', "leaf");
    let middle = mangled_symbol(&ir, '@', "middle");
    let leaf_definition = ir
        .lines()
        .find(|line| line.starts_with("define ") && line.contains(&format!("{leaf}(")))
        .expect("leaf definition");
    let middle_definition = ir
        .lines()
        .find(|line| line.starts_with("define ") && line.contains(&format!("{middle}(")))
        .expect("middle definition");
    assert!(leaf_definition.contains("(ptr "), "{leaf_definition}");
    assert!(middle_definition.contains("(ptr "), "{middle_definition}");
    assert!(
        ir.lines()
            .any(|line| { line.contains("call ") && line.contains(&format!("{leaf}(ptr %")) }),
        "tracked calls must forward the incoming pointer: {ir}",
    );
}

#[test]
fn embeds_logical_source_identity_instead_of_physical_path() {
    let root = temp_dir("logical_caller_location_identity");
    let main = root.join("physical-main.nia");
    std::fs::write(
        &main,
        r#"using std::callerLocation;

fn main() u32 {
    callerLocation().line()
}
"#,
    )
    .expect("write logical identity source");

    let logical = "package:example/src/main.nia";
    let request = nia_loader_query::LoadRequest::from_source_path(
        nia_source::SourcePath::with_identity(main.to_string_lossy(), logical),
    )
    .with_module_map(nia_imports::ModuleMap::new());
    let codegen = codegen_program_request(request, nia_opt::NiaOptimizationLevel::O0);
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let embedded_file = ir
        .lines()
        .find(|line| line.starts_with("@nia__source_file__"))
        .expect("embedded source identity global");
    assert!(embedded_file.contains(logical), "{embedded_file}");
    assert!(
        !embedded_file.contains(main.to_string_lossy().as_ref()),
        "{embedded_file}"
    );
}

#[test]
fn trait_methods_inherit_and_dispatch_tracked_caller_abi() {
    let root = temp_dir("trait_methods_inherit_tracked_caller_abi");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"using std::{SourceLocation, callerLocation};

trait Located {
    @[trackCaller]
    fn location(&self) SourceLocation;
}

struct Value {}

extend Value : Located {
    fn location(&self) SourceLocation {
        callerLocation()
    }
}

fn throughObject(value: & Located) SourceLocation {
    value.location()
}

fn main() u32 {
    let value = Value {};
    throughObject(&value).line()
}
"#,
    )
    .expect("write tracked trait source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let implementation = mangled_symbol(&ir, '@', "location");
    let definition = ir
        .lines()
        .find(|line| line.starts_with("define ") && line.contains(&format!("{implementation}(")))
        .expect("trait implementation definition");
    assert_eq!(definition.matches("ptr ").count(), 3, "{definition}");
    assert!(
        ir.lines().any(|line| {
            line.contains("call void %") && line.contains("ptr @nia__source_location__")
        }),
        "dynamic trait call must carry a static caller pointer: {ir}",
    );
}

#[test]
fn generic_and_extension_calls_preserve_tracked_caller_abi_under_optimization() {
    let root = temp_dir("generic_extension_tracked_caller_abi");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"using std::{SourceLocation, callerLocation};

@[trackCaller]
fn genericLocation[T](value: T) SourceLocation {
    _ = value;
    callerLocation()
}

struct Value { inner: i32 }

extend Value {
    @[trackCaller]
    fn location(&self) SourceLocation {
        callerLocation()
    }
}

fn main() u32 {
    let value = Value { inner: 1 };
    genericLocation[i32](value.inner).line() + value.location().line()
}
"#,
    )
    .expect("write generic tracked source");

    for optimization in [
        nia_opt::NiaOptimizationLevel::O0,
        nia_opt::NiaOptimizationLevel::O3,
    ] {
        let codegen =
            codegen_program_with_options(main.to_string_lossy().into_owned(), optimization);
        assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
        assert!(
            codegen
                .backend_lowering
                .program
                .modules
                .iter()
                .flat_map(|module| module.function_instances.iter())
                .any(|instance| instance
                    .attributes
                    .contains(&nia_backend_ir::BackendFunctionAttribute::TrackCaller))
        );
        let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let ir = output
            .modules
            .iter()
            .map(|module| module.ir.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            ir.matches("ptr @nia__source_location__").count() >= 2,
            "{ir}"
        );
    }
}

#[test]
fn rejects_trait_implementation_that_adds_tracked_caller_abi() {
    let root = temp_dir("rejects_added_trait_track_caller");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"using std::{SourceLocation, callerLocation};

trait Located {
    fn location(&self) SourceLocation;
}

struct Value {}

extend Value : Located {
    @[trackCaller]
    fn location(&self) SourceLocation {
        callerLocation()
    }
}

fn main() () {}
"#,
    )
    .expect("write mismatched tracked trait source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .summary
            .contains("does not match the trait signature")
    }));
}
