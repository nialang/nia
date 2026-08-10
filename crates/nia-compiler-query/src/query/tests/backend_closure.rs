// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn lowered_closure_entries_remain_owned_by_the_source_body_query() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn main(base: i32) i32 {
    let callback = [base](value: i32) i32 { base + value };
    let view: &Fn(i32) i32 = &callback;
    callback(1) + view(2)
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);
    let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
    let module = facts
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module facts");
    let main = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == sym("main")).then_some(GlobalDefId { module_id, def_id })
        })
        .expect("main definition");

    let lowered = db.expect_get(LoweredFunctionBodyQuery(main));
    assert!(lowered.diagnostic().is_none());
    assert_eq!(lowered.closure_entries().len(), 1);
    let entry = &lowered.closure_entries()[0];
    assert_eq!(entry.closure_id.owner, main);
    assert!(matches!(
        db.context().type_store.get(entry.body.locals[0].ty),
        Some(nia_ty::TyKind::Pointer {
            is_readonly: true,
            elem,
        }) if *elem == entry.state_ty
    ));
    assert!(
        lowered
            .body()
            .expect("source body")
            .blocks
            .iter()
            .flat_map(|block| &block.ops)
            .any(|op| matches!(
                op,
                nia_function_ir::FunctionOp::Binding(nia_function_ir::FunctionBinding {
                    value: Some(nia_function_ir::FunctionExpr {
                        kind: nia_function_ir::FunctionExprKind::CallableCoercion {
                            closure_id,
                            state,
                        },
                        ..
                    }),
                    ..
                }) if *closure_id == entry.closure_id
                    && matches!(state.kind, nia_function_ir::FunctionExprKind::AddrOf(_))
            ))
    );
    let tail = lowered
        .body()
        .expect("source body")
        .blocks
        .iter()
        .find_map(|block| match &block.terminator {
            nia_function_ir::FunctionTerminator::Tail {
                value: Some(value), ..
            } => Some(value),
            _ => None,
        })
        .expect("source tail");
    let nia_function_ir::FunctionExprKind::Call { args, .. } = &tail.kind else {
        panic!("expected addition call");
    };
    assert!(matches!(
        args[0].kind,
        nia_function_ir::FunctionExprKind::Call {
            callee: nia_function_ir::FunctionCallee::ClosureEntry { closure_id, .. },
            ..
        } if closure_id == entry.closure_id
    ));
    assert!(matches!(
        args[1].kind,
        nia_function_ir::FunctionExprKind::Call {
            callee: nia_function_ir::FunctionCallee::Callable(_),
            ..
        }
    ));

    let backend_inputs = db.expect_get(BackendLoweringInputsQuery);
    assert!(backend_inputs.semantic.is_some());
    assert!(resolve_diagnostic_bundle(db.context(), &backend_inputs.diagnostics).is_empty());

    let backend = db.expect_get(BackendLoweringQuery);
    assert!(resolve_diagnostic_bundle(db.context(), &backend.diagnostics).is_empty());
    let backend_module = backend
        .semantic
        .program
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry backend module");
    assert_eq!(backend_module.closure_entries.len(), 1);
    let backend_entry = &backend_module.closure_entries[0];
    assert_eq!(backend_entry.key.closure_id, entry.closure_id);
    assert_eq!(
        backend_entry.key.owner,
        nia_backend_ir::BackendClosureEntryOwner::Source(main)
    );
    let owner_symbol = nia_mangle::mangle_base_symbol_id(
        main,
        nia_mangle::MangleModuleId::from_normalized_source_path("main.nia"),
        sym("main"),
    );
    assert_eq!(
        backend_entry.symbol,
        nia_mangle::mangle_closure_entry_symbol(&owner_symbol, entry.closure_id)
    );
    assert_eq!(backend_entry.abi.state_type, entry.state_ty);
    assert_eq!(backend_entry.abi.params.len(), 1);
    assert_eq!(backend_entry.abi.params[0], entry.body.locals[1].ty);
    assert_eq!(backend_entry.abi.return_type, entry.return_type);
    assert_eq!(backend_entry.state_param, entry.state_param);
    assert_eq!(backend_entry.params, entry.params);
    assert_eq!(backend_entry.function_body, entry.body);
    assert!(matches!(
        db.context()
            .type_store
            .get(backend_entry.abi.state_pointer_type),
        Some(nia_ty::TyKind::Pointer {
            is_readonly: true,
            elem,
        }) if *elem == backend_entry.abi.state_type
    ));
}

