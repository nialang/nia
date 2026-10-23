// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn supertrait_impls_must_be_explicit() {
    let root = temp_dir("supertrait_impls_must_be_explicit");
    write(
        &root.join("main.nia"),
        r#"
trait Same {
    fun eq(& self, other: & Self) bool;
}

trait Ranked : Same {
    fun lt(& self, other: & Self) bool;
}

struct Point {
    x: i32,
}

extend Point : Ranked {
    fun lt(& self, other: & Point) bool {
        self.x < other.x
    }
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("requires explicit implementation of supertrait `Same`")),
        "{:?}",
        program.diagnostics
    );
    let missing = program
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("requires explicit implementation of supertrait `Same`")
        })
        .expect("missing supertrait diagnostic");
    assert!(
        missing
            .diagnostic
            .primary_message()
            .is_some_and(|message| message.contains("no `Same` supertrait witness")),
        "{missing:?}"
    );
    assert!(
        missing.diagnostic.related.iter().any(|related| related
            .message
            .contains("implemented trait requires `Same`")),
        "{missing:?}"
    );
    assert!(
        missing
            .diagnostic
            .help
            .iter()
            .any(|help| help.contains("add an implementation of `Same`")),
        "{missing:?}"
    );
}

#[test]
fn where_bound_on_subtrait_exposes_supertrait_methods() {
    let root = temp_dir("where_bound_on_subtrait_exposes_supertrait_methods");
    write(
        &root.join("main.nia"),
        r#"
trait Same {
    fun eq(& self, other: & Self) bool;
}

trait Ranked : Same {
    fun lt(& self, other: & Self) bool;
}

struct Point {
    x: i32,
}

extend Point : Same {
    fun eq(& self, other: & Point) bool {
        self.x == other.x
    }
}

extend Point : Ranked {
    fun lt(& self, other: & Point) bool {
        self.x < other.x
    }
}

fun same_ord[T](a: & T, b: & T) bool
where T: Ranked {
    a.eq(b)
}

fun main() bool {
    let mut a = Point { x: 1 };
    let mut b = Point { x: 1 };
    same_ord[Point](& a, & b)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_supertraits_substitute_trait_arguments() {
    let root = temp_dir("generic_supertraits_substitute_trait_arguments");
    write(
        &root.join("main.nia"),
        r#"
trait Source[A] {
    type Item;

    fun get(& self) [Self as Source[A]]::Item;
}

trait SizedSource[A] : Source[A] {
    fun size(& self) usize;

    fun get_or(& self, fallback: [Self as Source[A]]::Item) [Self as Source[A]]::Item {
        if self.size() == 0 {
            fallback
        } else {
            self.get()
        }
    }
}

struct I32Source {
    value: i32,
}

extend I32Source : Source[i32] {
    type Item = i32;

    fun get(& self) i32 {
        self.value
    }
}

extend I32Source : SizedSource[i32] {
    fun size(& self) usize {
        1usize
    }
}

fun read_sized[S](value: & S, fallback: [S as Source[i32]]::Item) [S as Source[i32]]::Item
where S: SizedSource[i32] {
    value.get_or(fallback)
}

fun main() i32 {
    let mut source = I32Source { value: 7 };
    read_sized[I32Source](& source, 9)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn supertrait_associated_bindings_reach_default_methods_and_impl_validation() {
    let root = temp_dir("supertrait_associated_bindings_reach_default_methods_and_impl_validation");
    write(
        &root.join("main.nia"),
        r#"
trait Parent {
    type Item;
}

trait Child : Parent[Item = i32] {
    fun value(&self) i32 {
        let item: [Self as Parent]::Item = 7;
        item
    }
}

struct Good {}
extend Good : Parent { type Item = i32; }
extend Good : Child {}

struct Bad {}
extend Bad : Parent { type Item = bool; }
extend Bad : Child {}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_eq!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("does not satisfy associated type bindings of supertrait `Parent`"))
            .count(),
        1,
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().all(|diagnostic| {
            !diagnostic
                .diagnostic
                .summary
                .contains("type mismatch in binding initializer")
                && !diagnostic
                    .diagnostic
                    .summary
                    .contains("type mismatch in function body")
        }),
        "{:?}",
        program.diagnostics
    );
    let mismatch = program
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("does not satisfy associated type bindings of supertrait `Parent`")
        })
        .expect("supertrait associated binding diagnostic");
    assert!(
        mismatch
            .diagnostic
            .primary_message()
            .is_some_and(|message| message.contains("associated types do not satisfy `Parent`")),
        "{mismatch:?}"
    );
    assert!(
        mismatch
            .diagnostic
            .related
            .iter()
            .any(|related| related.message.contains("supertrait binding for `Parent`")),
        "{mismatch:?}"
    );
    assert!(
        mismatch
            .diagnostic
            .help
            .iter()
            .any(|help| help.contains("define associated types")),
        "{mismatch:?}"
    );
}

#[test]
fn conflicting_supertrait_associated_bindings_are_rejected() {
    let root = temp_dir("conflicting_supertrait_associated_bindings_are_rejected");
    write(
        &root.join("main.nia"),
        r#"
trait Parent { type Item; }

trait DirectConflict : Parent[Item = i32] + Parent[Item = bool] {}

trait Left : Parent[Item = i32] {}
trait RightConflict : Parent[Item = bool] {}
trait DiamondConflict : Left + RightConflict {}

trait RightCompatible : Parent[Item = i32] {}
trait DiamondCompatible : Left + RightCompatible {}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_eq!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("conflicting inherited associated type binding"))
            .count(),
        2,
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_supertraits_require_the_exact_parent_instance() {
    let root = temp_dir("const_generic_supertraits_require_the_exact_parent_instance");
    write(
        &root.join("main.nia"),
        r#"
trait Base[N: usize] {
    fun base(& self) usize;
}

trait Child[N: usize] : Base[N] {
    fun child(& self) usize;
}

struct Good {}

extend Good : Base[4] {
    fun base(& self) usize { 4usize }
}

extend Good : Child[4] {
    fun child(& self) usize { 4usize }
}

struct Bad {}

extend Bad : Base[8] {
    fun base(& self) usize { 8usize }
}

extend Bad : Child[4] {
    fun child(& self) usize { 4usize }
}

fun main() usize { 0usize }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_eq!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("requires explicit implementation of supertrait `Base`"))
            .count(),
        1,
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn source_trait_with_builtin_supertrait_requires_builtin_impl() {
    let root = temp_dir("source_trait_with_builtin_supertrait_requires_builtin_impl");
    write(
        &root.join("main.nia"),
        r#"
trait Child : Iterator {}

struct Missing {}

extend Missing : Child {}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("requires explicit implementation of supertrait `Iterator`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_where_bound_trait_methods_dispatch_to_impl_instances() {
    let root = temp_dir("generic_where_bound_trait_methods_dispatch_to_impl_instances");
    write(
        &root.join("main.nia"),
        r#"
trait Same {
    fun eq(& self, other: & Self) bool;
}

struct Point {
    x: i32,
}

extend Point : Same {
    fun eq(& self, other: & Point) bool {
        self.x == other.x
    }
}

fun same[T](a: & T, b: & T) bool
where T: Same {
    a.eq(b)
}

fun main() bool {
    let mut a = Point { x: 1 };
    let mut b = Point { x: 1 };
    same[Point](& a, & b)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_default_methods_are_used_when_impl_omits_method() {
    let root = temp_dir("trait_default_methods_are_used_when_impl_omits_method");
    write(
        &root.join("main.nia"),
        r#"
trait Same {
    fun eq(& self, other: & Self) bool;

    fun ne(& self, other: & Self) bool {
        not self.eq(other)
    }
}

struct Point {
    x: i32,
}

extend Point : Same {
    fun eq(& self, other: & Point) bool {
        self.x == other.x
    }
}

fun different[T](a: & T, b: & T) bool
where T: Same {
    a.ne(b)
}

fun main() bool {
    let mut a = Point { x: 1 };
    let mut b = Point { x: 2 };
    different[Point](& a, & b)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
