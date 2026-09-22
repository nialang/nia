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
fn records_track_caller_on_functions_and_methods() {
    let signatures = signatures_ok(
        r#"
@[trackCaller]
fn top() () {}

trait Report {
    @[trackCaller]
    fn report(&self) ();
}

struct Value {}

extend Value {
    @[trackCaller]
    fn report(&self) () {}
}
"#,
    );

    let tracked = signatures
        .functions
        .values()
        .filter(|signature| signature.attributes == [FunctionAttribute::TrackCaller])
        .count();
    assert_eq!(tracked, 3);
}

#[test]
fn rejects_invalid_track_caller_attributes() {
    let signatures = signatures(
        r#"
@[trackCaller]
extern fn foreign() ();

@[trackCaller(1)]
fn withArgument() () {}

@[trackCaller]
@[trackCaller]
fn duplicate() () {}
"#,
    );

    let summaries = signatures
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert!(summaries.contains(&"`@[trackCaller]` is not valid on `extern fn`"));
    assert!(summaries.contains(&"`@[trackCaller]` does not take arguments"));
    assert!(summaries.contains(&"duplicate `@[trackCaller]` function attribute"));
    let extern_error = signatures
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.summary == "`@[trackCaller]` is not valid on `extern fn`")
        .expect("trackCaller extern diagnostic");
    assert!(
        extern_error
            .primary_message()
            .is_some_and(|message| message.contains("declared `extern`")),
        "{extern_error:?}"
    );
    assert!(
        extern_error
            .help
            .iter()
            .any(|help| help.contains("remove `extern`")),
        "{extern_error:?}"
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
fn projects_only_unambiguous_builtin_trait_identity() {
    let (module, diagnostics) = nia_parser::parse_module(
        r#"
@[builtin("IntoError")]
trait Conversion[Target] {}
"#,
    );
    assert!(diagnostics.is_empty());
    let item_tree = nia_item_tree::ModuleItemTree::from_module(&module);
    assert_eq!(
        crate::declared_builtin_trait(&item_tree.items[0].attributes),
        Some(BuiltinTrait::IntoError)
    );

    let (module, diagnostics) = nia_parser::parse_module(
        r#"
@[builtin("IntoError")]
@[builtin("Iterator")]
trait Ambiguous {}
"#,
    );
    assert!(diagnostics.is_empty());
    let item_tree = nia_item_tree::ModuleItemTree::from_module(&module);
    assert_eq!(
        crate::declared_builtin_trait(&item_tree.items[0].attributes),
        None
    );
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
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, errors) = parse_module("fn missing_body() ();");
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let resolved = resolve_module_types(&module, &defs);
    let type_store = TypeStore::new().expect("create type store");
    let lowering = lower_module_types_with_context(
        module_id,
        &module,
        &resolved,
        TypeLoweringContext::empty(&type_store),
    )
    .expect("lower module types");
    let signatures = collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(&module),
        defs: &defs,
        lowered: &lowering,
        type_store: &type_store,
        symbols: None,
    })
    .expect("collect item signatures");

    assert!(signatures.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("bodyless non-extern functions require `@[builtin]`")
    }));
    let bodyless = signatures
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .summary
                .contains("bodyless non-extern functions require `@[builtin]`")
        })
        .expect("bodyless function diagnostic");
    assert!(
        bodyless
            .primary_message()
            .is_some_and(|message| message.contains("has no body")),
        "{bodyless:?}"
    );
    assert!(
        bodyless
            .help
            .iter()
            .any(|help| help.contains("add a function body")),
        "{bodyless:?}"
    );
}
