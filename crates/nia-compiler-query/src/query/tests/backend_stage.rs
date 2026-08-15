// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn backend_lowering_uses_executable_per_item_ir() {
    let fixture =
        LoadedProgramFixture::new("main.nia", "fn main() i32 { static value: i32 = 1; value }");
    let db = query_db(fixture.program());

    let _ = db.expect_get(BackendLoweringQuery);
    let trace = db.query_trace();

    assert!(trace_has_dependency(
        &trace,
        "backend_lowering",
        "backend_item_plan"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_item_plan",
        "backend_lowering_inputs"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_lowering",
        "backend_module_finalization"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_module_finalization",
        "backend_module_item_plan"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_module_finalization",
        "backend_finalization_task_context"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_finalization_task_context",
        "backend_lowering_inputs"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_lowering_inputs",
        "backend_module_source_item_plan"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_lowering_inputs",
        "backend_module_function_instance_plan"
    ));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "backend_lowering_inputs"
            && dependency.to.name == "executable_checked_modules"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "backend_lowering_inputs"
            && dependency.to.name == "full_active_module_item_tree"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "backend_lowering" && dependency.to.name == "checked_module_ids"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "backend_lowering_inputs"
            && dependency.to.name == "signature_item_tree"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "backend_lowering"
            && dependency.to.name == "program_full_defs_by_id"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "backend_lowering"
            && dependency.to.name == "program_backend_signatures"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "backend_lowering" && dependency.to.name == "const_enum_values"
    }));
    assert!(!depends_on_body_signature_query(&trace, "backend_lowering"));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "backend_lowering"
            && dependency.to.name == "program_type_normalizations"
    }));
}

#[test]
fn codegen_tracks_and_reuses_backend_stage_products() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module helper; using entry::helper; fn main() i32 { helper::id[i32](1) }",
    );
    let module_id = fixture.entry_id();
    let helper_id = fixture.add_child(
        module_id,
        "helper",
        "helper.nia",
        "pub fn id[T](value: T) T { value }",
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let first_codegen = db.expect_get(CodegenProgramQuery);
    assert!(
        first_codegen.diagnostics.is_empty(),
        "{:?}",
        first_codegen.diagnostics
    );
    let monomorphization = db.expect_get(MonomorphizationQuery);
    let entry_source_item_plan = db.expect_get(BackendModuleSourceItemPlanQuery(module_id));
    let helper_source_item_plan = db.expect_get(BackendModuleSourceItemPlanQuery(helper_id));
    let entry_function_instance_plan =
        db.expect_get(BackendModuleFunctionInstancePlanQuery(module_id));
    let helper_function_instance_plan =
        db.expect_get(BackendModuleFunctionInstancePlanQuery(helper_id));
    let backend_lowering = db.expect_get(BackendLoweringQuery);
    let second_codegen = db.expect_get(CodegenProgramQuery);
    let trace = db.query_trace();

    assert!(Arc::ptr_eq(&first_codegen, &second_codegen));
    assert!(Arc::ptr_eq(
        &first_codegen.monomorphization,
        &monomorphization.semantic
    ));
    assert!(Arc::ptr_eq(
        &first_codegen.backend_lowering,
        &backend_lowering.semantic
    ));
    assert!(trace_has_dependency(
        &trace,
        "codegen_program",
        "codegen_preparation"
    ));
    assert!(trace_has_dependency(
        &trace,
        "codegen_preparation",
        "monomorphization"
    ));
    assert!(trace_has_dependency(
        &trace,
        "codegen_program",
        "backend_lowering"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_lowering",
        "backend_item_plan"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_item_plan",
        "backend_lowering_inputs"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_lowering_inputs",
        "backend_module_source_item_plan"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_lowering_inputs",
        "backend_module_function_instance_plan"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_module_function_instance_plan",
        "monomorphization"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_lowering",
        "backend_module_finalization"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "backend_lowering",
        "monomorphization"
    ));
    assert_eq!(entry_source_item_plan.functions.len(), 1);
    assert_eq!(helper_source_item_plan.functions.len(), 1);
    assert!(entry_function_instance_plan.instances.is_empty());
    assert_eq!(helper_function_instance_plan.instances.len(), 1);
    assert_eq!(
        helper_function_instance_plan.instances[0].def_id.module_id,
        helper_id
    );
    assert_eq!(
        helper_function_instance_plan.instances[0].arg_module_id,
        module_id
    );
    assert_eq!(query_executions(&trace, "codegen_program"), 1);
    assert_eq!(query_executions(&trace, "monomorphization"), 1);
    assert_eq!(query_executions(&trace, "backend_item_plan"), 1);
    assert_eq!(query_executions(&trace, "backend_module_item_plan"), 0);
    assert_eq!(query_executions(&trace, "backend_lowering_inputs"), 1);
    assert_eq!(
        query_executions(&trace, "backend_finalization_task_context"),
        1
    );
    assert_eq!(query_executions(&trace, "backend_module_finalization"), 2);
    assert_eq!(
        query_executions(&trace, "backend_module_source_item_plan"),
        2
    );
    assert_eq!(
        query_executions(&trace, "backend_module_function_instance_plan"),
        2
    );
    assert_eq!(query_executions(&trace, "backend_lowering"), 1);
}

