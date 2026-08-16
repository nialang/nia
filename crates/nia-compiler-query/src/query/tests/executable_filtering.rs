// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn executable_checked_modules_include_reachable_builtin_trait_witness_bodies() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
struct Counter {
current: i32,
end: i32,
}

extend Counter : Iterator {
type Item = i32;

fn next(&mut self) ?i32 {
    if self.current >= self.end {
        null
    } else {
        let value = self.current;
        self.current += 1;
        ?value
    }
}
}

fn main() i32 {
let mut total = 0;
let mut iter = Counter { current: 0, end: 3 };
for value in iter {
    total += value;
}
total
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let next = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Method && def.name == sym("next"))
                .then_some(GlobalDefId { module_id, def_id })
        })
        .expect("Iterator witness method");

    assert!(
        module.body_ir.function_bodies.contains_key(&next),
        "executable body checking must include builtin trait witness bodies"
    );
}

#[test]
fn executable_checked_modules_do_not_body_check_unmatched_builtin_trait_witnesses() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
struct Counter {
current: i32,
end: i32,
}

extend Counter : Iterator {
type Item = i32;

fn next(&mut self) ?i32 {
    if self.current >= self.end {
        null
    } else {
        let value = self.current;
        self.current += 1;
        ?value
    }
}
}

struct Unused {}

extend Unused : Iterator {
type Item = i32;

fn next(&mut self) ?i32 {
    ?missing_symbol
}
}

fn main() i32 {
let mut total = 0;
let mut iter = Counter { current: 0, end: 3 };
for value in iter {
    total += value;
}
total
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let unused_next = module
        .defs
        .defs
        .iter()
        .filter_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Method && def.name == sym("next"))
                .then_some(GlobalDefId { module_id, def_id })
        })
        .find(|def_id| !module.body_ir.function_bodies.contains_key(def_id))
        .expect("unmatched Iterator witness method");

    assert!(
        !module.body_ir.function_bodies.contains_key(&unused_next),
        "executable reachability should not include builtin trait witnesses for unmatched receiver types"
    );
    assert!(
        module.body_diagnostics.is_empty(),
        "unmatched builtin trait witness diagnostics should not block executable checking: {:?}",
        module.body_diagnostics
    );
}

#[test]
fn executable_checked_modules_do_not_body_check_unused_trait_witness_methods() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
trait Ops {
fn used(self) i32;
fn unused(self) i32;
}

struct Value {}

extend Value : Ops {
fn used(self) i32 {
    1
}

fn unused(self) i32 {
    missing_symbol
}
}

fn main() i32 {
let value = Value {};
value.used()
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let unused = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Method && def.name == sym("unused"))
                .then_some(GlobalDefId { module_id, def_id })
        })
        .expect("unused witness method");

    assert!(
        !module.body_ir.function_bodies.contains_key(&unused),
        "executable body checking should not include unused trait witness bodies"
    );
}

#[test]
fn executable_checked_modules_include_trait_witnesses_required_by_generic_where_predicates() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
trait IntoError[Target] {
fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
fn cast_error(self) Target!T {
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

struct Source {
value: i32,
}

struct Target {
value: i32,
}

extend Source : IntoError[Target] {
fn into_error(self) Target {
    Target { value: self.value }
}
}

struct Unused {}

extend Unused : IntoError[Target] {
fn into_error(self) Target {
    missing_symbol
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
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let into_error_methods = module
        .defs
        .defs
        .iter()
        .filter_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Method && def.name == sym("into_error"))
                .then_some(GlobalDefId { module_id, def_id })
        })
        .collect::<Vec<_>>();
    let reachable_into_error_count = into_error_methods
        .iter()
        .filter(|def_id| module.body_ir.function_bodies.contains_key(def_id))
        .count();

    assert_eq!(
        reachable_into_error_count, 1,
        "generic where-predicate closure should include only the matching IntoError witness"
    );
    assert!(
        module.body_diagnostics.is_empty(),
        "unmatched IntoError witness diagnostics should not block executable checking: {:?}",
        module.body_diagnostics
    );
}

#[test]
fn executable_checked_modules_include_trait_witnesses_required_by_default_method_body() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
trait Writer {
type Error;

fn short_write(&self) Error;

fn write(&mut self) Error!usize;

fn write_all(&mut self) Error!() {
    let n = self.write().?;
    if n == 0usize {
        return self.short_write()!;
    }
    !()
}
}

struct FileWriter {
value: i32,
}

extend FileWriter : Writer {
type Error = i32;

fn short_write(&self) Error {
    1
}

fn write(&mut self) Error!usize {
    self.value = 2;
    !1usize
}
}

