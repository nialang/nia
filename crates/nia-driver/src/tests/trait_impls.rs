// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::check_program;

#[test]
fn trait_impl_methods_are_checked_against_trait_requirements() {
    let root = temp_dir("trait_impl_methods_are_checked_against_trait_requirements");
    write(
        &root.join("main.nia"),
        r#"
trait Show {
    fn show(& self) i32;
}

struct Point {
    x: i32,
}

extend Point : Show {
    fn show(& self) i32 {
        self.x
    }
}

fn main() i32 {
    var point: Point = { x: 7 };
    point.show()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_impl_rejects_extra_missing_and_mismatched_methods() {
    let root = temp_dir("trait_impl_rejects_extra_missing_and_mismatched_methods");
    write(
        &root.join("main.nia"),
        r#"
trait Show {
    fn show(& self) i32;
    fn size(& self) i32;
}

struct Point {
    x: i32,
}

extend Point : Show {
    fn show(self) i32 {
        self.x
    }

    fn debug(& self) i32 {
        self.x
    }
}

fn main() i32 {
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("does not match the trait signature")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("is not a member of implemented trait")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("missing implementation for trait method `size`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trait_impl_substitutes_self_in_required_signatures() {
    let root = temp_dir("trait_impl_substitutes_self_in_required_signatures");
    write(
        &root.join("main.nia"),
        r#"
trait Same {
    fn eq(& self, other: & Self) bool;
}

struct Point {
    x: i32,
}

extend Point : Same {
    fn eq(& self, other: & Point) bool {
        self.x == other.x
    }
}

fn main() bool {
    var a: Point = { x: 1 };
    var b: Point = { x: 1 };
    a.eq(& b)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_impl_where_clause_constrains_impl_availability() {
    let root = temp_dir("trait_impl_where_clause_constrains_impl_availability");
    write(
        &root.join("main.nia"),
        r#"
trait Show {
    fn show(& self) i32;
}

trait Marker {}

extend i32 : Marker {}

struct Box[T] {
    value: &T,
}

extend[T] Box[T] : Show
where T: Marker {
    fn show(& self) i32 {
        _ = self;
        1
    }
}

fn needs_show[S](value: &S) i32
where S: Show {
    value.show()
}

fn main(value: &Box[bool]) i32 {
    needs_show[Box[bool]](value)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("trait bound not satisfied")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn cross_module_trait_impls_are_checked() {
    let root = temp_dir("cross_module_trait_impls_are_checked");
    write(
        &root.join("main.nia"),
        r#"
import .traits;

struct Point {
    x: i32,
}

extend Point : traits::Show {
    fn show(& self) i32 {
        self.x
    }

    fn debug(& self) i32 {
        self.x
    }
}

fn main() i32 {
    0
}
"#,
    );
    write(
        &root.join("traits.nia"),
        r#"
pub trait Show {
    fn show(& self) i32;
    fn size(& self) i32;
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("is not a member of implemented trait")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("missing implementation for trait method `size`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn cross_module_generic_trait_method_dispatch_finds_impl_defined_with_generic() {
    let root =
        temp_dir("cross_module_generic_trait_method_dispatch_finds_impl_defined_with_generic");
    write(
        &root.join("io.nia"),
        r#"
pub trait Writer {
    type Error;

    fn write(& self, bytes: &[u8]) [Self as Writer]::Error!usize;
}

pub fn write_all[W](writer: & W, bytes: &[u8]) [W as Writer]::Error!void
where W: Writer
{
    var written = writer.write(bytes).?;
    _ = written;
    !{}
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .io;

struct Fd {
    raw: i32,
}

enum Error: i32 {
    Io = 5,
    _,
}

fn write(fd: Fd, bytes: &[u8]) Error!usize {
    _ = fd;
    !(bytes.len())
}

extend Fd : io::Writer {
    type Error = Error;

    fn write(& self, bytes: &[u8]) Error!usize {
        write(self.*, bytes)
    }
}

fn main() void {
    var stdout: Fd = { raw: 1 };
    switch io::write_all[Fd](& stdout, b"nia\n") {
        !ok => _ = ok,
        error! => {},
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn cross_module_generic_trait_method_dispatch_finds_impl_in_generic_module_for_foreign_type() {
    let root = temp_dir(
        "cross_module_generic_trait_method_dispatch_finds_impl_in_generic_module_for_foreign_type",
    );
    write(
        &root.join("os.nia"),
        r#"
pub struct Fd {
    raw: i32,
}

pub enum Error: i32 {
    Io = 5,
    _,
}

pub fn stdout() Fd {
    { raw: 1 }
}

pub fn write(fd: Fd, bytes: &[u8]) Error!usize {
    _ = fd;
    !(bytes.len())
}
"#,
    );
    write(
        &root.join("io.nia"),
        r#"
import .os;

pub trait Writer {
    type Error;

    fn write(& self, bytes: &[u8]) [Self as Writer]::Error!usize;
}

pub fn write_all[W](writer: & W, bytes: &[u8]) [W as Writer]::Error!void
where W: Writer
{
    var written = writer.write(bytes).?;
    _ = written;
    !{}
}

extend os::Fd : Writer {
    type Error = os::Error;

    fn write(& self, bytes: &[u8]) os::Error!usize {
        os::write(self.*, bytes)
    }
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .io;
import .os;

fn main() void {
    var stdout = os::stdout();
    switch io::write_all[os::Fd](& stdout, b"nia\n") {
        !ok => _ = ok,
        error! => {},
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