#[test]
fn const_only_generic_iteration_instances_do_not_enter_backend_plan() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
struct Pair[T] {
first: T,
second: T,
}

struct PairIter[T] {
first: T,
second: T,
index: usize,
}

extend[T] PairIter[T] : Iterator {
type Item = T;

const fn next(&mut self) ?T {
match self.index {
0usize => {
self.index += 1;
?self.first
},
1usize => {
self.index += 1;
?self.second
},
_ => null,
}
}
}

extend[T] Pair[T] : Iterable {
type Item = T;
type Iter = PairIter[T];

const fn iter(&self) PairIter[T] {
PairIter[T] { first: self.first, second: self.second, index: 0 }
}
}

const fn count[T](values: Pair[T]) usize {
let mut total: usize = 0;
for _ in values {
total += 1;
}
total
}

const compileCount: usize = count(Pair[u8] { first: 1u8, second: 2u8 });

fn main() usize {
let values: [u8; compileCount] = [0; compileCount];
count(Pair[usize] { first: 1, second: 2 })
+ count(Pair[bool] { first: true, second: false })
+ values[0] as usize
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let codegen = db.expect_get(CodegenProgramQuery);
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let def_id = |name, kind| {
        module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == kind && def.name == sym(name))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .unwrap_or_else(|| panic!("missing {kind:?} `{name}`"))
    };
    let count = def_id("count", nia_defs::DefKind::Function);
    let iter = def_id("iter", nia_defs::DefKind::Method);
    let next = def_id("next", nia_defs::DefKind::Method);
    let plan = db.expect_get(BackendModuleFunctionInstancePlanQuery(module_id));

    assert_eq!(
        plan.instances
            .iter()
            .filter(|instance| instance.def_id == count)
            .count(),
        2,
        "const-only generic count instance leaked into backend plan: {:?}",
        plan.instances
    );
    let lowered_instances = codegen
        .backend_lowering
        .program
        .modules
        .iter()
        .flat_map(|module| &module.function_instances)
        .collect::<Vec<_>>();
    for generic_def in [count, iter, next] {
        assert_eq!(
            lowered_instances
                .iter()
                .filter(|instance| instance.def_id == generic_def)
                .count(),
            2,
            "const-only generic iteration instance leaked into final backend program: {:?}",
            lowered_instances
        );
    }
}

#[test]
fn backend_module_plan_slots_are_consumed_and_republished_after_invalidation() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module helper; using entry::helper; fn main() i32 { helper::value() }",
    );
    let module_id = fixture.entry_id();
    let helper_id = fixture.add_child(
        module_id,
        "helper",
        "helper.nia",
        "pub fn value() i32 { 0 }",
    );
    let db = query_db(fixture.program());

    let first = db.expect_get(BackendLoweringQuery);
    assert!(first.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(db.context(), &first.diagnostics).is_empty(),
        "{:?}",
        resolve_diagnostic_bundle(db.context(), &first.diagnostics)
    );
    assert_eq!(
        first
            .semantic
            .program
            .modules
            .iter()
            .map(|module| module.id)
            .collect::<Vec<_>>(),
        vec![module_id, helper_id]
    );
    for owner in [module_id, helper_id] {
        assert!(
            db.get_owned(BackendModuleItemPlanQuery(owner)).is_err(),
            "finalization must leave no module-plan payload in its query slot"
        );
    }

    db.invalidate(BackendLoweringQuery);
    let second = db.expect_get(BackendLoweringQuery);
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);

    let invalidation = db.invalidate(BackendItemPlanQuery);
    assert!(
        invalidation
            .invalidated
            .iter()
            .any(|frame| { frame.name == "backend_module_item_plan" })
    );
    assert!(
        invalidation
            .invalidated
            .iter()
            .any(|frame| { frame.name == "backend_lowering" })
    );

    let third = db.expect_get(BackendLoweringQuery);
    assert!(third.diagnostics.is_empty(), "{:?}", third.diagnostics);
    let trace = db.query_trace();
    assert_eq!(query_executions(&trace, "backend_item_plan"), 3);
    assert_eq!(query_executions(&trace, "backend_module_item_plan"), 0);
    assert_eq!(query_executions(&trace, "backend_module_finalization"), 6);
    assert_eq!(
        query_executions(&trace, "backend_finalization_task_context"),
        1
    );
    assert_eq!(query_executions(&trace, "backend_lowering"), 3);
    assert!(trace_has_dependency(
        &trace,
        "backend_lowering",
        "backend_module_finalization"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_module_finalization",
        "backend_module_item_plan"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_module_finalization",
        "backend_finalization_task_context"
    ));
}

