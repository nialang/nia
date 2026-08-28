// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn lowers_checked_program_shape() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let source = r#"
static hello = b"hello\0";

struct Point {
    x: i32,
    y: i32,
}

struct Unused {
    value: i64,
}

union UnusedPayload {
    value: i64,
}

extend Point {
    fn make(x: i32, y: i32) Point {
        Self { x, y }
    }
}

fn main() i32 {
    let mut p = Point::make(1, 2);
    p.x
}
"#;
    let symbols = SymbolTable::new();
    let (module, errors) = parse_module_with_symbols(source, symbols.clone());
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let type_resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let type_store = nia_ty::TypeStore::new();
    let type_lowering = lower_module_types_with_context(
        module_id,
        &module,
        &type_resolved,
        TypeLoweringContext::empty(&type_store).with_symbols(&symbols),
    );
    let signatures = collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(&module),
        defs: &defs,
        lowered: &type_lowering,
        type_store: &type_store,
        symbols: None,
    });
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let active_item_tree = active_item_tree(&module);
    let semantic_uses = semantic_use_table(
        module_id,
        &values,
        &locals,
        &type_lowering,
        &active_item_tree,
    );
    let normalization_input = type_lowering.explicit_type_roots();
    let normalization = normalize_module_types(nia_type_normalize::TypeNormalizationInput {
        module_id,
        type_store: &type_store,
        input_ids: &normalization_input,
        signatures: &signatures,
    });
    let target = nia_target_config::TargetConfig::host();
    let source_path = nia_source::SourcePath::new("/tmp/nia-backend-lower-test/lowering.nia");
    let const_module = nia_const_check::lower_module_const(nia_const_check::ConstModuleInput {
        active_item_tree: &active_item_tree,
        defs: &defs,
        signatures: &signatures,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        const_exprs: &type_lowering.const_exprs,
        source_path: &source_path,
    });
    assert!(
        const_module.diagnostics.is_empty(),
        "{:?}",
        const_module.diagnostics
    );
    let const_input = nia_const_check::ConstInput {
        type_store: &type_store,
        module: &const_module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        lowered: &type_lowering,
        signatures: &signatures,
        normalization: &normalization,
        target: &target,
        source_path: &source_path,
        program: nia_const_check::ConstProgramContext::empty(),
    };
    let const_eval = nia_const_check::check_module_const(const_input);
    let root_types = signatures.type_roots();
    let layouts =
        nia_layout::compute_layouts_with_program_context(nia_layout::LayoutComputationInput {
            type_store: &type_store,
            defs: &defs,
            signatures: &signatures,
            root_types: &root_types,
            normalized: &normalization.normalized,
            array_lengths: &|id| const_eval.array_lengths.get(&id).copied(),
            target: nia_layout::TargetDataLayout::LP64,
            program: nia_layout::ProgramLayoutContext::default(),
        });
    let _abi = check_module_abi(&defs, &type_store, &signatures);
    let _flow = check_module_flow(&module, &type_store, &signatures);
    let point_id = defs
        .module_scope
        .types
        .get(&sym("Point"))
        .expect("Point def");
    let reachable_structs = vec![GlobalDefId {
        module_id,
        def_id: point_id,
    }];
    let reachable_unions = Vec::new();
    let make_id = defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == DefKind::Method && def.name == sym("make")).then_some(def_id)
        })
        .expect("make def");
    let mut extensions = VisibleExtensionMethods::default();
    let point_ty = type_lowering
        .explicit_type_roots()
        .into_iter()
        .find(|ty_id| {
            matches!(
                type_store.get(*ty_id),
                Some(nia_ty::TyKind::Nominal {
                    def_id,
                    args,
                    ..
                }) if def_id.module_id == module_id && def_id.def_id == point_id && args.is_empty()
            )
        })
        .expect("Point type");
    let impl_id = signatures.trait_impls[0].impl_id;
    extensions.insert(
        impl_id,
        point_ty,
        VisibleExtensionMethod {
            name: sym("make"),
            def_id: GlobalDefId {
                module_id,
                def_id: make_id,
            },
            impl_id,
            effective_generics: Vec::new(),
            effective_const_generics: Vec::new(),
            trait_id: None,
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            where_predicates: Vec::new(),
            is_callable: true,
            is_trait_witness: false,
        },
    );
    let origins = NodeOriginTable::default();
    let program_signatures = EmptyBodyProgramSignatures::new();
    let const_array_lengths = nia_const_check::ConstArrayLengths {
        values: const_eval.array_lengths.clone(),
        diagnostics: Vec::new(),
    };
    let const_values = nia_const_check::ConstValues {
        values: const_eval.values.clone(),
        typed_values: const_eval.typed_values.clone(),
        diagnostics: Vec::new(),
    };
    let const_typed_facts = nia_const_check::ConstTypedFacts {
        typed_values: const_eval.typed_values.clone(),
        diagnostics: Vec::new(),
    };
    let body_const = nia_body_check::BodyConst::from_phases(
        &const_values,
        &const_array_lengths,
        &const_typed_facts,
    );
    let body_check = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        type_store: &type_store,
        source_version: None,
        source_path: &source_path,
        symbols: &symbols,
        origins: &origins,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        lowered: &type_lowering,
        signatures: nia_body_check::BodyLocalSignatures::from_item_signatures(&signatures),
        const_signatures: &signatures,
        normalization: &normalization,
        seed: None,
        target: &target,
        const_eval: body_const,
        const_module: &const_module.module,
        layouts: &layouts,
        extensions: &extensions,
        lazy_extensions: None,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        program: nia_body_check::BodyProgramContext::empty(),
        program_signatures: program_signatures.context(),
        function_scope: nia_body_check::FunctionCheckScope::LocalModule,
        program_const: nia_body_check::ProgramConstMaps::empty(),
        filter: nia_body_check::BodyCheckFilter::All,
        product: nia_body_check::BodyCheckProduct::Full,
        prechecked: None,
    });
    assert!(
        body_check.diagnostics.is_empty(),
        "{:?}",
        body_check.diagnostics
    );
    let function_bodies = body_check
        .ir
        .function_bodies
        .iter()
        .map(|(def_id, body)| {
            (
                *def_id,
                lower_function_body(
                    module_id,
                    body,
                    FunctionTypeContext::for_module(&type_store, module_id),
                )
                .expect("valid typed body")
                .body,
            )
        })
        .collect::<HashMap<_, _>>();
    let program_const = HashMap::from([(module_id, const_array_lengths.values.as_ref())]);
    let const_enum_values = const_enum_values_from_check(&const_eval);
    let program_function_bodies = function_bodies
        .iter()
        .map(|(def_id, body)| (*def_id, body))
        .collect::<HashMap<_, _>>();
    let program_static_inits = body_check
        .ir
        .global_inits
        .iter()
        .map(|(def_id, init)| (*def_id, init.as_ref()))
        .collect::<HashMap<_, _>>();
    let program = TestBackendProgramFacts::new(
        module_id,
        program_const,
        program_function_bodies,
        program_static_inits,
    );
    let function_instance_plan = Vec::new();

    let input = BackendLowerModuleInput {
        module_id,
        source_identity: nia_source::SourceIdentity::new("main"),
        module_name: "main".to_string(),
        symbols: &symbols,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        type_lowering: &type_lowering,
        signatures: &signatures,
        type_normalization: &normalization,
        semantic_facts: &body_check.facts,
        extensions: &extensions,
        const_array_lengths: const_array_lengths.values.as_ref(),
        const_enum_values: const_enum_values.values.as_ref(),
        layouts: &layouts,
        roots: BackendFunctionRoots::FunctionBodies,
        reachable_functions: None,
        reachable_globals: None,
        reachable_structs: Some(&reachable_structs),
        reachable_unions: Some(&reachable_unions),
        function_instance_plan: &function_instance_plan,
        program: &program,
    };
    let optimization = nia_opt::OptimizationPolicy::default();
    let inputs = [input];
    let plan = plan_backend_program(&inputs, &type_store, optimization);
    assert!(plan.diagnostics().is_empty(), "{:?}", plan.diagnostics());
    assert_eq!(plan.modules().len(), 1);
    assert_eq!(plan.optimization(), optimization);
    let planned_functions = plan.modules()[0].module().functions.as_ptr();
    let planned_globals = plan.modules()[0].module().globals.as_ptr();
    let (finalization, module_plans) = plan.into_module_plans();
    let lowering =
        finalize_backend_module_item_plans(&inputs, &type_store, finalization, module_plans);
    assert!(
        lowering.diagnostics.is_empty(),
        "{:?}",
        lowering.diagnostics
    );
    assert_eq!(lowering.program.modules.len(), 1);
    assert_eq!(lowering.program.modules[0].globals.len(), 1);
    assert_eq!(lowering.program.modules[0].functions.len(), 2);
    assert_eq!(lowering.program.modules[0].structs.len(), 1);
    assert_eq!(
        lowering.program.modules[0].structs[0].def_id.def_id,
        point_id
    );
    assert!(lowering.program.modules[0].unions.is_empty());
    assert_eq!(
        lowering.program.modules[0].functions.as_ptr(),
        planned_functions
    );
    assert_eq!(
        lowering.program.modules[0].globals.as_ptr(),
        planned_globals
    );
}

