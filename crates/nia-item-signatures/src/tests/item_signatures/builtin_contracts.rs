use super::*;

#[test]
fn records_builtin_function_attributes() {
    let signatures = signatures_ok(
        r#"
@[builtin("trap")]
pub fn trap() never;
"#,
    );

    assert_eq!(signatures.functions.len(), 1);
    let signature = signatures
        .functions
        .values()
        .next()
        .expect("trap signature");
    assert_eq!(
        signature.attributes,
        vec![FunctionAttribute::Builtin(BuiltinFunction::Trap)]
    );
}

#[test]
fn records_builtin_trait_attributes() {
    let signatures = signatures_ok(
        r#"
@[builtin("Iterator")]
pub trait Iterator {
type Item;
}
"#,
    );

    assert_eq!(signatures.traits.len(), 1);
    let signature = signatures
        .traits
        .values()
        .next()
        .expect("iterator signature");
    assert_eq!(signature.builtin, Some(BuiltinTrait::Iterator));
}

#[test]
fn records_trait_associated_const_requirements() {
    let signatures = signatures_ok(
        r#"
trait Simd {
type Lane;
const Lanes: usize;
}
"#,
    );

    let signature = signatures.traits.values().next().expect("simd signature");
    assert_eq!(signature.associated_types.len(), 1);
    assert_eq!(signature.associated_values.len(), 1);
    assert_eq!(signature.associated_values[0].name, sym("Lanes"));
}

#[test]
fn records_builtin_extend_attributes_with_bodyless_methods() {
    let signatures = signatures_ok(
        r#"
trait Probe {
fn probe(&self) usize;
}

@[builtin("test.Probe")]
extend[T] [T] : Probe {
fn probe(&self) usize;
}
"#,
    );

    assert_eq!(signatures.trait_impls.len(), 1);
    let impl_signature = &signatures.trait_impls[0];
    assert_eq!(impl_signature.builtin.as_deref(), Some("test.Probe"));
    assert_eq!(impl_signature.methods.len(), 1);
    let method = &signatures.functions[&impl_signature.methods[0].def_id];
    assert!(!method.has_body);
}

#[test]
fn bodyless_non_extern_functions_require_builtin_attribute() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module("fn missing_body() void;");
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    let type_store = TypeStore::new();
    let lowering = lower_module_types_with_context(
        module_id,
        &module,
        &resolved,
        TypeLoweringContext::empty(&type_store),
    );
    let signatures = collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(&module),
        defs: &defs,
        lowered: &lowering,
        type_store: &type_store,
        symbols: None,
    });

    assert!(signatures.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("bodyless non-extern functions require `@[builtin]`")
    }));
}
