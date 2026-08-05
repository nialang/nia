// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn const_pointer_equality_uses_allocation_provenance() {
    let root = temp_dir("const_pointer_equality_uses_allocation_provenance");
    write(
        &root.join("main.nia"),
        r#"
const fn samePlace() bool {
    let value: usize = 7;
    let first = &value;
    let second = &value;
    first == second
}

const fn distinctPlaces() bool {
    let left: usize = 7;
    let right: usize = 7;
    &left == &right
}

const width: usize = if samePlace() and not distinctPlaces() { 2 } else { 0 };

fn main() usize {
    let values: [width]usize = [1, 2];
    values.len()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_place_pointer_dereference_reads_the_live_allocation() {
    let root = temp_dir("const_place_pointer_dereference_reads_the_live_allocation");
    write(
        &root.join("main.nia"),
        r#"
const fn read() usize {
    let mut value: usize = 4;
    let pointer = &value;
    value = 11;
    pointer.*
}

const width: usize = read();

fn main() usize {
    let values: [width]u8 = [0; width];
    values.len()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_can_return_caller_pointer_provenance() {
    let root = temp_dir("const_function_can_return_caller_pointer_provenance");
    write(
        &root.join("main.nia"),
        r#"
const fn identity(pointer: &usize) &usize {
    pointer
}

const fn read() usize {
    let value: usize = 13;
    identity(&value).*
}

const width: usize = read();

fn main() usize {
    let values: [width]u8 = [0; width];
    values.len()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_rejects_pointer_to_local_storage_escape() {
    let root = temp_dir("const_function_rejects_pointer_to_local_storage_escape");
    write(
        &root.join("main.nia"),
        r#"
const fn dangling() &usize {
    let value: usize = 7;
    &value
}

const escaped: &usize = dangling();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot retain a pointer to storage whose lifetime ends here")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_nested_pointer_to_local_storage_escape() {
    let root = temp_dir("const_function_rejects_nested_pointer_to_local_storage_escape");
    write(
        &root.join("main.nia"),
        r#"
struct Holder {
    pointer: &usize,
}

const fn dangling() Holder {
    let value: usize = 7;
    Holder { pointer: &value }
}

const escaped: Holder = dangling();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot retain a pointer to storage whose lifetime ends here")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_pointer_rejects_dereference_after_nested_scope_ends() {
    let root = temp_dir("const_pointer_rejects_dereference_after_nested_scope_ends");
    write(
        &root.join("main.nia"),
        r#"
const fn danglingRead() usize {
    let mut pointer: &usize = &0usize;
    if true {
        let value: usize = 7;
        pointer = &value;
    }
    pointer.*
}

const value: usize = danglingRead();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const pointer refers to storage whose lifetime has ended")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn top_level_const_can_promote_a_frozen_readonly_array_allocation() {
    let root = temp_dir("top_level_const_can_promote_a_frozen_readonly_array_allocation");
    write(
        &root.join("main.nia"),
        r#"
const values: &[2]usize = &[2]usize[5, 9];
const width: usize = values.*[1];

fn main() usize {
    let array: [width]u8 = [0; width];
    array.len()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn local_const_can_promote_a_frozen_readonly_array_allocation() {
    let root = temp_dir("local_const_can_promote_a_frozen_readonly_array_allocation");
    write(
        &root.join("main.nia"),
        r#"
fn main() usize {
    const values: &[2]usize = &[2]usize[5, 9];
    values.*[1]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_rejects_readonly_rvalue_temporary_escape() {
    let root = temp_dir("const_function_rejects_readonly_rvalue_temporary_escape");
    write(
        &root.join("main.nia"),
        r#"
const fn temporary() &[2]usize {
    &[2]usize[5, 9]
}

const values: &[2]usize = temporary();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot retain a pointer to storage whose lifetime ends here")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn top_level_const_rejects_writable_frozen_promotion() {
    let root = temp_dir("top_level_const_rejects_writable_frozen_promotion");
    write(
        &root.join("main.nia"),
        r#"
const values: &mut [2]usize = &mut [2]usize[5, 9];
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot retain a writable pointer to promoted temporary storage")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_union_pointer_field_round_trips_provenance() {
    let root = temp_dir("const_union_pointer_field_round_trips_provenance");
    write(
        &root.join("main.nia"),
        r#"
union Slot {
    pointer: &usize,
    integer: usize,
}

const fn read() usize {
    let value: usize = 21;
    let pointer = &value;
    let slot: Slot = { pointer: pointer };
    if slot.pointer == pointer { slot.pointer.* } else { 0 }
}

const width: usize = read();

fn main() usize {
    let values: [width]u8 = [0; width];
    values.len()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn top_level_const_union_preserves_frozen_pointer_relocation() {
    let root = temp_dir("top_level_const_union_preserves_frozen_pointer_relocation");
    write(
        &root.join("main.nia"),
        r#"
union Slot {
    pointer: &usize,
    integer: usize,
}

const slot: Slot = { pointer: &34usize };
const width: usize = slot.pointer.*;

fn main() usize {
    let values: [width]u8 = [0; width];
    values.len()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_union_rejects_pointer_relocation_as_integer_bytes() {
    let root = temp_dir("const_union_rejects_pointer_relocation_as_integer_bytes");
    write(
        &root.join("main.nia"),
        r#"
union Slot {
    pointer: &usize,
    integer: usize,
}

const fn bits() usize {
    let value: usize = 21;
    let slot: Slot = { pointer: &value };
    slot.integer
}

const value: usize = bits();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("scalar field reinterprets pointer relocation storage")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_union_rejects_integer_bytes_as_pointer() {
    let root = temp_dir("const_union_rejects_integer_bytes_as_pointer");
    write(
        &root.join("main.nia"),
        r#"
union Slot {
    pointer: &usize,
    integer: usize,
}

const fn pointer() &usize {
    let slot: Slot = { integer: 0 };
    slot.pointer
}

const value: &usize = pointer();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("pointer field requires one exact pointer relocation")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_union_partial_pointer_overwrite_marks_unwritten_bytes_uninitialized() {
    let root =
        temp_dir("const_union_partial_pointer_overwrite_marks_unwritten_bytes_uninitialized");
    write(
        &root.join("main.nia"),
        r#"
union Slot {
    pointer: &usize,
    byte: u8,
    integer: usize,
}

const fn bits() usize {
    let value: usize = 21;
    let mut slot: Slot = { pointer: &value };
    slot.byte = 0;
    slot.integer
}

const value: usize = bits();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const union field reads uninitialized storage")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_union_relocation_cannot_hide_a_local_pointer_escape() {
    let root = temp_dir("const_union_relocation_cannot_hide_a_local_pointer_escape");
    write(
        &root.join("main.nia"),
        r#"
union Slot {
    pointer: &usize,
    integer: usize,
}

const fn dangling() Slot {
    let value: usize = 21;
    { pointer: &value }
}

const slot: Slot = dangling();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot retain a pointer to storage whose lifetime ends here")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn runtime_union_materialization_waits_for_relocation_lowering() {
    let root = temp_dir("runtime_union_materialization_waits_for_relocation_lowering");
    write(
        &root.join("main.nia"),
        r#"
union Slot {
    pointer: &usize,
    integer: usize,
}

const slot: Slot = { pointer: &34usize };

fn main() Slot {
    slot
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("runtime expression cannot use this const value")),
        "{:?}",
        program.diagnostics
    );
}