#[test]
fn no_capture_function_pointer_retains_its_owned_closure_entry_identity() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn main() i32 {
    let callback = [](value: i32) i32 { value + 1 };
    let pointer: &fn(i32) i32 = &callback;
    pointer(2)
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);
    let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
    let module = facts
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module facts");
    let main = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == sym("main")).then_some(GlobalDefId { module_id, def_id })
        })
        .expect("main definition");

    let lowered = db.expect_get(LoweredFunctionBodyQuery(main));
    assert!(lowered.diagnostic().is_none());
    assert_eq!(lowered.closure_entries().len(), 1);
    let entry = &lowered.closure_entries()[0];
    assert_eq!(entry.closure_id.owner, main);
    assert!(lowered
        .body()
        .expect("source body")
        .blocks
        .iter()
        .flat_map(|block| &block.ops)
        .any(|op| matches!(
            op,
            nia_function_ir::FunctionOp::Binding(nia_function_ir::FunctionBinding {
                value: Some(nia_function_ir::FunctionExpr {
                    kind: nia_function_ir::FunctionExprKind::ClosureFunctionPointer { closure_id },
                    ..
                }),
                ..
            }) if *closure_id == entry.closure_id
        )));
}

#[test]
fn generic_function_instances_materialize_distinct_concrete_closure_entries() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn apply[T](value: T) T {
    let callback = [value]() T { value };
    callback()
}

fn main() i32 {
    apply[i32](7)
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let backend = db.expect_get(BackendLoweringQuery);
    assert!(resolve_diagnostic_bundle(db.context(), &backend.diagnostics).is_empty());
    let module = backend
        .semantic
        .program
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry backend module");
    let apply_instance = module
        .function_instances
        .iter()
        .find(|instance| instance.name == sym("apply"))
        .expect("apply[i32] instance");
    let entry = module
        .closure_entries
        .iter()
        .find(|entry| {
            matches!(
                &entry.key.owner,
                nia_backend_ir::BackendClosureEntryOwner::FunctionInstance(owner)
                    if owner.def_id == apply_instance.def_id
                        && owner.arg_module_id == apply_instance.arg_module_id
                        && owner.args == apply_instance.args
                        && owner.const_args == apply_instance.const_args
            )
        })
        .expect("concrete apply closure entry");

    assert_eq!(
        entry.symbol,
        nia_mangle::mangle_closure_entry_symbol(&apply_instance.symbol, entry.key.closure_id)
    );
    let Some(nia_ty::TyKind::ClosureState {
        captures,
        params,
        return_type,
        ..
    }) = db.context().type_store.get(entry.abi.state_type)
    else {
        panic!("expected instantiated closure-state ABI type");
    };
    let i32_ty = db
        .context()
        .type_store
        .append_for_module(module_id)
        .primitive(nia_ty::PrimitiveTy::I32);
    assert_eq!(captures, &[i32_ty]);
    assert!(params.is_empty());
    assert_eq!(*return_type, i32_ty);
    assert_eq!(entry.abi.return_type, i32_ty);
    assert!(entry.abi.params.is_empty());
}

#[test]
fn closure_entry_bodies_participate_in_backend_reachability() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn helper(value: i32) i32 { value + 1 }

fn main() i32 {
    let callback = [](value: i32) i32 { helper(value) };
    callback(1)
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let backend = db.expect_get(BackendLoweringQuery);
    assert!(resolve_diagnostic_bundle(db.context(), &backend.diagnostics).is_empty());
    let module = backend
        .semantic
        .program
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry backend module");
    assert_eq!(module.closure_entries.len(), 1);
    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == sym("helper")),
        "helper referenced only by the closure entry must remain reachable"
    );
}

#[test]
fn executable_backend_lowering_skips_unreachable_recursive_aggregates() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
struct Recursive {
next: Recursive,
}

fn unused(value: Recursive) i32 {
1
}

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
    assert!(
        resolve_diagnostic_bundle(db.context(), &module.layout_diagnostics).is_empty(),
        "unreachable recursive aggregate should not force layout diagnostics: {:?}",
        resolve_diagnostic_bundle(db.context(), &module.layout_diagnostics)
    );

    let backend_lowering = db.expect_get(BackendLoweringQuery);
    let backend_module = backend_lowering
        .semantic
        .program
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be backend-lowered");
    let recursive = sym("Recursive");
    assert!(
        backend_module
            .structs
            .iter()
            .all(|item| item.name != recursive),
        "unreachable recursive aggregate should not be lowered for codegen"
    );
}

#[test]
fn executable_backend_lowering_uses_canonical_external_extension_where_predicates() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module ext;
module bounds;
using entry::ext;
using entry::bounds;

