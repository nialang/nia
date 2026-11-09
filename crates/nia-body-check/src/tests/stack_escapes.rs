// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

/// Diagnostic summaries in source order; functions are checked in no
/// particular order.
fn summaries(source: &str) -> Vec<String> {
    let checked = pipeline(source);
    let mut diagnostics = checked.diagnostics.iter().collect::<Vec<_>>();
    diagnostics.sort_by_key(|diagnostic| diagnostic.primary_span().map(|span| span.start));
    diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.summary.clone())
        .collect()
}

#[test]
fn rejects_returned_addresses_of_the_calls_own_storage() {
    let found = summaries(
        r#"
struct Pair {
    left: &u8,
    right: u8,
}

fun make() u8 {
    7
}

fun local_address() &u8 {
    let value: u8 = 1;
    &value
}

fun literal() &[char] {
    "exe"
}

fun local_range() &[u8] {
    let bytes: [u8; 3] = [1, 2, 3];
    &bytes[0..2]
}

fun whole_array() &[u8] {
    let bytes: [u8; 3] = [1, 2, 3];
    bytes
}

fun parameter(value: u8) &u8 {
    &value
}

fun temporary() &u8 {
    &make()
}

fun through_local() &u8 {
    let value: u8 = 1;
    let pointer = &value;
    pointer
}

fun in_aggregate() Pair {
    let value: u8 = 1;
    Pair { left: &value, right: 0 }
}

fun optional() ?&u8 {
    let value: u8 = 1;
    ?&value
}

fun early(flag: bool, other: &u8) &u8 {
    let value: u8 = 1;
    if flag {
        return &value;
    }
    other
}

fun array_pointer() &u8 {
    let bytes: [u8; 2] = [1, 2];
    bytes.ptr()
}
"#,
    );
    let expected = [
        "returned value points to local `value`, which does not outlive this call",
        "returned value points to a string literal, which does not outlive this call",
        "returned value points to local `bytes`, which does not outlive this call",
        "returned value points to local `bytes`, which does not outlive this call",
        "returned value points to parameter `value`, which does not outlive this call",
        "returned value points to a temporary, which does not outlive this call",
        "returned value points to local `value`, which does not outlive this call",
        "returned value points to local `value`, which does not outlive this call",
        "returned value points to local `value`, which does not outlive this call",
        "returned value points to local `value`, which does not outlive this call",
        "returned value points to local `bytes`, which does not outlive this call",
    ];
    assert_eq!(found, expected);
}

#[test]
fn rejects_stored_addresses_of_the_calls_own_storage() {
    let found = summaries(
        r#"
static anchor: u8 = 1;
static mut kept: &u8 = &anchor;

fun into_static() () {
    let value: u8 = 1;
    kept = &value;
}

fun into_caller(out: &mut &u8) () {
    let value: u8 = 1;
    out.* = &value;
}

fun into_caller_slice(out: &mut [&u8]) () {
    let value: u8 = 1;
    out[0] = &value;
}
"#,
    );
    let expected = [
        "a `static` is given a pointer to local `value`, which does not outlive this call",
        "memory the caller owns is given a pointer to local `value`, which does not outlive this call",
        "memory the caller owns is given a pointer to local `value`, which does not outlive this call",
    ];
    assert_eq!(found, expected);
}

#[test]
fn accepts_addresses_that_outlive_the_call_or_stay_in_it() {
    let found = summaries(
        r#"
struct Pair {
    left: &u8,
    right: u8,
}

static anchor: u8 = 1;

fun passthrough(value: &u8) &u8 {
    value
}

fun field(pair: &Pair) &u8 {
    pair.left
}

fun element(items: &[&u8]) &u8 {
    items[0]
}

fun range_of_parameter(items: &[u8]) &[u8] {
    &items[1..]
}

// A call is opaque: what it returns is not followed.
fun through_call(value: &u8) &u8 {
    let local: u8 = 1;
    _ = passthrough(&local);
    passthrough(value)
}

fun read_through() u8 {
    let value: u8 = 1;
    let pointer = &value;
    pointer.*
}

fun pointer_read_through_local(pair: Pair) &u8 {
    let copy = pair;
    let pointer = &copy;
    pointer.left
}

fun block_static() &[char] {
    static text: [char; 3] = "exe";
    text
}

fun static_address() &u8 {
    &anchor
}

// Zero-sized storage has nothing to read through a dangling pointer.
fun empty() &[u8] {
    &[]
}

fun unit() &() {
    &()
}

fun reassigned(other: &u8) &u8 {
    let value: u8 = 1;
    let mut pointer = &value;
    pointer = other;
    pointer
}

fun store_into_own_frame() () {
    let value: u8 = 1;
    let mut slot: &u8 = &anchor;
    let target = &mut slot;
    target.* = &value;
}

fun store_into_by_value_parameter(pair: Pair) u8 {
    let value: u8 = 1;
    let mut copy = pair;
    copy.left = &value;
    copy.right
}

// What the generic `Slice` implementation for `S` returns is not known here.
fun generic_range[S](items: S) [S as Slice[..]]::Output
where S: Slice[..] {
    &items[..]
}
"#,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn reports_the_return_and_the_address_site() {
    let checked = pipeline(
        r#"
fun local_address() &u8 {
    let value: u8 = 1;
    let pointer = &value;
    pointer
}

fun literal() &[char] {
    "exe"
}
"#,
    );
    let mut diagnostics = checked.diagnostics.iter().collect::<Vec<_>>();
    diagnostics.sort_by_key(|diagnostic| diagnostic.primary_span().map(|span| span.start));
    let [explicit, implicit] = diagnostics.as_slice() else {
        panic!("{:?}", checked.diagnostics);
    };
    let labels = |diagnostic: &nia_diagnostic::Diagnostic| {
        diagnostic
            .labels
            .iter()
            .map(|label| label.message.clone().unwrap_or_default())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        labels(explicit),
        ["the pointer is returned here", "`value` is addressed here"]
    );
    assert_eq!(
        explicit.notes.as_slice(),
        ["`value` lives in this call's frame, which is reclaimed when the call returns"]
    );
    assert_eq!(
        labels(implicit),
        ["the string literal is addressed and returned here"]
    );
    assert_eq!(
        implicit.help.as_slice(),
        ["declare the text as a `static` and point to that"]
    );
}
