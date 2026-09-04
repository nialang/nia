// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn caller_location_is_available_in_const_initializers() {
    let root = temp_dir("caller_location_const_initializer");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"using std::{SourceLocation, callerLocation};

const HERE: SourceLocation = callerLocation();

fn main() u32 {
    HERE.line()
}
"#,
    )
    .expect("write const caller-location source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = source_module_ir(&output, "main.nia");
    // `callerLocation()` in a const initializer is folded into the aggregate:
    // the call expression is on line 3 and column 30 in this fixture.
    assert!(main_ir.contains("store i32 3"), "{main_ir}");
    assert!(main_ir.contains("store i32 30"), "{main_ir}");
}

#[test]
fn tracked_const_functions_forward_their_outer_callsite() {
    let root = temp_dir("tracked_const_caller_location");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"using std::{SourceLocation, callerLocation};

@[trackCaller]
const fn capture() SourceLocation {
    callerLocation()
}

const HERE: SourceLocation = capture();

fn main() u32 {
    HERE.line()
}
"#,
    )
    .expect("write tracked const caller-location source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = source_module_ir(&output, "main.nia");
    assert!(main_ir.contains("store i32 8"), "{main_ir}");
}
