use super::*;
use nia_defs::{collect_module_defs, collect_module_defs_from_active_item_tree};
use nia_ids::ModuleIdAllocator;
use nia_item_tree::ModuleItemTree;
use nia_parser::parse_module_with_symbols;
use nia_symbol_table::SymbolTable;

fn resolve_source(source: &str) -> TypeResolution {
    let symbols = SymbolTable::new();
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module_with_symbols(source, symbols.clone());
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    resolve_module_types_with_symbols(&module, &defs, &symbols)
}

#[test]
fn resolves_primitive_nominal_and_generic_types() {
    let resolved = resolve_source(
        r#"
struct Box[T] {
value: T,
}

type Byte = u8;

fn make(value: i32) Box[i32] {
let mut tmp: Byte = 1;
{ value: value }
}
"#,
    );
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    assert!(
        resolved
            .node_type_names
            .values()
            .any(|resolution| matches!(resolution, TypeNameResolution::GenericParam))
    );
    assert!(
        resolved
            .node_type_names
            .values()
            .any(|resolution| matches!(resolution, TypeNameResolution::Primitive(_)))
    );
    assert!(
        resolved
            .node_type_names
            .values()
            .any(|resolution| matches!(resolution, TypeNameResolution::Def(_)))
    );
}

#[test]
fn resolves_trait_associated_type_shorthand_in_trait_scope() {
    let resolved = resolve_source(
        r#"
trait Writer {
type Error;

fn write(& self) Error!void;
}
"#,
    );
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    assert!(
        resolved
            .node_type_names
            .values()
            .any(|resolution| { matches!(resolution, TypeNameResolution::AssociatedType) })
    );
}

#[test]
fn resolves_trait_impl_associated_type_shorthand_before_builtin_error() {
    let resolved = resolve_source(
        r#"
trait Reader {
type Error;

fn end_of_stream(&self) Error;
}

struct Buffer {}

extend Buffer : Reader {
type Error = i32;

fn end_of_stream(&self) Error {
    1
}
}
"#,
    );
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let associated_type_count = resolved
        .node_type_names
        .values()
        .filter(|resolution| matches!(resolution, TypeNameResolution::AssociatedType))
        .count();
    assert_eq!(associated_type_count, 2);
}

#[test]
fn local_types_shadow_builtin_trait_fallback_names() {
    let resolved = resolve_source(
        r#"
type Ptr[T] = &T;

fn id(value: Ptr[u8]) Ptr[u8] {
value
}
"#,
    );
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    assert!(
        resolved
            .node_type_names
            .values()
            .any(|resolution| matches!(resolution, TypeNameResolution::Def(_)))
    );
    assert!(
        !resolved.node_type_names.values().any(|resolution| matches!(
            resolution,
            TypeNameResolution::BuiltinTrait(BuiltinTrait::Ptr)
        ))
    );
}

#[test]
fn reports_unknown_types_without_resolving_values() {
    let resolved = resolve_source(
        r#"
fn main() Missing {
let mut value = MissingValue;
0
}
"#,
    );
    assert_eq!(resolved.diagnostics.len(), 1);
    assert!(
        resolved.diagnostics[0]
            .summary
            .contains("unknown type `Missing`")
    );
}

#[test]
fn reports_qualified_namespace_errors_on_type_path_span() {
    let resolved = resolve_source(
        r#"
fn main() Missing::Type {
0
}
"#,
    );
    assert_eq!(resolved.diagnostics.len(), 1);
    assert!(
        resolved.diagnostics[0]
            .summary
            .contains("unknown namespace `Missing`")
    );
    assert_ne!(
        resolved.diagnostics[0].primary_span(),
        Some(Span::default())
    );
}

#[test]
fn resolves_types_from_active_item_tree_only() {
    let symbols = SymbolTable::new();
    let (module, errors) = parse_module_with_symbols(
        r#"
@[if false]
fn skipped(value: MissingType) void {}
@[if true]
fn selected(value: i32) void {}
"#,
        symbols.clone(),
    );
    assert!(errors.is_empty(), "{errors:?}");
    let tree = ModuleItemTree::from_module(&module);
    let active = tree.active_items(&mut BoolResolver(false)).unwrap();
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let defs = collect_module_defs_from_active_item_tree(module_id, &active);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let resolved = resolve_module_types_from_active_item_tree_with_symbols(
        &active,
        &defs,
        ProgramDefsContext::empty(),
        &nia_defs::PublicSurfaces::default(),
        &nia_defs::ModuleUsingScope::default(),
        &symbols,
    );
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    assert!(
        resolved
            .node_type_names
            .values()
            .any(|resolution| matches!(resolution, TypeNameResolution::Primitive(_)))
    );
}

struct BoolResolver(bool);

impl nia_item_tree::ConditionResolver for BoolResolver {
    fn resolve_condition(
        &mut self,
        cond: &nia_ast::ConditionExpr,
    ) -> Result<bool, nia_item_tree::ItemTreeError> {
        match &cond.kind {
            nia_ast::ConditionExprKind::Bool(value) => Ok(*value),
            _ => Ok(self.0),
        }
    }
}
