// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn executable_incremental_body_check_preserves_extension_method_receiver_types() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module writer;
using entry::writer;

fn main() i32 {
let mut sink = writer::Sink::init();
switch sink.write(b"ok") {
    !value => {
        value as i32
    },
    error! => {
        0
    },
}
}
"#,
    );
    let entry_id = fixture.entry_id();
    let writer_id = fixture.add_child(
        entry_id,
        "writer",
        "writer.nia",
        r#"
pub trait Writer {
type Error;

fn short_write(&self) Error;

fn write(&mut self, bytes: &[u8]) Error!usize;
}

pub enum WriteError: i32 {
Short = 1,
_,
}

pub struct Sink {}

extend Sink {
pub fn init() Sink {
    {}
}
}

extend Sink : Writer {
type Error = WriteError;

pub fn short_write(&self) Error {
    WriteError::Short
}

pub fn write(&mut self, bytes: &[u8]) Error!usize {
    if bytes.len() == 0 {
        return self.short_write()!;
    }
    !bytes.len()
}
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let writer = modules
        .iter()
        .find(|module| module.id == writer_id)
        .expect("writer module should be executable-reachable");
    let write_def = writer
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == sym("write") && def.kind == nia_defs::DefKind::Method).then_some(def_id)
        })
        .expect("write method should be defined");
    let write_id = GlobalDefId {
        module_id: writer_id,
        def_id: write_def,
    };
    let write_body = writer
        .body_ir
        .function_bodies
        .get(&write_id)
        .expect("write method should have a checked body");
    let self_ty = write_body
        .locals
        .iter()
        .find(|local| {
            local.name.is_self_value() && local.kind == nia_body_ir::TypedLocalKind::Param
        })
        .map(|local| local.ty)
        .expect("write method should have a self param");
    assert!(
        !matches!(db.context().type_store.get(self_ty), Some(TyKind::Error)),
        "reachable extension method receiver/params should not collapse to error types"
    );
}

#[test]
fn trait_signature_subset_resolves_local_extend_target_types() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
trait Writer {
type Error;
fn write(&mut self) Error!();
}

enum WriteError: i32 {
Bad = 1,
_,
}

struct Sink {}

extend Sink : Writer {
type Error = WriteError;

fn write(&mut self) Error!() {
    !()
}
}
"#,
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let signatures = db.expect_get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Traits,
    ));
    let impl_signature = signatures
        .semantic
        .trait_impls
        .iter()
        .find(|impl_signature| !impl_signature.methods.is_empty())
        .expect("trait impl should be collected");

    assert!(
        !matches!(
            db.context().type_store.get(impl_signature.target_ty),
            Some(TyKind::Error)
        ),
        "trait signature subset should resolve local extend target types"
    );
}

#[test]
fn trait_signature_subset_resolves_imported_extend_target_types() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module platform;
using entry::platform;

trait IntoError[Target] {
fn into_error(self) Target;
}

enum Error: i32 {
Bad = 1,
_,
}

extend platform::Errno : IntoError[Error] {
fn into_error(self) Error {
    Error::Bad
}
}
"#,
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "platform",
        "platform.nia",
        r#"
pub enum Errno: i32 {
Bad = 1,
_,
}
"#,
    );
    let db = query_db(fixture.program());

    let signatures = db.expect_get(SignatureItemSignaturesQuery(
        entry_id,
        nia_item_tree::SignatureItemSet::Traits,
    ));
    let impl_signature = signatures
        .semantic
        .trait_impls
        .iter()
        .find(|impl_signature| !impl_signature.methods.is_empty())
        .expect("trait impl should be collected");

    assert!(
        !matches!(
            db.context().type_store.get(impl_signature.target_ty),
            Some(TyKind::Error)
        ),
        "trait signature subset should resolve imported extend target types"
    );
}

#[test]
fn trait_signature_subset_resolves_reexported_extend_target_types() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module platform;
using entry::platform;

trait IntoError[Target] {
fn into_error(self) Target;
}

enum Error: i32 {
Bad = 1,
_,
}

extend platform::Errno : IntoError[Error] {
fn into_error(self) Error {
    Error::Bad
}
}
"#,
    );
    let entry_id = fixture.entry_id();
    let platform_id = fixture.add_child(
        entry_id,
        "platform",
        "platform.nia",
        r#"
module types;
using entry::platform::types;

pub using types::{Errno};
"#,
    );
    fixture.add_child(
        platform_id,
        "types",
        "types.nia",
        r#"
pub enum Errno: i32 {
Bad = 1,
_,
}
"#,
    );
    let db = query_db(fixture.program());

    let signatures = db.expect_get(SignatureItemSignaturesQuery(
        entry_id,
        nia_item_tree::SignatureItemSet::Traits,
    ));
    let impl_signature = signatures
        .semantic
        .trait_impls
        .iter()
        .find(|impl_signature| !impl_signature.methods.is_empty())
        .expect("trait impl should be collected");

    assert!(
        !matches!(
            db.context().type_store.get(impl_signature.target_ty),
            Some(TyKind::Error)
        ),
        "trait signature subset should resolve re-exported extend target types"
    );
}

#[test]
fn executable_incremental_body_check_preserves_reexported_trait_witness_receiver_types() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module platform;
using entry::platform;

trait IntoError[Target] {
fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
fn cast_error(self) Target!T {
    switch self {
        !ok => {
            !ok
        },
        error! => {
            error.into_error()!
        },
    }
}
}

enum Error: i32 {
Bad = 1,
_,
}

extend platform::Errno : IntoError[Error] {
fn into_error(self) Error {
    Error::Bad
}
}

fn fail() platform::Errno!i32 {
platform::Errno::Bad!
}

fn main() Error!i32 {
fail().cast_error()
}
"#,
    );
    let entry_id = fixture.entry_id();
    let platform_id = fixture.add_child(
        entry_id,
        "platform",
        "platform.nia",
        r#"
module types;
using entry::platform::types;

pub using types::{Errno};
"#,
    );
    fixture.add_child(
        platform_id,
        "types",
        "types.nia",
        r#"
pub enum Errno: i32 {
Bad = 1,
_,
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == entry_id)
        .expect("entry module should be executable-reachable");
    assert!(
        module.body_diagnostics.is_empty(),
        "generic extension wrapper diagnostics should stay clean: {:?}",
        module.body_diagnostics
    );
    let into_error = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == sym("into_error") && def.kind == nia_defs::DefKind::Method).then_some(
                GlobalDefId {
                    module_id: entry_id,
                    def_id,
                },
            )
        })
        .expect("into_error method should be defined");
    let body = module
        .body_ir
        .function_bodies
        .get(&into_error)
        .expect("into_error should have a checked body");
    let self_ty = body
        .locals
        .iter()
        .find(|local| {
            local.name.is_self_value() && local.kind == nia_body_ir::TypedLocalKind::Param
        })
        .map(|local| local.ty)
        .expect("into_error should have a self param");
    assert!(
        !matches!(db.context().type_store.get(self_ty), Some(TyKind::Error)),
        "re-exported trait witness receiver should not collapse to error"
    );
}