fn const_enum_values_from_check(
    const_eval: &nia_const_check::ConstCheck,
) -> nia_const_check::ConstEnumValues {
    nia_const_check::ConstEnumValues {
        values: const_eval.enum_values.clone(),
        typed_values: const_eval.typed_enum_values.clone(),
        diagnostics: Vec::new(),
    }
}

#[test]
fn const_bindings_do_not_lower_to_storage() {
    let source = r#"
const answer: i32 = 40 + 2;

fn main() i32 {
    const local: i32 = answer;
    local
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    assert!(module.globals.is_empty());
    let main = module
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    assert!(
        main.function_body
            .as_ref()
            .expect("main function body")
            .locals
            .iter()
            .all(|local| local.name != local_name("local"))
    );
}

#[test]
fn lowers_large_array_repeat_count_from_const_binding() {
    let source = r#"
const N: usize = 1048576;

fn main() i32 {
    let mut buffer: [u8; N] = [0u8; N];
    0
}
"#;
    let lowering = lower_source(source);
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let Some(FunctionOp::Binding(binding)) = body.blocks[0].ops.first() else {
        panic!("expected buffer binding");
    };
    let value = binding.value.as_ref().expect("buffer initializer");
    let FunctionExprKind::ArrayLiteral {
        elems: FunctionArrayElements::Repeat { count, .. },
    } = &value.kind
    else {
        panic!("expected repeat array initializer");
    };
    let nia_ty::ArrayLenTy::ConstExpr(id) = count else {
        panic!("expected const expr repeat count, got {count:?}");
    };
    assert_eq!(
        lowering.program.modules[0]
            .const_eval
            .array_lengths
            .get(id)
            .copied(),
        Some(1048576)
    );
}

#[test]
fn lowers_function_body_to_function_ir() {
    let source = r#"
fn main() i32 {
    defer {
    };
    let mut value = 1;
    return value;
}
"#;
    let lowering = lower_source(source);
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let function_body = main.function_body.as_ref().expect("main function body");
    assert_eq!(function_body.blocks.len(), 2);
    assert!(matches!(
        function_body.blocks[0].ops[0],
        FunctionOp::Defer(_)
    ));
    assert!(matches!(
        function_body.blocks[0].ops[1],
        FunctionOp::Binding(_)
    ));
    let FunctionTerminator::Next { target, .. } = function_body.blocks[0].terminator else {
        panic!("expected first block to continue to return terminator block");
    };
    assert!(matches!(
        function_body
            .block(target)
            .expect("return terminator block")
            .terminator,
        FunctionTerminator::Return { value: Some(_), .. }
    ));
}

#[test]
fn lowers_loop_break_and_continue_to_function_ir_branches() {
    let source = r#"
fn main() i32 {
    loop {
        continue;
        break;
    }
    0
}
"#;
    let lowering = lower_source(source);
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let function_body = main.function_body.as_ref().expect("main function body");
    let FunctionTerminator::Next { target, .. } = function_body.blocks[0].terminator else {
        panic!("expected entry branch to loop header");
    };
    let FunctionTerminator::Loop {
        body,
        continue_target,
        ..
    } = function_body.block(target).expect("loop header").terminator
    else {
        panic!("expected loop terminator");
    };
    let body = function_body
        .blocks
        .iter()
        .find(|block| block.id == body)
        .expect("loop body block");
    assert_eq!(body.terminator.successors(), vec![continue_target]);
}

#[test]
fn instantiates_generic_function_instances_in_function_ir() {
    let source = r#"
fn id[T](value: T) T {
    value
}

fn main() i32 {
    id[i32](42)
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let interner = lowering.append(module.id);
    let instance = module
        .function_instances
        .iter()
        .find(|instance| instance.name == sym("id"))
        .expect("id instance");
    let body = instance
        .function_body
        .as_ref()
        .expect("id instance function body");
    let i32_ty = interner.primitive(nia_ty::PrimitiveTy::I32);

    assert_eq!(instance.params[0].passing_ty, i32_ty);
    assert_eq!(instance.params[0].local_ty, i32_ty);
    assert_eq!(instance.return_type, i32_ty);
    assert_eq!(body.ty, i32_ty);
    assert!(body.locals.iter().all(|local| local.ty == i32_ty));
}

#[test]
fn instantiates_const_generic_function_array_lengths_in_function_ir() {
    let source = r#"
fn take[T, N: usize](items: [T; N]) usize {
    N
}

fn main() usize {
    take([1u8, 2u8, 3u8, 4u8]) + take([1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8])
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let interner = lowering.append(module.id);
    let u8_ty = interner.primitive(nia_ty::PrimitiveTy::U8);
    let usize_ty = interner.primitive(nia_ty::PrimitiveTy::Usize);
    let mut instances = module
        .function_instances
        .iter()
        .filter(|instance| instance.name == sym("take"))
        .collect::<Vec<_>>();
    instances.sort_by_key(|instance| {
        instance
            .const_args
            .first()
            .and_then(|arg| match arg.value {
                nia_ty::ConstGenericValue::Int(value) => Some(value.bits() as u64),
                _ => None,
            })
            .unwrap_or(0)
    });

    assert_eq!(instances.len(), 2);
    for (instance, expected_len) in instances.into_iter().zip([4, 8]) {
        assert_eq!(instance.args, vec![u8_ty]);
        assert_eq!(instance.const_args.len(), 1);
        assert_eq!(instance.const_args[0].ty, usize_ty);
        assert!(matches!(
            instance.const_args[0].value,
            nia_ty::ConstGenericValue::Int(value) if value.bits() == expected_len
        ));
        assert!(matches!(
            lowering.type_store.get(instance.params[0].local_ty),
            Some(nia_ty::TyKind::Array {
                len: nia_ty::ArrayLenTy::ConstValue(len),
                elem,
            }) if *len == expected_len as u64 && *elem == u8_ty
        ));
        assert_eq!(instance.return_type, usize_ty);
    }
}

#[test]
fn instantiates_interleaved_type_and_const_function_generics_by_kind() {
    let source = r#"
fn choose[T, N: usize, U](left: T, right: U) U {
    right
}

fn main() i64 {
    choose[i32, 3, i64](1, 9i64)
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let interner = lowering.append(module.id);
    let instance = module
        .function_instances
        .iter()
        .find(|instance| instance.name == sym("choose"))
        .expect("choose instance");
    let i32_ty = interner.primitive(nia_ty::PrimitiveTy::I32);
    let i64_ty = interner.primitive(nia_ty::PrimitiveTy::I64);

    assert_eq!(instance.args, vec![i32_ty, i64_ty]);
    assert_eq!(instance.const_args.len(), 1);
    assert_eq!(instance.params[0].local_ty, i32_ty);
    assert_eq!(instance.params[1].local_ty, i64_ty);
    assert_eq!(instance.return_type, i64_ty);
    assert_eq!(
        instance.function_body.as_ref().map(|body| body.ty),
        Some(i64_ty)
    );
}

#[test]
fn instantiates_nested_const_generic_callee_identity() {
    let source = r#"
fn inner[N: usize]() usize {
    N
}

fn outer[N: usize]() usize {
    inner[N]()
}

fn main() usize {
    outer[3]()
}
"#;
    let lowering = lower_source(source);
    let outer = lowering.program.modules[0]
        .function_instances
        .iter()
        .find(|instance| instance.name == sym("outer"))
        .expect("outer instance");
    let value = first_terminal_value(outer.function_body.as_ref().expect("outer body"));
    let FunctionExprKind::Call {
        callee:
            nia_function_ir::FunctionCallee::FunctionInstance {
                const_args, args, ..
            },
        ..
    } = &value.kind
    else {
        panic!("expected concrete inner function instance call: {value:?}");
    };

    assert!(args.is_empty());
    assert!(matches!(
        const_args.as_slice(),
        [nia_ty::ConstGenericArg {
            value: nia_ty::ConstGenericValue::Int(value),
            ..
        }] if value.bits() == 3
    ));
}

#[test]
fn instantiates_nominal_const_generic_array_lengths() {
    let source = r#"
struct Buffer[T, N: usize] {
    data: [T; N],
}

fn make4() Buffer[u8, 4] {
    Buffer[u8, 4] { data: [1u8, 2u8, 3u8, 4u8] }
}

fn make8() Buffer[u8, 8] {
    Buffer[u8, 8] { data: [1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8] }
}

fn main() usize {
    let a = make4();
    let b = make8();
    a.data[0] as usize + b.data[0] as usize
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let interner = lowering.append(module.id);
    let u8_ty = interner.primitive(nia_ty::PrimitiveTy::U8);
    let usize_ty = interner.primitive(nia_ty::PrimitiveTy::Usize);
    let mut instances = module
        .struct_instances
        .iter()
        .filter(|instance| instance.name == sym("Buffer"))
        .collect::<Vec<_>>();
    instances.sort_by_key(|instance| {
        instance
            .const_args
            .first()
            .and_then(|arg| match arg.value {
                nia_ty::ConstGenericValue::Int(value) => Some(value.bits() as u64),
                _ => None,
            })
            .unwrap_or(0)
    });

    assert_eq!(instances.len(), 2);
    for (instance, expected_len) in instances.into_iter().zip([4, 8]) {
        assert_eq!(instance.args, vec![u8_ty]);
        assert_eq!(instance.const_args.len(), 1);
        assert_eq!(instance.const_args[0].ty, usize_ty);
        assert!(matches!(
            instance.const_args[0].value,
            nia_ty::ConstGenericValue::Int(value) if value.bits() == expected_len
        ));
        assert_eq!(instance.fields.len(), 1);
        assert!(matches!(
            lowering.type_store.get(instance.fields[0].ty),
            Some(nia_ty::TyKind::Array {
                len: nia_ty::ArrayLenTy::ConstValue(len),
                elem,
            }) if *len == expected_len as u64 && *elem == u8_ty
        ));
    }
}

#[test]
fn materializes_struct_instance_referenced_only_by_array_layout_metadata() {
    let source = r#"
struct Inner[T] {
    value: T,
}

struct Wrapper {
    data: [u8; std::builtin::size[Inner[i32]]()],
}

fn main(value: Wrapper) usize {
    value.data[0] as usize
}
"#;
    let lowering = lower_source(source);
    assert!(
        lowering.diagnostics.is_empty(),
        "{:?}",
        lowering.diagnostics
    );
    let module = &lowering.program.modules[0];
    assert!(
        module
            .struct_instances
            .iter()
            .any(|instance| instance.name == sym("Inner")
                && instance.args.len() == 1
                && matches!(
                    lowering.type_store.get(instance.args[0]),
                    Some(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))
                )),
        "array layout metadata type must retain the nested struct instance: {:?}",
        module.struct_instances
    );
}

#[test]
fn instantiates_generic_local_static_storage_per_function_instance() {
    let source = r#"
fn slot[T]() &mut T {
    static mut item: T;
    &mut item
}

fn main() i32 {
    let mut left = slot[i32]();
    let mut right = slot[u64]();
    _ = left;
    _ = right;
    0
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let interner = lowering.append(module.id);
    let i32_ty = interner.primitive(nia_ty::PrimitiveTy::I32);
    let u64_ty = interner.primitive(nia_ty::PrimitiveTy::U64);

    assert!(
        module
            .globals
            .iter()
            .all(|global| global.name != sym("item")),
        "generic local static must not lower as shared ordinary global"
    );
    let mut item_instances = module
        .global_instances
        .iter()
        .filter(|global| global.name == sym("item"))
        .collect::<Vec<_>>();
    item_instances.sort_by_key(|global| global.symbol.clone());

    assert_eq!(item_instances.len(), 2);
    assert!(item_instances.iter().any(|global| global.ty == i32_ty));
    assert!(item_instances.iter().any(|global| global.ty == u64_ty));
    assert!(
        item_instances
            .iter()
            .all(|global| global.args.len() == 1 && global.arg_module_id == lowering.module_id)
    );
}

#[test]
fn instantiates_interleaved_generic_local_static_types_by_kind() {
    let source = r#"
fn slot[T, N: usize, U]() &mut [U; N] {
    static mut item: [U; N];
    &mut item
}

fn main() i32 {
    let mut value = slot[i32, 3, i64]();
    _ = value;
    0
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let interner = lowering.append(module.id);
    let i32_ty = interner.primitive(nia_ty::PrimitiveTy::I32);
    let i64_ty = interner.primitive(nia_ty::PrimitiveTy::I64);
    let item = module
        .global_instances
        .iter()
        .find(|global| global.name == sym("item"))
        .unwrap_or_else(|| panic!("item global instance: {:?}", module.global_instances));

    assert_eq!(item.args, vec![i32_ty, i64_ty]);
    assert_eq!(item.const_args.len(), 1);
    assert!(matches!(
        lowering.type_store.get(item.ty),
        Some(nia_ty::TyKind::Array {
            len: nia_ty::ArrayLenTy::ConstValue(3),
            elem,
        }) if *elem == i64_ty
    ));
}

#[test]
fn nested_local_static_shadowing_keeps_distinct_definition_ids() {
    let source = r#"
fn main() i32 {
    static mut value: i32 = 1;
    if true {
        static mut value: i32 = 2;
        value
    } else {
        value
    }
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let values = module
        .globals
        .iter()
        .filter(|global| global.name == sym("value"))
        .collect::<Vec<_>>();

    assert_eq!(values.len(), 2, "nested static definitions: {values:?}");
    assert_ne!(values[0].def_id, values[1].def_id);
    assert!(values.iter().all(|global| global.init.is_some()));
}

#[test]
fn instantiates_nested_generic_function_instance_args_in_canonical_store() {
    let source = r#"
fn inner[T](value: T) T {
    value
}

fn outer[T](value: &T) &T {
    inner[&T](value)
}

fn main() i32 {
    let mut value = 1;
    let mut ptr = &value;
    _ = outer[i32](ptr);
    0
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let interner = lowering.append(module.id);
    let i32_ty = interner.primitive(nia_ty::PrimitiveTy::I32);
    let instance = module
        .function_instances
        .iter()
        .find(|instance| instance.name == sym("inner"))
        .expect("inner instance");
    let i32_ptr = instance.args[0];

    assert_eq!(instance.args, vec![i32_ptr]);
    assert_eq!(instance.params[0].passing_ty, i32_ptr);
    assert_eq!(instance.params[0].local_ty, i32_ptr);
    assert_eq!(instance.return_type, i32_ptr);
    assert!(matches!(
        lowering.type_store.get(i32_ptr),
        Some(nia_ty::TyKind::Pointer {
            is_readonly: true,
            elem,
        }) if *elem == i32_ty
    ));
}

#[test]
fn lowers_reachable_concrete_extension_methods_to_backend_functions() {
    let source = r#"
struct Args {
    len: usize,
    ptr: &&u8,
}

struct Env {
    ptr: &&u8,
}

struct Init {
    argc: usize,
    argv: &&u8,
    envp: &&u8,
}

extend Init {
    fn init(argc: usize, argv: &&u8, envp: &&u8) Init {
        Self { argc, argv, envp }
    }

    pub fn argc(&self) usize {
        self.argc
    }

    pub fn args(&self) Args {
        Args { len: self.argc, ptr: self.argv }
    }

    pub fn env(&self) Env {
        Env { ptr: self.envp }
    }

    pub fn argv(&self) &&u8 {
        self.argv
    }

    pub fn envp(&self) &&u8 {
        self.envp
    }
}

extend Args {
    fn init(len: usize, ptr: &&u8) Args {
        Self { len, ptr }
    }
}

extend Env {
    fn init(ptr: &&u8) Env {
        Self { ptr }
    }
}

fn main(argc: usize, argv: &&u8, envp: &&u8) usize {
    let mut init = Init { argc: argc, argv: argv, envp: envp };
    _ = init;
    argc
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let init_methods = module
        .functions
        .iter()
        .filter(|function| {
            matches!(function.name, name if [
                sym("init"),
                sym("argc"),
                sym("args"),
                sym("env"),
                sym("argv"),
                sym("envp"),
            ]
            .contains(&name))
        })
        .map(|function| function.name)
        .collect::<Vec<_>>();

    for name in ["argc", "args", "env", "argv", "envp"] {
        let name_symbol = sym(name);
        assert!(
            init_methods.contains(&name_symbol),
            "missing concrete extension method `{name}` from backend functions: {init_methods:?}"
        );
    }
    assert!(
        !init_methods.contains(&sym("init")),
        "unused extension constructors should not be lowered eagerly: {init_methods:?}"
    );
}
