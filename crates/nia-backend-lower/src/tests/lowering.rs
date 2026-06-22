// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn lowers_checked_program_shape() {
    let source = r#"
let hello = b"hello\0";

struct Point {
    x: i32,
    y: i32,
}

extend Point {
    fn make(x: i32, y: i32) Point {
        { x: x, y: y }
    }
}

fn main() i32 {
    var p = Point::make(1, 2);
    p.x
}
"#;
    let (module, errors) = parse_module(source);
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_resolved = resolve_module_types(&module, &defs);
    let type_lowering = lower_module_types_with_id(ModuleId(0), &module, &type_resolved);
    let signatures = collect_item_signatures(&module, &defs, &type_lowering);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let semantic_uses = semantic_use_table(ModuleId(0), &values, &locals, &type_lowering);
    let normalization = normalize_module_types(ModuleId(0), &type_lowering.interner, &signatures);
    let target = nia_target_config::TargetConfig::host();
    let source_path = nia_source::SourcePath::new("/tmp/nia-backend-lower-test/lowering.nia");
    let active_item_tree = active_item_tree(&module);
    let comptime_module =
        nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            const_exprs: &type_lowering.const_exprs,
            source_path: &source_path,
        });
    assert!(
        comptime_module.diagnostics.is_empty(),
        "{:?}",
        comptime_module.diagnostics
    );
    let comptime = nia_comptime_check::check_module_comptime(nia_comptime_check::ComptimeInput {
        module: &comptime_module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        signatures: &signatures,
        interner: &normalization.interner,
        normalized: &normalization.normalized,
        target: &target,
        source_path: &source_path,
        program: nia_comptime_check::ComptimeProgramContext::empty(),
    });
    let layouts = nia_layout::compute_layouts_with_normalized_types(
        &defs,
        &normalization.interner,
        &signatures,
        &normalization.normalized,
        &|id| comptime.array_lengths.get(&id).copied(),
        nia_layout::TargetDataLayout::LP64,
    );
    let _abi = check_module_abi(&defs, &type_lowering.interner, &signatures);
    let _flow = check_module_flow(&module, &type_lowering.interner, &signatures);
    let point_id = defs.module_scope.types.get("Point").expect("Point def");
    let make_id = defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == DefKind::Method && def.name == "make").then_some(def_id)
        })
        .expect("make def");
    let mut extensions = VisibleExtensionMethods::default();
    let point_ty = normalization
        .interner
        .iter()
        .find_map(|(ty_id, ty)| {
            matches!(
                ty,
                nia_ty::TyKind::Nominal {
                    def_id,
                    args
                } if def_id.module_id == ModuleId(0) && def_id.def_id == point_id && args.is_empty()
            )
            .then_some(ty_id)
        })
        .expect("Point type");
    extensions.insert(
        0,
        point_ty,
        VisibleExtensionMethod {
            name: "make".to_string(),
            def_id: GlobalDefId {
                module_id: ModuleId(0),
                def_id: make_id,
            },
            impl_index: 0,
            impl_generics: Vec::new(),
            trait_id: None,
            trait_args: Vec::new(),
            where_predicates: Vec::new(),
            is_callable: true,
            is_trait_witness: false,
        },
    );
    let origins = NodeOriginTable::default();
    let body_check = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: None,
        source_path: &source_path,
        origins: &origins,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        lowered: &type_lowering,
        signatures: &signatures,
        normalization: &normalization,
        target: &target,
        comptime: &comptime,
        comptime_module: &comptime_module.module,
        layouts: &layouts,
        extensions: &extensions,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        extension_interner: None,
        program: nia_body_check::BodyProgramContext::empty(),
        program_signatures: ProgramSignatureMaps {
            functions: &HashMap::new(),
            globals: &HashMap::new(),
            comptimes: &HashMap::new(),
            structs: &HashMap::new(),
            unions: &HashMap::new(),
            enums: &HashMap::new(),
            traits: &HashMap::new(),
            type_aliases: &HashMap::new(),
            trait_impls: &[],
        },
        program_comptime: nia_body_check::ProgramComptimeMaps {
            comptimes: &HashMap::new(),
            modules: &HashMap::new(),
        },
        filter: nia_body_check::BodyCheckFilter::All,
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
                lower_function_body(body).expect("valid typed body"),
            )
        })
        .collect::<HashMap<_, _>>();
    let program_comptime = HashMap::from([(ModuleId(0), &comptime)]);

    let input = BackendLowerModuleInput {
        module_id: ModuleId(0),
        module_name: "main".to_string(),
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        type_lowering: &type_lowering,
        signatures: &signatures,
        type_normalization: &normalization,
        body_ir: &body_check.ir,
        function_interner: &body_check.ir.interner,
        semantic_facts: &body_check.facts,
        extensions: &extensions,
        comptime: &comptime,
        program_comptime: &program_comptime,
        layouts: &layouts,
        function_bodies: &function_bodies,
        roots: BackendFunctionRoots::Public,
        program_function_bodies: &function_bodies,
        extension_interner: None,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        program_extensions: &HashMap::new(),
        program_defs: &HashMap::new(),
        program_type_interners: &HashMap::new(),
        program_type_normalizations: &HashMap::new(),
        program_functions: &HashMap::new(),
        program_structs: &HashMap::new(),
        program_unions: &HashMap::new(),
        program_enums: &HashMap::new(),
        program_traits: &HashMap::new(),
        program_type_aliases: &HashMap::new(),
        trait_impls: &[],
    };
    let lowering = lower_backend_program(
        &[input],
        &Monomorphization {
            instances: Vec::new(),
            type_interners: HashMap::new(),
            diagnostics: Vec::new(),
        },
        nia_opt::OptimizationPolicy::default(),
    );
    assert!(
        lowering.diagnostics.is_empty(),
        "{:?}",
        lowering.diagnostics
    );
    assert_eq!(lowering.program.modules.len(), 1);
    assert_eq!(lowering.program.modules[0].globals.len(), 1);
    assert_eq!(lowering.program.modules[0].functions.len(), 2);
}

