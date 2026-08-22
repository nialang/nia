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
    let ptr = &value;
    let callback = \[ptr] -> ptr.* + 1;
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
    let callback = \[ptr] -> { ptr.* };
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
    let callback = \[base] value: i32 -> { base + value };
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
    let callback = \[base] value: i32 -> { base + value };
    &callback
}

fn aggregate(base: i32) (&Fn(i32) i32, i32) {
    let callback = \[base] value: i32 -> { base + value };
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
    let callback = \[base] value: i32 -> { base + value };
    identity(&callback)
}

fn passed_to_unknown(base: i32) () {
    let callback = \[base] value: i32 -> { base + value };
    retain(&callback);
}

fn stored_indirectly(base: i32) () {
    let fallback = \[base] value: i32 -> { base - value };
    let callback = \[base] value: i32 -> { base + value };
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
fn mutually_recursive_call_summaries_converge_and_preserve_returned_callable() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn first(callback: &Fn(i32) i32, depth: i32) &Fn(i32) i32 {
    if depth == 0 {
        callback
    } else {
        second(callback, depth - 1)
    }
}

fn second(callback: &Fn(i32) i32, depth: i32) &Fn(i32) i32 {
    if depth == 0 {
        callback
    } else {
        first(callback, depth - 1)
    }
}

fn main(base: i32) &Fn(i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    first(&callback, 2)
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
            .contains("cannot be returned")
    );
}

#[test]
fn inner_block_view_cannot_flow_into_an_outer_local() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn main(base: i32) i32 {
    let outer = \[base] value: i32 -> { base + value };
    let mut view: &Fn(i32) i32 = &outer;
    {
        let inner = \[base] value: i32 -> { base - value };
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
fn placement_through_a_stack_pointer_remains_stack_backed() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn place[T](destination: &mut u8, value: T) &mut T
where T: Sized
{
    let mut target = destination as &mut T;
    target.* = value;
    target
}

fn leak(base: i32) &Fn(i32) i32 {
    let mut storage: [u8; 16] = [0; 16];
    let mut state = place(&mut storage[0], \[base] value: i32 -> { base + value });
    &mut state.*
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
            .contains("cannot be returned")
    );
}

#[test]
fn explicit_nonlexical_storage_allows_an_escaping_callable_view() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn place[T](destination: &mut u8, value: T) &mut T
where T: Sized
{
    let mut target = destination as &mut T;
    target.* = value;
    target
}

fn escape(address: usize, base: i32) &Fn(i32) i32 {
    let mut state = place(address as &mut u8, \[base] value: i32 -> { base + value });
    &mut state.*
}
"#,
    );
    let checked = query_db(fixture.program()).expect_get(CheckedProgramQuery);

    assert!(
        closure_diagnostics(&checked).is_empty(),
        "explicit externally managed storage produced diagnostics: {:?}",
        checked.diagnostics
    );
}

#[test]
fn propagated_error_provenance_does_not_contaminate_the_success_value() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn place[T](destination: &mut u8, value: T) &mut T
where T: Sized
{
    let mut target = destination as &mut T;
    target.* = value;
    target
}

fn separate(input: &mut u8, fail: bool) &mut u8!&mut u8 {
    if fail {
        return input!;
    }
    let address = input as usize;
    !(address as &mut u8)
}

fn escape(address: usize, base: i32) &mut u8!&Fn(i32) i32 {
    let mut local = 0u8;
    let destination = separate(&mut local, false).?;
    let mut state = place(destination, \[base] value: i32 -> { base + value });
    !&mut state.*
}
"#,
    );
    let checked = query_db(fixture.program()).expect_get(CheckedProgramQuery);

    assert!(
        closure_diagnostics(&checked).is_empty(),
        "error-only provenance contaminated the success value: {:?}",
        checked.diagnostics
    );
}

#[test]
fn unrelated_storage_cannot_hide_a_stack_backed_callable() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
struct Owner {
    callback: &Fn(i32) i32,
    storage: &mut u8,
}

fn pack(callback: &Fn(i32) i32, storage: &mut u8) Owner {
    Owner { callback, storage }
}

fn leak(storage: &mut u8, base: i32) Owner {
    let state = \[base] value: i32 -> { base + value };
    let callback: &Fn(i32) i32 = &state;
    pack(callback, storage)
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
            .contains("cannot be returned")
    );
}

#[test]
fn closure_summary_preserves_the_selected_capture_slot() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn select(first: &Fn(i32) i32, second: &Fn(i32) i32) &Fn(i32) i32 {
    let choose = \[first, second] -> { first };
    choose()
}

fn preserve(external: &Fn(i32) i32, base: i32) &Fn(i32) i32 {
    let local = \[base] value: i32 -> { base + value };
    select(external, &local)
}
"#,
    );
    let checked = query_db(fixture.program()).expect_get(CheckedProgramQuery);

    assert!(
        closure_diagnostics(&checked).is_empty(),
        "unselected capture contaminated the returned callable: {:?}",
        checked.diagnostics
    );
}

#[test]
fn expression_callee_is_analyzed_before_its_arguments() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
extern fn retain(callback: &Fn(i32) i32) ();

fn consume(callback: &Fn(i32) i32) () {
    retain(callback);
}

fn main(external: &Fn(i32) i32, base: i32) () {
    let local = \[base] value: i32 -> { base + value };
    let mut selected = external;
    ({ selected = &local; consume })(selected);
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
            .contains("passed to a call that may retain it")
    );
}

#[test]
fn while_condition_provenance_is_reapplied_after_each_backedge() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
extern fn retain(callback: &Fn(i32) i32) ();

fn main(external: &Fn(i32) i32, base: i32) () {
    let local = \[base] value: i32 -> { base + value };
    let mut pending = external;
    let mut selected = external;
    while { selected = pending; true } {
        retain(selected);
        pending = &local;
    }
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
            .contains("passed to a call that may retain it")
    );
}

#[test]
fn repeated_scalar_tuple_return_does_not_retain_callable_provenance() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
extern fn erase(callback: &Fn(i32) i32) (i32, i32);

fn main(base: i32) (i32, i32) {
    let local = \[base] value: i32 -> { base + value };
    erase(&local)
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
            .contains("passed to a call that may retain it")
    );
}

#[test]
fn defers_observe_scope_exit_state_in_lifo_order() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
extern fn retain(callback: &Fn(i32) i32) ();

fn main(external: &Fn(i32) i32, base: i32) () {
    let local = \[base] value: i32 -> { base + value };
    let mut selected = external;
    defer retain(selected);
    defer { selected = &local; };
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
            .contains("passed to a call that may retain it")
    );
}

#[test]
fn return_paths_run_active_defers_before_later_environment_overwrites() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
extern fn retain(callback: &Fn(i32) i32) ();

fn main(external: &Fn(i32) i32, base: i32, stop: bool) () {
    let local = \[base] value: i32 -> { base + value };
    let mut selected = external;
    defer retain(selected);
    if stop {
        selected = &local;
        return;
    }
    selected = external;
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
            .contains("passed to a call that may retain it")
    );
}

#[test]
fn assignment_place_effects_precede_rhs_provenance_reads() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn main(destination: &mut &Fn(i32) i32, external: &Fn(i32) i32, base: i32) () {
    let local = \[base] value: i32 -> { base + value };
    let mut selected = external;
    ({ selected = &local; destination }).* = selected;
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
            .contains("stored outside its local frame")
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
    let callback = \[base] value: i32 -> { base + value };
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
    let callback = \[base] value: i32 -> { base + value };
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
