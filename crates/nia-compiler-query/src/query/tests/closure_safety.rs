// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

const ESCAPE_MESSAGE: &str = "stack-backed callable view cannot be";
const CAPTURED_ADDRESS_MESSAGE: &str = "closure state capturing a local address cannot be";

fn closure_diagnostics(program: &CheckedProgramAnalysis) -> Vec<&ProgramDiagnostic> {
    program
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.diagnostic.summary.contains(ESCAPE_MESSAGE))
        .collect()
}

fn captured_address_diagnostics(program: &CheckedProgramAnalysis) -> Vec<&ProgramDiagnostic> {
    program
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains(CAPTURED_ADDRESS_MESSAGE)
        })
        .collect()
}

#[test]
fn captured_local_address_is_safe_while_closure_state_stays_local() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn main() i32 {
    let value = 41;
    let callback = [ptr = &value]() i32 { ptr.* + 1 };
    callback()
}
"#,
    );
    let checked = query_db(fixture.program()).expect_get(CheckedProgramQuery);

    assert!(
        captured_address_diagnostics(&checked).is_empty(),
        "local closure state produced diagnostics: {:?}",
        checked.diagnostics
    );
}

#[test]
fn captured_address_escape_propagates_across_function_summaries() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn store[T](slot: &mut T, value: T) () {
    slot.* = value;
}

fn captureAndStore(ptr: &i32) () {
    let callback = [ptr]() i32 { ptr.* };
    let mut slot = callback;
    store(&mut slot, callback);
}

fn main() () {
    let value = 1;
    captureAndStore(&value);
}
"#,
    );
    let checked = query_db(fixture.program()).expect_get(CheckedProgramQuery);
    let diagnostics = captured_address_diagnostics(&checked);

    assert_eq!(diagnostics.len(), 1, "{:?}", checked.diagnostics);
    assert!(
        diagnostics[0]
            .diagnostic
            .summary
            .contains("passed to a call that may retain it")
    );
}

#[test]
fn ordinary_local_pointer_flow_is_not_owned_by_closure_escape_analysis() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn store(slot: &mut &i32, value: &i32) () {
    slot.* = value;
}

fn main() i32 {
    let first = 1;
    let second = 2;
    let mut slot = &first;
    store(&mut slot, &second);
    slot.*
}
"#,
    );
    let checked = query_db(fixture.program()).expect_get(CheckedProgramQuery);

    assert!(
        captured_address_diagnostics(&checked).is_empty(),
        "ordinary pointer flow produced closure diagnostics: {:?}",
        checked.diagnostics
    );
}

#[test]
fn local_callable_view_use_does_not_escape() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn invoke(callback: &Fn(i32) i32, value: i32) i32 {
    callback(value)
}

fn main(base: i32) i32 {
    let callback = [base](value: i32) i32 { base + value };
    let view: &Fn(i32) i32 = &callback;
    invoke(view, 1)
}
"#,
    );
    let checked = query_db(fixture.program()).expect_get(CheckedProgramQuery);

    assert!(
        closure_diagnostics(&checked).is_empty(),
        "safe local use produced diagnostics: {:?}",
        checked.diagnostics
    );
}

#[test]
fn direct_and_aggregate_returns_reject_stack_backed_views() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn direct(base: i32) &Fn(i32) i32 {
    let callback = [base](value: i32) i32 { base + value };
    &callback
}

fn aggregate(base: i32) (&Fn(i32) i32, i32) {
    let callback = [base](value: i32) i32 { base + value };
    let view: &Fn(i32) i32 = &callback;
    (view, 1)
}
"#,
    );
    let checked = query_db(fixture.program()).expect_get(CheckedProgramQuery);

    assert_eq!(
        closure_diagnostics(&checked).len(),
        2,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn direct_call_summaries_propagate_return_and_escape_behavior() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn identity(callback: &Fn(i32) i32) &Fn(i32) i32 {
    callback
}

extern fn retain(callback: &Fn(i32) i32) ();

fn store(slot: &mut &Fn(i32) i32, callback: &Fn(i32) i32) () {
    slot.* = callback;
}

fn returned(base: i32) &Fn(i32) i32 {
    let callback = [base](value: i32) i32 { base + value };
    identity(&callback)
}

fn passed_to_unknown(base: i32) () {
    let callback = [base](value: i32) i32 { base + value };
    retain(&callback);
}

fn stored_indirectly(base: i32) () {
    let fallback = [base](value: i32) i32 { base - value };
    let callback = [base](value: i32) i32 { base + value };
    let mut slot: &Fn(i32) i32 = &fallback;
    store(&mut slot, &callback);
}
"#,
    );
    let checked = query_db(fixture.program()).expect_get(CheckedProgramQuery);
    let diagnostics = closure_diagnostics(&checked);

    assert_eq!(diagnostics.len(), 3, "{:?}", checked.diagnostics);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.diagnostic.summary.contains("cannot be returned") })
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("passed to a call that may retain it"))
            .count(),
        2
    );
}

#[test]
fn inner_block_view_cannot_flow_into_an_outer_local() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn main(base: i32) i32 {
    let outer = [base](value: i32) i32 { base + value };
    let mut view: &Fn(i32) i32 = &outer;
    {
        let inner = [base](value: i32) i32 { base - value };
        view = &inner;
    };
    view(1)
}
"#,
    );
    let checked = query_db(fixture.program()).expect_get(CheckedProgramQuery);
    let diagnostics = closure_diagnostics(&checked);

    assert_eq!(diagnostics.len(), 1, "{:?}", checked.diagnostics);
    assert!(
        diagnostics[0]
            .diagnostic
            .summary
            .contains("moved beyond its closure state's lexical scope")
    );
}

#[test]
fn closure_safety_diagnostics_keep_owner_path_and_reach_all_program_products() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "pub fn main() i32 { 0 }");
    let entry = fixture.entry_id();
    fixture.add_child(
        entry,
        "child",
        "child.nia",
        r#"
pub fn leak(base: i32) &Fn(i32) i32 {
    let callback = [base](value: i32) i32 { base + value };
    &callback
}
"#,
    );

    let db = query_db(fixture.program());
    let checked = db.expect_get(CheckedProgramQuery);
    let diagnostics = closure_diagnostics(&checked);
    assert_eq!(diagnostics.len(), 1, "{:?}", checked.diagnostics);
    assert_eq!(diagnostics[0].path.as_str(), "child.nia");

    let mut executable = fixture.program();
    executable.runtime = RuntimeModel::FreestandingExecutable;
    let executable_db = query_db(executable);
    let entry_checked = executable_db.expect_get(EntryCheckedProgramQuery);
    assert!(closure_diagnostics(&entry_checked).is_empty());

    let executable_fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
pub fn main(base: i32) &Fn(i32) i32 {
    let callback = [base](value: i32) i32 { base + value };
    &callback
}
"#,
    );
    let mut executable = executable_fixture.program();
    executable.runtime = RuntimeModel::FreestandingExecutable;
    let executable_db = query_db(executable);
    let preparation = executable_db.expect_get(CodegenPreparationQuery);
    assert_eq!(
        preparation
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.diagnostic.summary.contains(ESCAPE_MESSAGE))
            .count(),
        1,
        "{:?}",
        preparation.diagnostics
    );
}