#[test]
fn comptime_bindings_do_not_lower_to_storage() {
    let source = r#"
comptime let answer: i32 = 40 + 2;

fn main() i32 {
    comptime let local: i32 = answer;
    local
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    assert!(module.globals.is_empty());
    let main = module
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    assert!(
        main.function_body
            .as_ref()
            .expect("main function body")
            .locals
            .iter()
            .all(|local| local.name != "local")
    );
}

#[test]
fn lowers_large_array_repeat_count_from_comptime_binding() {
    let source = r#"
comptime let N: usize = 1048576;

fn main() i32 {
    var buffer: [N]u8 = [0u8; N];
    0
}
"#;
    let lowering = lower_source(source);
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
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
    assert_eq!(*count, 1048576);
}

#[test]
fn lowers_function_body_to_function_ir() {
    let source = r#"
fn main() i32 {
    defer {
    };
    var value = 1;
    return value;
}
"#;
    let lowering = lower_source(source);
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
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
        .find(|function| function.name == "main")
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
    let instance = module
        .function_instances
        .iter()
        .find(|instance| instance.name == "id")
        .expect("id instance");
    let body = instance
        .function_body
        .as_ref()
        .expect("id instance function body");
    let i32_ty = module.interner.primitive(nia_ty::PrimitiveTy::I32);

    assert_eq!(instance.params[0].passing_ty, i32_ty);
    assert_eq!(instance.params[0].local_ty, i32_ty);
    assert_eq!(instance.return_type, i32_ty);
    assert_eq!(body.ty, i32_ty);
    assert!(body.locals.iter().all(|local| local.ty == i32_ty));
}

#[test]
fn instantiates_nested_generic_function_instance_args_in_visible_interner() {
    let source = r#"
fn inner[T](value: T) T {
    value
}

fn outer[T](value: &T) &T {
    inner[&T](value)
}

fn main() i32 {
    var value = 1;
    var ptr = &value;
    _ = outer[i32](ptr);
    0
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let i32_ty = module.interner.primitive(nia_ty::PrimitiveTy::I32);
    let i32_ptr = module
        .interner
        .iter()
        .find_map(|(ty_id, ty)| {
            matches!(
                ty,
                nia_ty::TyKind::Pointer {
                    is_readonly: true,
                    elem,
                } if *elem == i32_ty
            )
            .then_some(ty_id)
        })
        .expect("&i32 type");
    let instance = module
        .function_instances
        .iter()
        .find(|instance| instance.name == "inner")
        .expect("inner instance");

    assert_eq!(instance.args, vec![i32_ptr]);
    assert_eq!(instance.params[0].passing_ty, i32_ptr);
    assert_eq!(instance.params[0].local_ty, i32_ptr);
    assert_eq!(instance.return_type, i32_ptr);
    assert!(module.interner.get(instance.args[0]).is_some());
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
        { argc: argc, argv: argv, envp: envp }
    }

    pub fn argc(&self) usize {
        self.argc
    }

    pub fn args(&self) Args {
        { len: self.argc, ptr: self.argv }
    }

    pub fn env(&self) Env {
        { ptr: self.envp }
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
        { len: len, ptr: ptr }
    }
}

extend Env {
    fn init(ptr: &&u8) Env {
        { ptr: ptr }
    }
}

fn main(argc: usize, argv: &&u8, envp: &&u8) usize {
    var init: Init = { argc: argc, argv: argv, envp: envp };
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
            matches!(
                function.name.as_str(),
                "init" | "argc" | "args" | "env" | "argv" | "envp"
            )
        })
        .map(|function| function.name.as_str())
        .collect::<Vec<_>>();

    for name in ["argc", "args", "env", "argv", "envp"] {
        assert!(
            init_methods.contains(&name),
            "missing concrete extension method `{name}` from backend functions: {init_methods:?}"
        );
    }
    assert!(
        !init_methods.contains(&"init"),
        "unused extension constructors should not be lowered eagerly: {init_methods:?}"
    );
}
