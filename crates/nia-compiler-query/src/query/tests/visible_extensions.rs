// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn executable_visible_extensions_follow_facade_provider_chains() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module facade;
using entry::facade;

fn main() i32 {
let init = facade::Init::init();
let args = init.args();
let mut iter = args.iter();
match iter.next() {
    ?value => {
        value
    },
    null => {
        0
    },
}
}
"#,
    );
    let entry_id = fixture.entry_id();
    let facade_id = fixture.add_child(
        entry_id,
        "facade",
        "facade.nia",
        r#"
module args_impl;
module init_impl;
module types;

pub using self::types::{Args, ArgsIter, Init};
"#,
    );
    fixture.add_child_with_visibility(
        facade_id,
        "args_impl",
        nia_ids::Visibility::Private,
        "facade/args_impl.nia",
        r#"
using entry::facade::types::{Args, ArgsIter};

extend Args {
pub fn iter(&self) ArgsIter {
    ArgsIter {}
}
}

extend ArgsIter {
pub fn next(&mut self) ?i32 {
    ?42
}
}
"#,
    );
    fixture.add_child_with_visibility(
        facade_id,
        "init_impl",
        nia_ids::Visibility::Private,
        "facade/init_impl.nia",
        r#"
using entry::facade::types::{Args, Init};

extend Init {
pub fn init() Init {
    {}
}

pub fn args(&self) Args {
    Args {}
}
}
"#,
    );
    fixture.add_child(
        facade_id,
        "types",
        "facade/types.nia",
        r#"
pub struct Init {}
pub struct Args {}
pub struct ArgsIter {}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let checked = db.expect_get(CodegenProgramQuery);

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn visible_extensions_do_not_expand_using_type_modules_as_provider_modules() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module facade;
using entry::facade;

fn main(value: facade::Used) i32 {
value.len()
}
"#,
    );
    let entry_id = fixture.entry_id();
    let facade_id = fixture.add_child(
        entry_id,
        "facade",
        "facade.nia",
        r#"
module impls;
module types;

pub using self::types::{Unused, Used};
"#,
    );
    fixture.add_child_with_visibility(
        facade_id,
        "impls",
        nia_ids::Visibility::Private,
        "facade/impls.nia",
        r#"
using entry::facade::types::Used;

extend Used {
pub fn len(&self) i32 {
    1
}
}
"#,
    );
    let types_id = fixture.add_child(
        facade_id,
        "types",
        "facade/types.nia",
        r#"
pub struct Unused {}
pub struct Used {}
"#,
    );
    let entry_description = format!("{entry_id:?}");
    let types_description = format!("{types_id:?}");
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let checked = db.expect_get(CodegenProgramQuery);

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let trace = db.query_trace();
    assert!(
        !trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.from.description.contains(&entry_description)
                && dependency.to.description.contains(&types_description)
                && dependency.to.name == "signature_type_normalization"
        }),
        "visible extensions should not normalize every module that merely defines a using-imported type"
    );
}

#[test]
fn visible_trait_impls_follow_facade_reexport_item_modules() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module parse;
using entry::parse;

fn main() i32 {
(&"abc").parse[i32]()
}
"#,
    );
    let entry_id = fixture.entry_id();
    let parse_id = fixture.add_child(
        entry_id,
        "parse",
        "parse.nia",
        r#"
pub module parse_impl;
pub using parse_impl::From;
"#,
    );
    let parse_impl_id = fixture.add_child(
        parse_id,
        "parse_impl",
        "fmt/parse_impl.nia",
        r#"
pub trait From[Input] {
fn from(input: Input) Self;
}

extend[Unit] [Unit]
where Unit: Sized
{
pub fn parse[T](&self) T
where T: From[&[Unit]] {
[T]::from(self)
}
}

extend i32 : From[&[char]] {
fn from(input: &[char]) i32 {
    input.len() as i32
}
}

extend i32 : From[&[u8]] {
fn from(input: &[u8]) i32 {
    input.len() as i32
}
}
"#,
    );
    let db = query_db(fixture.program());

    let trait_impls = db.expect_get(VisibleTraitImplsQuery(entry_id));

    assert_eq!(trait_impls.trait_impls.len(), 2);
    assert!(
        trait_impls
            .trait_impls
            .iter()
            .all(|impl_signature| impl_signature.module_id == parse_impl_id),
        "{:?}",
        trait_impls.trait_impls
    );
}
