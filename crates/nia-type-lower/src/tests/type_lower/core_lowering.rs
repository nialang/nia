use super::*;

#[test]
fn lowers_primitive_pointer_array_function_and_nominal_types() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
struct Box[T] {
value: T,
}

fn make(ptr: &u8, cb: &fn(i32) ()) [i32] {
let mut tmp: [i32; _] = [1, 2, 3];
[Box[i32] { value: 0 }; 4]
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let (type_store, lowered) = lower_test_module(&module, &defs, &resolved);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let append = type_store.append_for_module(module_id);
    assert!(matches!(
        type_store.get(append.intern(TyKind::Error)),
        Some(TyKind::Error)
    ));
    assert!(matches!(
        type_store.get(append.intern(TyKind::Primitive(PrimitiveTy::I8))),
        Some(TyKind::Primitive(PrimitiveTy::I8))
    ));
    assert!(
        lowered
            .type_uses
            .values()
            .any(|ty_id| matches!(type_store.get(*ty_id), Some(TyKind::Nominal { .. })))
    );
    assert!(
        lowered
            .type_uses
            .values()
            .any(|ty_id| matches!(type_store.get(*ty_id), Some(TyKind::Array { .. })))
    );
    assert!(
        lowered
            .type_uses
            .values()
            .any(|ty_id| matches!(type_store.get(*ty_id), Some(TyKind::Pointer { .. })))
    );
}

#[test]
fn lowers_const_generic_array_lengths_and_nominal_args() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
struct Buffer[T, N: usize] {
data: [T; N],
}

