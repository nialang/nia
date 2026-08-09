use super::*;
use nia_diagnostic::DiagnosticCategory;
use nia_ids::ModuleIdAllocator;
use nia_parser::parse_module;
use nia_symbol::stable_hash;

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

#[test]
fn collects_top_level_defs_into_separate_namespaces() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
module math;
using entry::math;
struct Point { x: i32, y: i32 }
enum Color { Red, Green }
type Byte = u8;
fn Point() i32 { 0 }
static mut counter = 0;
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let collection = collect_module_defs(module_id, &module);
    assert!(
        collection.diagnostics.is_empty(),
        "{:?}",
        collection.diagnostics
    );
    assert!(collection.module_scope.modules.get(&sym("math")).is_some());
    assert!(collection.module_scope.types.get(&sym("Point")).is_some());
    assert!(collection.module_scope.types.get(&sym("Color")).is_some());
    assert!(collection.module_scope.types.get(&sym("Byte")).is_some());
    assert!(collection.module_scope.values.get(&sym("Point")).is_some());
    assert!(
        collection
            .module_scope
            .values
            .get(&sym("counter"))
            .is_some()
    );
    assert!(collection.def_nodes.entries().count() >= collection.defs.len());
}

#[test]
fn public_surface_facts_rebase_without_revision_or_module_handles() {
    let (module, errors) = parse_module(
        r#"
pub using entry::dep::{Thing, Choice::*};
pub struct Local[T] { value: T }
pub enum Choice { First, Second }
pub fn make() () {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let mut first_ids = ModuleIdAllocator::new();
    let first_module = first_ids.allocate();
    let original = collect_module_defs(first_module, &module);
    assert!(
        original.diagnostics.is_empty(),
        "{:?}",
        original.diagnostics
    );
    let facts = PublicSurfaceModuleFacts::from_defs(&original);
    let mut second_ids = ModuleIdAllocator::new();
    let second_module = second_ids.allocate();

    let rebased = facts.materialize_for_public_surface(second_module);

    assert_ne!(first_module, second_module);
    assert_eq!(rebased.module_id, second_module);
    assert_eq!(PublicSurfaceModuleFacts::from_defs(&rebased), facts,);
    assert!(
        rebased
            .defs
            .iter()
            .all(|(_, def)| def.module_id == second_module)
    );
    assert!(rebased.def_nodes.entries().next().is_none());
}

#[test]
fn reports_duplicates_per_namespace() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
struct Thing { a: i32, a: i32 }
struct Thing {}
fn f() {}
fn f() {}
enum E { A, A }
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let collection = collect_module_defs(module_id, &module);
    assert_eq!(collection.diagnostics.len(), 4);
    assert!(collection.diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_str() == "E0101"
            && diagnostic.category == DiagnosticCategory::User
            && diagnostic
                .primary_message()
                .is_some_and(|message| message.contains("duplicate type definition"))
    }));
    assert!(collection.diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_str() == "E0101"
            && diagnostic.category == DiagnosticCategory::User
            && diagnostic
                .primary_message()
                .is_some_and(|message| message.contains("duplicate value definition"))
    }));
    assert!(collection.diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_str() == "E0101"
            && diagnostic.category == DiagnosticCategory::User
            && diagnostic
                .primary_message()
                .is_some_and(|message| message.contains("duplicate struct field"))
    }));
    assert!(collection.diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_str() == "E0101"
            && diagnostic.category == DiagnosticCategory::User
            && diagnostic
                .primary_message()
                .is_some_and(|message| message.contains("duplicate enum variant"))
    }));
}

#[test]
fn reports_duplicate_generic_parameters() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
struct Box[T, T] { value: T }
type Alias[T, T] = T;
fn id[T, T](x: T) T { x }
struct Methods[T] {
value: T,
}

extend[T, T] Methods[T] {
fn get[U, U](self) T { self.value }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let collection = collect_module_defs(module_id, &module);
    let duplicate_count = collection
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code.as_str() == "E0101"
                && diagnostic.category == DiagnosticCategory::User
                && diagnostic
                    .primary_message()
                    .is_some_and(|message| message.contains("duplicate generic parameter"))
        })
        .count();
    assert_eq!(duplicate_count, 5, "{:?}", collection.diagnostics);
}