fn main() i32 {
let value = ext::Box[bounds::Token]::init(bounds::Token {});
value.get()
}
"#,
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "ext",
        "ext.nia",
        r#"
using entry::bounds;

pub struct Box[T]
where T: bounds::Marker
{
value: T,
}

extend[T] Box[T]
where T: bounds::Marker
{
pub fn init(value: T) Box[T] {
    { value: value }
}

pub fn get(self) i32 {
    1
}
}
"#,
    );
    fixture.add_child(
        entry_id,
        "bounds",
        "bounds.nia",
        r#"
pub trait Marker {}

pub struct Token {}

extend Token : Marker {}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let backend_lowering = db.expect_get(BackendLoweringQuery);

    assert!(
        backend_lowering.diagnostics.is_empty(),
        "backend lowering should import external extension owner predicates without diagnostics: {:?}",
        backend_lowering.diagnostics
    );
}

#[test]
fn executable_backend_lowering_includes_shallow_primitive_extension_owners() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module unicode;
using entry::unicode;

fn main() i32 {
'a'.encoded_len()
}
"#,
    );
    let entry_id = fixture.entry_id();
    let unicode_id = fixture.add_shallow_child(
        entry_id,
        "unicode",
        "unicode.nia",
        r#"
extend char {
pub fn encoded_len(self) i32 {
    _ = self;
    1
}
}
"#,
    );
    fixture.graph.mark_semantic_selected(unicode_id);
    let unused_id = fixture.add_shallow_child(
        entry_id,
        "unused",
        "unused.nia",
        r#"
extend i32 {
pub fn unreachable(self) i32 {
    missing_symbol
}
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let defs = module_defs_semantic(&db, unicode_id).expect("unicode defs");
    let method = defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Method && def.name == sym("encoded_len")).then_some(
                GlobalDefId {
                    module_id: unicode_id,
                    def_id,
                },
            )
        })
        .expect("encoded_len extension method");
    let signatures = db.expect_get(SignatureItemSignaturesQuery(
        unicode_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    assert!(
        signatures
            .semantic
            .functions
            .get(&method.def_id)
            .is_some_and(|signature| signature.has_body),
        "shallow extension signature should retain its runtime body contract"
    );

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    assert!(
        modules.iter().all(|module| module.id != unused_id),
        "parse-ok shallow providers without reachable bodies must remain unchecked"
    );
    let entry = modules
        .iter()
        .find(|module| module.id == entry_id)
        .expect("entry module should be executable-reachable");
    assert!(
        entry
            .semantic_facts
            .iter_node_resolved_calls()
            .map(|(_, call)| call)
            .any(|call| matches!(call, nia_sema_ir::ResolvedCall::Method { def_id, .. } if *def_id == method)),
        "entry semantic facts should retain the resolved cross-module extension call"
    );
    assert!(
        entry.semantic_facts.function_facts.values().any(|facts| {
            facts.node_resolved_calls.values().any(
                |call| matches!(call, nia_sema_ir::ResolvedCall::Method { def_id, .. } if *def_id == method),
            )
        }),
        "the resolved extension call should be attributed to its calling function"
    );
    let unicode = modules
        .iter()
        .find(|module| module.id == unicode_id)
        .expect("shallow primitive extension owner should be executable-reachable");
    assert!(
        unicode.body_ir.function_bodies.contains_key(&method),
        "reachable primitive extension method body should be checked"
    );

    let backend = db.expect_get(BackendLoweringQuery);
    assert!(
        backend
            .semantic
            .program
            .modules
            .iter()
            .any(|module| module.id == unicode_id),
        "reachable primitive extension owner should be present in the backend module plan"
    );
}

#[test]
fn executable_backend_signatures_include_checked_shallow_provider_impls() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module provider;
using entry::provider;

fn main() i32 {
'a'.activate()
}
"#,
    );
    let entry_id = fixture.entry_id();
    let provider_id = fixture.add_shallow_child(
        entry_id,
        "provider",
        "provider.nia",
        r#"
pub trait Value {
fn value(self) i32;
}

extend char : Value {
fn value(self) i32 {
_ = self;
1
}
}

extend[T] T
where T: Value
{
pub fn activate(self) i32 {
self.value()
}
}
"#,
    );
    fixture.graph.mark_semantic_selected(provider_id);
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    assert!(
        modules.iter().any(|module| module.id == provider_id),
        "the selected shallow provider should be executable-reachable"
    );

    let backend = db.expect_get(BackendLoweringQuery);
    assert!(
        backend.diagnostics.is_empty(),
        "checked provider trait methods should lower without diagnostics: {:?}",
        backend.diagnostics
    );
}