#[test]
fn backend_materializes_frontend_planned_source_functions() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module child;

fn helper() i32 {
7
}

fn unused() i32 {
9
}

fn main() i32 {
helper() + child::value()
}
"#,
    );
    let module_id = fixture.entry_id();
    let child_id = fixture.add_child(
        module_id,
        "child",
        "child.nia",
        r#"
pub struct Value {
number: i32,
}

pub fn value() i32 {
let value = Value { number: 5 };
value.number
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
    let module = facts
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module facts");
    let function = |name| {
        module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Function && def.name == sym(name))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .unwrap_or_else(|| panic!("missing function `{name}`"))
    };
    let helper = function("helper");
    let unused = function("unused");
    let main = function("main");

    let plan = db.expect_get(BackendModuleSourceItemPlanQuery(module_id));
    for items in [&plan.functions, &plan.globals, &plan.structs, &plan.unions] {
        assert!(items.windows(2).all(|pair| pair[0] < pair[1]), "{plan:?}");
        assert!(
            items.iter().all(|def_id| def_id.module_id == module_id),
            "{plan:?}"
        );
    }
    assert!(plan.functions.contains(&helper), "{plan:?}");
    assert!(plan.functions.contains(&main), "{plan:?}");
    assert!(!plan.functions.contains(&unused), "{plan:?}");
    assert!(plan.structs.is_empty(), "{plan:?}");

    let child_plan = db.expect_get(BackendModuleSourceItemPlanQuery(child_id));
    for items in [
        &child_plan.functions,
        &child_plan.globals,
        &child_plan.structs,
        &child_plan.unions,
    ] {
        assert!(
            items.windows(2).all(|pair| pair[0] < pair[1]),
            "{child_plan:?}"
        );
        assert!(
            items.iter().all(|def_id| def_id.module_id == child_id),
            "{child_plan:?}"
        );
    }
    assert_eq!(child_plan.functions.len(), 1, "{child_plan:?}");
    assert_eq!(child_plan.structs.len(), 1, "{child_plan:?}");

    let backend = db.expect_get(BackendLoweringQuery);
    assert!(backend.diagnostics.is_empty(), "{:?}", backend.diagnostics);
    let backend_module = backend
        .semantic
        .program
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry backend module");
    let functions = backend_module
        .functions
        .iter()
        .map(|function| function.def_id)
        .collect::<HashSet<_>>();
    assert!(functions.contains(&helper), "{functions:?}");
    assert!(functions.contains(&main), "{functions:?}");
    assert!(!functions.contains(&unused), "{functions:?}");

    let trace = db.query_trace();
    assert!(trace_has_dependency(
        &trace,
        "backend_lowering_inputs",
        "backend_module_source_item_plan"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_module_source_item_plan",
        "executable_checked_module_facts"
    ));
}

#[test]
fn codegen_reuses_per_function_lowering_between_mono_and_backend() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "fn helper() i32 { 1 } fn main() i32 { helper() }",
    );
    let db = query_db(fixture.program());

    let codegen = db.expect_get(CodegenProgramQuery);
    let trace = db.query_trace();

    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let body_count = codegen
        .modules
        .iter()
        .map(|module| module.body_ir.function_bodies.len())
        .sum::<usize>();
    assert_eq!(
        query_executions(&trace, "lowered_function_body"),
        body_count
    );
    assert!(
        query_cache_hits(&trace, "lowered_function_body") >= body_count,
        "backend lowering should reuse monomorphization's function products"
    );
    assert!(trace_has_dependency(
        &trace,
        "monomorphization",
        "lowered_function_body"
    ));
    assert!(trace_has_dependency(
        &trace,
        "backend_lowering_inputs",
        "lowered_function_body"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "codegen_program",
        "lowered_function_body"
    ));
    assert!(trace_has_dependency(
        &trace,
        "lowered_function_body",
        "executable_function_body"
    ));
}