fn use_buffer(buf: Buffer[u8, 4]) () {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let resolved = resolve_module_types(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let (type_store, lowered) = lower_test_module(&module, &defs, &resolved);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    assert!(
        lowered
            .type_uses
            .values()
            .filter_map(|ty| type_store.get(*ty))
            .any(|ty| {
                matches!(
                    ty,
                    TyKind::Array {
                        len: ArrayLenTy::GenericParam(name),
                        ..
                    } if *name == sym("N")
                )
            })
    );
    assert!(
        lowered
            .type_uses
            .values()
            .filter_map(|ty| type_store.get(*ty))
            .any(|ty| {
                matches!(
                    ty,
                    TyKind::Nominal { const_args, .. }
                        if matches!(
                            const_args.as_slice(),
                            [ConstGenericArg {
                                value: ConstGenericValue::Int(value),
                                ..
                            }] if value.bits() == 4
                        )
                )
            })
    );
}

#[test]
fn lowers_external_const_generic_parameter_types_in_their_defining_module() {
    let mut module_ids = ModuleIdAllocator::new();
    let defining_module_id = module_ids.allocate();
    let consuming_module_id = module_ids.allocate();
    let (defining_module, defining_errors) = parse_module(
        r#"
pub struct Packet[T, N: usize, U] {
    marker: T,
    values: [U; N],
}
"#,
    );
    assert!(defining_errors.is_empty(), "{defining_errors:?}");
    let defining_defs = collect_module_defs(defining_module_id, &defining_module);
    let packet_id = defining_defs
        .module_scope
        .types
        .get(&sym("Packet"))
        .expect("Packet definition");

    let (consuming_module, consuming_errors) = parse_module(
        r#"
fn consume(packet: Packet[u8, 2, u16]) () {}
"#,
    );
    assert!(consuming_errors.is_empty(), "{consuming_errors:?}");
    let consuming_defs = collect_module_defs(consuming_module_id, &consuming_module);
    let mut resolved = resolve_module_types(&consuming_module, &consuming_defs);
    let ItemKind::Function(function) = &consuming_module.items[0].kind else {
        panic!("expected function");
    };
    let packet_ty = function.params[0]
        .ty
        .as_ref()
        .expect("packet parameter type");
    resolved.node_type_names.insert(
        packet_ty.node_key.site().clone(),
        TypeNameResolution::External(GlobalDefId {
            module_id: defining_module_id,
            def_id: packet_id,
        }),
    );

    let program_defs = HashMap::from([
        (defining_module_id, Arc::new(defining_defs)),
        (consuming_module_id, Arc::new(consuming_defs.clone())),
    ]);
    let program_defs_by_module = |module_id| program_defs.get(&module_id).cloned();
    let type_store = nia_ty::TypeStore::new();
    let lowered = lower_module_types_with_context(
        consuming_module_id,
        &consuming_module,
        &resolved,
        TypeLoweringContext::from_program_defs(
            &type_store,
            ProgramDefsContext {
                defs: Some(&program_defs_by_module),
            },
        ),
    );
    let lowered_packet = lowered
        .ty_for_key(&packet_ty.node_key)
        .expect("lowered Packet type");
    let Some(TyKind::Nominal { const_args, .. }) = type_store.get(lowered_packet) else {
        panic!("expected nominal Packet type");
    };
    let [const_arg] = const_args.as_slice() else {
        panic!("expected one const argument");
    };
    assert_eq!(
        type_store.get(const_arg.ty),
        Some(&TyKind::Primitive(PrimitiveTy::Usize))
    );
}

#[test]
fn lowers_trait_associated_type_shorthand_to_projection() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
trait Writer {
type Error;

fn write(& self) Error!();
}

enum BufferError: i32 {
Bad = 1,
_,
}

struct Sink {}

extend Sink : Writer {
type Error = BufferError;

fn write(& self) Error!() {
    _ = self;
    !()
}
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let (type_store, lowered) = lower_test_module(&module, &defs, &resolved);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let shorthand_projections = lowered
        .type_uses
        .values()
        .filter(|ty_id| {
            matches!(
                type_store.get(**ty_id),
                Some(TyKind::Projection { name, .. }) if *name == sym("Error")
            )
        })
        .count();
    assert!(shorthand_projections >= 2, "{:?}", lowered.type_uses);
}

#[test]
fn lowers_slice_extend_target_to_slice_pointee() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
extend[T] [T] {
fn len2(& self) usize {
    self.len()
}
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let (type_store, lowered) = lower_test_module(&module, &defs, &resolved);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let nia_ast::ItemKind::Extend(extend) = &module.items[0].kind else {
        panic!("expected extend");
    };
    let target_ty = lowered
        .ty_for_key(&extend.target.node_key)
        .expect("expected lowered extend target");
    assert!(matches!(
        type_store.get(target_ty),
        Some(TyKind::SlicePointee { .. })
    ));
}

#[test]
fn lowers_callable_interfaces_and_views_with_distinct_identity() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
type Callback = Fn(i32, bool) i32;
type Reordered = Fn(bool, i32) i32;
type CallbackRef = &Fn(i32, bool) i32;
type CallbackMut = &mut Fn(i32, bool) i32;
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let (type_store, lowered) = lower_test_module(&module, &defs, &resolved);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);

    let callable_pointees = lowered
        .type_uses
        .values()
        .copied()
        .filter(|ty| matches!(type_store.get(*ty), Some(TyKind::CallablePointee { .. })))
        .collect::<Vec<_>>();
    assert_eq!(callable_pointees.len(), 2);
    assert_ne!(callable_pointees[0], callable_pointees[1]);

    let callable_views = lowered
        .type_uses
        .values()
        .copied()
        .filter(|ty| matches!(type_store.get(*ty), Some(TyKind::Callable { .. })))
        .collect::<Vec<_>>();
    assert_eq!(callable_views.len(), 2);
    assert_ne!(callable_views[0], callable_views[1]);
    assert!(callable_views.iter().any(|ty| matches!(
        type_store.get(*ty),
        Some(TyKind::Callable {
            is_readonly: true,
            ..
        })
    )));
    assert!(callable_views.iter().any(|ty| matches!(
        type_store.get(*ty),
        Some(TyKind::Callable {
            is_readonly: false,
            ..
        })
    )));
}

#[test]
fn rejects_bare_callable_interfaces_in_value_positions() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module("fn invoke(callback: Fn(i32) i32) {}");
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    let (type_store, lowered) = lower_test_module(&module, &defs, &resolved);
    assert!(lowered.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("callable interface types are unsized")
    }));
    assert!(
        lowered
            .type_uses
            .values()
            .any(|ty| matches!(type_store.get(*ty), Some(TyKind::CallablePointee { .. })))
    );
}