struct Unused {}

extend Unused : Writer {
type Error = i32;

fn short_write(&self) Error {
    missing_symbol
}

fn write(&mut self) Error!usize {
    missing_symbol
}
}

fn main() i32!i32 {
let mut writer = FileWriter { value: 0 };
writer.write_all().?;
!writer.value
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let checked_witness_names = module
        .defs
        .defs
        .iter()
        .filter_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Method
                && module
                    .body_ir
                    .function_bodies
                    .contains_key(&GlobalDefId { module_id, def_id }))
            .then_some(def.name)
        })
        .collect::<Vec<_>>();

    assert!(
        checked_witness_names.contains(&sym("write")),
        "default method reachability should include concrete write witness: {checked_witness_names:?}"
    );
    assert!(
        checked_witness_names.contains(&sym("short_write")),
        "default method reachability should include concrete short_write witness: {checked_witness_names:?}"
    );
    assert!(
        module.body_diagnostics.is_empty(),
        "unmatched Writer witness diagnostics should not block executable checking: {:?}",
        module.body_diagnostics
    );
}

#[test]
fn executable_checked_modules_do_not_body_check_unreachable_globals() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
static unused = missing_symbol;

fn main() i32 {
0
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let unused = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Global && def.name == sym("unused"))
                .then_some(GlobalDefId { module_id, def_id })
        })
        .expect("unused global");

    assert!(
        module.body_diagnostics.is_empty(),
        "unreachable global body diagnostics should not block executable checking: {:?}",
        module.body_diagnostics
    );
    assert!(
        !module.body_ir.global_inits.contains_key(&unused),
        "unreachable global initializers should not be retained for executable codegen"
    );
}

#[test]
fn executable_checked_modules_do_not_retain_rejected_static_initializers() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
static used: i32 = { 1 };

fn main() i32 {
    used
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let used = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Global && def.name == sym("used"))
                .then_some(GlobalDefId { module_id, def_id })
        })
        .expect("used global");

    let diagnostics = resolve_diagnostic_bundle(db.context(), &module.body_diagnostics);
    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("global initializer is not representable as static data")),
        "rejected static initializer must retain its diagnostic: {diagnostics:?}"
    );
    assert!(
        !module.body_ir.global_inits.contains_key(&used),
        "a rejected static initializer must not enter executable Body IR"
    );
}

#[test]
fn executable_checked_modules_keep_type_owner_modules_type_only() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module types;
using entry::types;

fn main(value: types::Used) i32 {
value.value
}
"#,
    );
    let entry_id = fixture.entry_id();
    let types_id = fixture.add_child(
        entry_id,
        "types",
        "types.nia",
        r#"
pub struct Used {
value: i32,
}

pub fn unused_bad() i32 {
missing_symbol
}
"#,
    );
    let types_description = format!("{types_id:?}");
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let type_module = modules
        .iter()
        .find(|module| module.id == types_id)
        .expect("type owner module should be present for backend type lookup");
    assert!(
        type_module.executable_type_only,
        "type owner module should not be treated as an executable body module"
    );
    assert!(
        type_module.body_ir.function_bodies.is_empty(),
        "type owner module should not retain or check function bodies"
    );

    let trace = db.query_trace();
    assert!(
        !trace.queries.iter().any(|query| {
            query.frame.name == "executable_body_check"
                && query.frame.description.contains(&types_description)
                && query.stats.executions > 0
        }),
        "type owner module should not be executable-body-checked: {:?}",
        trace
            .queries
            .iter()
            .filter(|query| query.frame.name == "executable_body_check")
            .collect::<Vec<_>>()
    );
    assert!(
        trace.queries.iter().any(|query| {
            query.frame.name == "signature_type_lowering"
                && query.frame.description.contains(&types_description)
                && query.frame.description.contains("Types")
                && query.stats.executions > 0
        }),
        "type-only module should use signature type lowering: {:?}",
        trace
            .queries
            .iter()
            .filter(|query| query.frame.description.contains(&types_description))
            .collect::<Vec<_>>()
    );
    for full_query in ["type_resolution", "type_lowering", "value_resolution"] {
        assert!(
            !trace.queries.iter().any(|query| {
                query.frame.name == full_query
                    && query.frame.description.contains(&types_description)
                    && query.stats.executions > 0
            }),
            "type-only module should not execute {full_query}: {:?}",
            trace
                .queries
                .iter()
                .filter(|query| query.frame.name == full_query)
                .collect::<Vec<_>>()
        );
    }
}
