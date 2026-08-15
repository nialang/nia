// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn executable_reachability_expands_where_predicates_through_generic_extension_wrappers() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module error;
module facade;
using entry::error;
using entry::facade;

enum Error: i32 {
Bad = 1,
_,
}

struct Source {
value: i32,
}

struct Target {
value: i32,
}

extend Source : error::IntoError[Target] {
fn into_error(self) Target {
    Target { value: self.value }
}
}

fn main() i32 {
let value: Source!i32 = Source { value: 1 }!;
match value.cast_error() {
    !ok => {
        ok
    },
    error! => {
        error.value
    },
}
}
"#,
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "error",
        "error.nia",
        r#"
pub trait IntoError[Target] {
fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
pub fn cast_error(self) Target!T {
    match self {
        !ok => {
            !ok
        },
        error! => {
            error.into_error()!
        },
    }
}
}
"#,
    );
    fixture.add_child(
        entry_id,
        "facade",
        "facade.nia",
        r#"
using entry::error;
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

    assert!(
        module.body_ir.function_bodies.contains_key(&into_error),
        "generic extension wrappers should make where-predicate trait witnesses executable-reachable"
    );
}

#[test]
fn executable_reachability_expands_generic_trait_calls_to_cross_module_impl_bodies() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module error;
module impls;
using entry::error;
using entry::impls;

fn main() i32 {
let value: impls::Source!i32 = impls::Source { value: 1 }!;
match value.cast_error() {
    !ok => {
        ok
    },
    error! => {
        error.value
    },
}
}
"#,
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "error",
        "error.nia",
        r#"
pub trait IntoError[Target] {
fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
pub fn cast_error(self) Target!T {
    match self {
        !ok => {
            !ok
        },
        error! => {
            error.into_error()!
        },
    }
}
}
"#,
    );
    let impls_id = fixture.add_child(
        entry_id,
        "impls",
        "impls.nia",
        r#"
using entry::error;

pub struct Source {
value: i32,
}

pub struct Target {
value: i32,
}

extend Source : error::IntoError[Target] {
fn into_error(self) Target {
    Target { value: self.value }
}
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == impls_id)
        .expect("impl module should be executable-reachable");
    let into_error = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == sym("into_error") && def.kind == nia_defs::DefKind::Method).then_some(
                GlobalDefId {
                    module_id: impls_id,
                    def_id,
                },
            )
        })
        .expect("cross-module into_error method should be defined");

    assert!(
        module.body_ir.function_bodies.contains_key(&into_error),
        "generic trait calls should make cross-module impl method bodies executable-reachable"
    );
}

#[test]
fn executable_reachability_expands_generic_trait_calls_from_incremental_wrapper_bodies() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module error;
module impls;
using entry::error;
using entry::impls;

fn main() i32 {
let value: impls::Source!i32 = impls::Source { value: 1 }!;
match value.as_target_error() {
    !ok => {
        ok
    },
    error! => {
        error.value
    },
}
}
"#,
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "error",
        "error.nia",
        r#"
pub trait IntoError[Target] {
fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
pub fn cast_error(self) Target!T {
    match self {
        !ok => {
            !ok
        },
        error! => {
            error.into_error()!
        },
    }
}
}
"#,
    );
    let impls_id = fixture.add_child(
        entry_id,
        "impls",
        "impls.nia",
        r#"
using entry::error;

pub struct Source {
value: i32,
}

pub struct Target {
value: i32,
}

extend Source : error::IntoError[Target] {
fn into_error(self) Target {
    Target { value: self.value }
}
}

extend[T] Source!T {
pub fn as_target_error(self) Target!T {
    self.cast_error()
}
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == impls_id)
        .expect("impl module should be executable-reachable");
    let into_error = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == sym("into_error") && def.kind == nia_defs::DefKind::Method).then_some(
                GlobalDefId {
                    module_id: impls_id,
                    def_id,
                },
            )
        })
        .expect("cross-module into_error method should be defined");

    assert!(
        module.body_ir.function_bodies.contains_key(&into_error),
        "generic wrapper bodies checked after incremental reachability must still expand their trait witnesses"
    );
}
