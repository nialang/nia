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

fn make(ptr: &u8, cb: &fn(i32) void) [4]Box[i32] {
let mut tmp: [_]i32 = [1, 2, 3];
[{ value: 0 }; 4]
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
data: [N]T,
}

fn use_buffer(buf: Buffer[u8, 4]) void {}
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
fn lowers_trait_associated_type_shorthand_to_projection() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
trait Writer {
type Error;

fn write(& self) Error!void;
}

enum BufferError: i32 {
Bad = 1,
_,
}

struct Sink {}

extend Sink : Writer {
type Error = BufferError;

fn write(& self) Error!void {
    _ = self;
    !{}
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