#[test]
fn executable_backend_lowering_includes_cross_module_trait_default_vtable_instances() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module module1;
module module2;
using entry::module1;
using entry::module2;

fn main() i32 {
let mut page = module2::Page::init();
let allocator: &mut module1::Allocator = &mut page;
allocator.remap()
}
"#,
    );
    let entry_id = fixture.entry_id();
    let module1_id = fixture.add_child(
        entry_id,
        "module1",
        "module1.nia",
        r#"
pub trait Allocator {
fn alloc(&mut self) i32;

fn remap(&mut self) i32 {
    _ = self;
    helper()
}
}

fn helper() i32 {
7
}
"#,
    );
    fixture.add_child(
        entry_id,
        "module2",
        "module2.nia",
        r#"
using entry::module1;
using module1::Allocator;

pub struct Page {}

extend Page {
pub fn init() Page {
    {}
}
}

extend Page : Allocator {
fn alloc(&mut self) i32 {
    _ = self;
    7
}
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let codegen = CompilerDatabase::new(
        CompileRequest::new(loaded).with_optimization(NiaOptimizationLevel::O1),
    )
    .codegen_program();

    let backend_lowering = &codegen.backend_lowering;

    assert!(
        backend_lowering.diagnostics.is_empty(),
        "backend lowering should not report diagnostics: {:?}",
        backend_lowering.diagnostics
    );
    let vtable_instance_refs = backend_lowering
        .program
        .modules
        .iter()
        .flat_map(|module| &module.trait_object_vtables)
        .flat_map(|vtable| &vtable.entries)
        .filter_map(|entry| match &entry.function {
            nia_backend_ir::BackendTraitObjectVtableFunction::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => Some((
                *def_id,
                *arg_module_id,
                *self_arg,
                args.clone(),
                const_args.clone(),
            )),
            nia_backend_ir::BackendTraitObjectVtableFunction::Function(_) => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !vtable_instance_refs.is_empty(),
        "trait object vtable should reference a default method instance"
    );
    for (def_id, arg_module_id, self_arg, args, const_args) in vtable_instance_refs {
        let matches = backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| {
                module
                    .function_instances
                    .iter()
                    .map(move |instance| (module, instance))
            })
            .filter(|(_, instance)| {
                backend_function_instance_matches_vtable_ref(
                    &codegen.type_store,
                    VtableFunctionInstanceRef {
                        def_id,
                        arg_module_id,
                        self_arg,
                        args: &args,
                        const_args: &const_args,
                    },
                    instance,
                )
            })
            .count();
        assert_eq!(
            matches, 1,
            "expected one lowered vtable function instance for {def_id:?}"
        );
    }
    let helper = backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.id == module1_id)
        .expect("trait owner module should be backend-lowered")
        .functions
        .iter()
        .find(|function| function.name == sym("helper"))
        .expect("default method helper should be materialized");
    assert!(
        backend_lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                nia_backend_lower::BackendOptimizationChange::Function {
                    module_id,
                    function,
                    pass: "inline-leaf-functions",
                    is_instance: false,
                    ..
                } if *module_id == module1_id && *function == helper.def_id
            )),
        "the vtable-induced default instance should be finalized after closure"
    );
}

#[test]
fn executable_backend_lowering_closes_vtables_from_generic_function_instances() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module dispatch;
using entry::dispatch;