#[test]
fn maps_top_level_bindings_by_binding_node_key() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
static global: i32 = 1;
const answer: i32 = 42;
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let collection = collect_module_defs(module_id, &module);
    assert!(
        collection.diagnostics.is_empty(),
        "{:?}",
        collection.diagnostics
    );

    for item in &module.items {
        let nia_ast::ItemKind::Binding(binding) = &item.kind else {
            continue;
        };
        let expected_kind = if binding.is_const() {
            DefKind::Const
        } else {
            DefKind::Global
        };
        let def_id = collection
            .def_nodes
            .get(&binding.node_key)
            .expect("binding node key should map to its definition");
        let def = collection
            .defs
            .get(def_id)
            .expect("binding definition should exist");
        assert_eq!(def.kind, expected_kind);
        assert_eq!(def.name, binding.name);
    }
}

#[test]
fn definition_ids_are_stable_across_unrelated_insertions() {
    let before = collect_ok(
        r#"
pub struct Point { x: i32, y: i32 }
pub enum Color { Red, Green }
trait Show {
fn show(self) i32;
}
extend Point {
fn len(self) i32 { 0 }
}
pub fn main() i32 { 0 }
"#,
    );
    let after = collect_ok(
        r#"
fn helper() i32 { 1 }
pub struct Point { x: i32, y: i32 }
pub enum Color { Red, Green }
trait Show {
fn show(self) i32;
}
extend Point {
fn len(self) i32 { 0 }
}
pub fn main() i32 { 0 }
"#,
    );

    assert_eq!(top_type_id(&before, "Point"), top_type_id(&after, "Point"));
    assert_eq!(top_value_id(&before, "main"), top_value_id(&after, "main"));
    assert_eq!(
        member_id(&before, top_type_id(&before, "Point"), "x"),
        member_id(&after, top_type_id(&after, "Point"), "x")
    );
    assert_eq!(
        enum_variant_id(&before, top_type_id(&before, "Color"), "Green"),
        enum_variant_id(&after, top_type_id(&after, "Color"), "Green")
    );
    assert_eq!(
        member_id(&before, top_type_id(&before, "Show"), "show"),
        member_id(&after, top_type_id(&after, "Show"), "show")
    );
    assert_eq!(
        extension_method_id(&before, "len"),
        extension_method_id(&after, "len")
    );
    assert_ne!(before.module_id, after.module_id);
}

#[test]
fn extension_definition_ids_ignore_type_formatting() {
    let before = collect_ok(
        r#"
struct Box[T] { value: T }
extend[T] &Box[T] {
fn get(self) T { self.value }
}
"#,
    );
    let after = collect_ok(
        r#"
struct Box[T] { value: T }
extend[T] & Box[ T ] {
fn get(self) T { self.value }
}
"#,
    );

    assert_eq!(
        extension_method_id(&before, "get"),
        extension_method_id(&after, "get")
    );
}

fn collect_ok(source: &str) -> DefCollection {
    let (module, errors) = parse_module(source);
    assert!(errors.is_empty(), "{errors:?}");
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let collection = collect_module_defs(module_id, &module);
    assert!(
        collection.diagnostics.is_empty(),
        "{:?}",
        collection.diagnostics
    );
    collection
}

fn top_type_id(defs: &DefCollection, name: &str) -> DefId {
    defs.module_scope
        .types
        .get(&sym(name))
        .unwrap_or_else(|| panic!("missing top-level type `{name}`"))
}

fn top_value_id(defs: &DefCollection, name: &str) -> DefId {
    defs.module_scope
        .values
        .get(&sym(name))
        .unwrap_or_else(|| panic!("missing top-level value `{name}`"))
}

fn member_id(defs: &DefCollection, owner: DefId, name: &str) -> DefId {
    let symbol = sym(name);
    defs.scopes
        .struct_members
        .get(&owner)
        .and_then(|members| {
            members
                .fields
                .get(&symbol)
                .or_else(|| members.methods.get(&symbol))
                .or_else(|| members.values.get(&symbol))
        })
        .unwrap_or_else(|| panic!("missing member `{name}`"))
}

fn enum_variant_id(defs: &DefCollection, owner: DefId, name: &str) -> DefId {
    let symbol = sym(name);
    defs.scopes
        .enum_members
        .get(&owner)
        .and_then(|members| members.variants.get(&symbol))
        .unwrap_or_else(|| panic!("missing enum variant `{name}`"))
}

fn extension_method_id(defs: &DefCollection, name: &str) -> DefId {
    let symbol = sym(name);
    defs.defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == DefKind::Method && def.parent.is_none() && def.name == symbol)
                .then_some(def_id)
        })
        .unwrap_or_else(|| panic!("missing extension method `{name}`"))
}
