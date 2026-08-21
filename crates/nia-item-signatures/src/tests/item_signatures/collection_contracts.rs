use super::*;

#[test]
fn collects_item_signatures_without_checking_bodies() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
extern fn printf(fmt: &u8, ...);
extern static errno: i32;

struct Point {
x: i32,
y: i32,
}

extend Point {
pub const Origin: i32 = 0;
fn len2(&self) i32 { missing + self.x }
}

enum Color: u8 {
Red,
Green,
}

type Byte = u8;
static mut counter: i32 = 0;

fn add(a: i32, b: i32) i32 {
a + b
}
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
    let type_store = TypeStore::new();
    let lowered = lower_module_types_with_context(
        module_id,
        &module,
        &resolved,
        TypeLoweringContext::empty(&type_store),
    );
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let signatures = collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(&module),
        defs: &defs,
        lowered: &lowered,
        type_store: &type_store,
        symbols: None,
    });
    assert!(
        signatures.diagnostics.is_empty(),
        "{:?}",
        signatures.diagnostics
    );
    assert_eq!(signatures.structs.len(), 1);
    assert_eq!(signatures.enums.len(), 1);
    assert_eq!(signatures.type_aliases.len(), 1);
    assert_eq!(signatures.globals.len(), 2);
    assert_eq!(signatures.functions.len(), 3);
    assert!(
        signatures
            .functions
            .values()
            .any(|signature| signature.is_variadic)
    );
    assert_eq!(signatures.trait_impls.len(), 1);
    let impl_signature = &signatures.trait_impls[0];
    assert_eq!(impl_signature.methods.len(), 1);
    assert_eq!(impl_signature.methods[0].name, sym("len2"));
    assert_eq!(impl_signature.methods[0].visibility, Visibility::Private);
    assert!(
        signatures
            .functions
            .contains_key(&impl_signature.methods[0].def_id)
    );
    assert_eq!(impl_signature.associated_values.len(), 1);
    assert_eq!(impl_signature.associated_values[0].name, sym("Origin"));
    assert_eq!(
        impl_signature.associated_values[0].visibility,
        Visibility::Public
    );
    assert!(
        signatures
            .consts
            .contains_key(&impl_signature.associated_values[0].def_id)
    );
}

#[test]
fn supertrait_associated_bindings_are_signature_type_roots() {
    let signatures = signatures_ok(
        r#"
trait Parent {
    type Item;
}

trait Child : Parent[Item = (i32, bool)] {}
"#,
    );

    let child = signatures
        .traits
        .values()
        .find(|signature| !signature.supertraits.is_empty())
        .expect("Child trait signature");
    let supertrait = child.supertraits.first().expect("Parent supertrait");
    let binding = supertrait
        .associated_type_bindings
        .first()
        .expect("Parent::Item binding");
    assert_eq!(binding.name, sym("Item"));
    assert!(signatures.type_roots().contains(&binding.ty));
}

#[test]
fn collects_item_signatures_from_active_item_tree_only() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
@[if false]
fn skipped() i32 { 0 }
@[if true]
fn selected() i32 { 1 }
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let tree = ModuleItemTree::from_module(&module);
    let active = tree.active_items(&mut BoolResolver(false)).unwrap();
    let active_module = active.to_module();
    let defs = collect_module_defs_from_active_item_tree(module_id, &active);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let resolved = resolve_module_types(&active_module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let type_store = TypeStore::new();
    let lowered = lower_module_types_with_context(
        module_id,
        &active_module,
        &resolved,
        TypeLoweringContext::empty(&type_store),
    );
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);

    let signatures = collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::ActiveItemTree(&active),
        defs: &defs,
        lowered: &lowered,
        type_store: &type_store,
        symbols: None,
    });
    assert!(
        signatures.diagnostics.is_empty(),
        "{:?}",
        signatures.diagnostics
    );
    assert_eq!(signatures.functions.len(), 1);
    assert_eq!(active.items.len(), 1);
    assert!(matches!(
        &active_module.items[0].kind,
        nia_ast::ItemKind::Function(function) if function.name == sym("selected")
    ));
}

#[test]
fn trait_impl_ids_ignore_type_formatting() {
    let before = signatures_ok(
        r#"
struct Box[T] { value: T }
extend[T] &Box[T] {
fn get(self) T { self.value }
}
"#,
    );
    let after = signatures_ok(
        r#"
struct Box[T] { value: T }
extend[T] & Box[ T ] {
fn get(self) T { self.value }
}
"#,
    );

    assert_eq!(before.trait_impls.len(), 1);
    assert_eq!(after.trait_impls.len(), 1);
    assert_eq!(before.trait_impls[0].impl_id, after.trait_impls[0].impl_id);
}