fn main() i32 {
let mut page = dispatch::Page::init();
dispatch::call[dispatch::Page](&mut page)
}
"#,
    );
    let entry_id = fixture.entry_id();
    let dispatch_id = fixture.add_child(
        entry_id,
        "dispatch",
        "dispatch.nia",
        r#"
pub trait Allocator {
fn alloc(&mut self) i32;

fn remap(&mut self) i32 {
    self.alloc()
}
}

pub struct Page {}

extend Page {
pub fn init() Page {
    {}
}
}

extend Page : Allocator {
fn alloc(&mut self) i32 {
    _ = self;
    7
}
}

pub fn call[T](value: &mut T) i32
where T: Allocator
{
let allocator: &mut Allocator = value;
allocator.remap()
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let backend_lowering = db.expect_get(BackendLoweringQuery);
    assert!(
        backend_lowering.diagnostics.is_empty(),
        "backend lowering should not report diagnostics: {:?}",
        backend_lowering.diagnostics
    );
    let dispatch_module = backend_lowering
        .semantic
        .program
        .modules
        .iter()
        .find(|module| module.id == dispatch_id)
        .expect("generic function owner module should be backend-lowered");
    assert_eq!(
        dispatch_module
            .function_instances
            .iter()
            .filter(|instance| instance.name == sym("call"))
            .count(),
        1,
        "the concrete generic function should be materialized once"
    );
    assert_eq!(
        dispatch_module.trait_object_vtables.len(),
        1,
        "the substituted generic body should discover one trait-object vtable"
    );
    let vtable_instance_refs = dispatch_module
        .trait_object_vtables
        .iter()
        .flat_map(|vtable| &vtable.entries)
        .filter_map(|entry| match &entry.function {
            nia_backend_ir::BackendTraitObjectVtableFunction::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => Some(VtableFunctionInstanceRef {
                def_id: *def_id,
                arg_module_id: *arg_module_id,
                self_arg: *self_arg,
                args,
                const_args,
            }),
            nia_backend_ir::BackendTraitObjectVtableFunction::Function(_) => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !vtable_instance_refs.is_empty(),
        "the vtable should reference the default method instance"
    );
    for vtable_ref in vtable_instance_refs {
        assert_eq!(
            dispatch_module
                .function_instances
                .iter()
                .filter(|instance| backend_function_instance_matches_vtable_ref(
                    &db.context().type_store,
                    VtableFunctionInstanceRef {
                        def_id: vtable_ref.def_id,
                        arg_module_id: vtable_ref.arg_module_id,
                        self_arg: vtable_ref.self_arg,
                        args: vtable_ref.args,
                        const_args: vtable_ref.const_args,
                    },
                    instance,
                ))
                .count(),
            1,
            "each vtable method instance should be materialized once"
        );
    }
}

#[test]
fn executable_backend_lowering_assigns_repeated_vtable_to_one_stable_owner() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module common;
module left;
module right;
using entry::left;
using entry::right;

fn main() i32 {
left::read() + right::read()
}
"#,
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "common",
        "common.nia",
        r#"
pub trait Value {
fn read(& self) i32;
}

pub struct Cell {}

extend Cell : Value {
fn read(& self) i32 {
    _ = self;
    7
}
}
"#,
    );
    let left_id = fixture.add_child(
        entry_id,
        "left",
        "left.nia",
        r#"
using entry::common;

pub fn read() i32 {
let cell: common::Cell = {};
let value: &common::Value = &cell;
value.read()
}
"#,
    );
    let right_id = fixture.add_child(
        entry_id,
        "right",
        "right.nia",
        r#"
using entry::common;

pub fn read() i32 {
let cell: common::Cell = {};
let value: &common::Value = &cell;
value.read()
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;

    let backend = query_db(loaded).expect_get(BackendLoweringQuery);
    assert!(backend.diagnostics.is_empty(), "{:?}", backend.diagnostics);
    let owners = backend
        .semantic
        .program
        .modules
        .iter()
        .filter(|module| !module.trait_object_vtables.is_empty())
        .map(|module| module.id)
        .collect::<Vec<_>>();

    assert_eq!(owners, vec![left_id]);
    assert_ne!(left_id, right_id);
}

#[test]
fn executable_backend_lowering_closes_cross_module_generic_local_static_instances() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module slots;
using entry::slots;

fn main() i32 {
let mut left = slots::slot[i32]();
let mut right = slots::slot[u64]();
_ = left;
_ = right;
0
}
"#,
    );
    let entry_id = fixture.entry_id();
    let slots_id = fixture.add_child(
        entry_id,
        "slots",
        "slots.nia",
        r#"
pub fn slot[T]() &mut T {
static mut item: T;
&mut item
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let backend_lowering = db.expect_get(BackendLoweringQuery);
    assert!(
        backend_lowering.diagnostics.is_empty(),
        "backend lowering should not report diagnostics: {:?}",
        backend_lowering.diagnostics
    );
    let slots_module = backend_lowering
        .semantic
        .program
        .modules
        .iter()
        .find(|module| module.id == slots_id)
        .expect("generic function owner module should be backend-lowered");
    let item_instances = slots_module
        .global_instances
        .iter()
        .filter(|instance| instance.name == sym("item"))
        .collect::<Vec<_>>();

    assert_eq!(item_instances.len(), 2);
    assert!(item_instances.iter().any(|instance| matches!(
        db.context().type_store.get(instance.ty),
        Some(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))
    )));
    assert!(item_instances.iter().any(|instance| matches!(
        db.context().type_store.get(instance.ty),
        Some(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::U64))
    )));
}
