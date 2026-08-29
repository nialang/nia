use super::*;
use nia_defs::{collect_module_defs, collect_module_defs_from_active_item_tree};
use nia_ids::{DefId, GlobalDefId, ModuleIdAllocator};
use nia_item_tree::ModuleItemTree;
use nia_parser::parse_module;
use nia_sema_ir::BuiltinAssociatedValue;
use nia_symbol::{SymbolId, stable_hash};

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

#[test]
fn resolves_module_value_names_and_defers_locals() {
    let (module, errors) = parse_module(
        r#"
static mut counter = 0;

fn add(a: i32, b: i32) i32 {
a + b + counter
}

fn main() i32 {
let mut local = add(counter, 1);
local
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let defs = collect_module_defs(module_id, &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let resolved = resolve_module_values(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    assert!(
        resolved
            .node_names
            .values()
            .any(|resolution| matches!(resolution, ValueNameResolution::Def(_)))
    );
    assert!(
        resolved
            .node_names
            .values()
            .any(|resolution| matches!(resolution, ValueNameResolution::LocalDeferred))
    );
}

#[test]
fn treats_std_builtin_paths_as_regular_qualified_values() {
    let (module, errors) = parse_module(
        r#"
fn main() usize {
let mut a = std::builtin::size[usize]();
let mut b = std::builtin::align[usize]();
const d: usize = std::builtin::error("bad");
std::builtin::trap();
a + b + d
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_values(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
}

#[test]
fn resolves_values_from_active_item_tree_only() {
    let (module, errors) = parse_module(
        r#"
@[if false]
fn skipped() usize {
unknown()
}
@[if true]
fn selected() usize {
std::builtin::size[usize]()
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let tree = ModuleItemTree::from_module(&module);
    let active = tree.active_items(&mut BoolResolver(false)).unwrap();
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let defs = collect_module_defs_from_active_item_tree(module_id, &active);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let resolved = resolve_module_values_from_active_item_tree(
        &active,
        &defs,
        ProgramDefsContext::empty(),
        &nia_defs::PublicSurfaces::default(),
        &nia_defs::ModuleUsingScope::default(),
    );
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
}

#[test]
fn resolves_value_names_embedded_in_patterns() {
    let (module, errors) = parse_module(
        r#"
const EXPECTED: i32 = 1;

fn isExpected(value: i32) bool {
    match value {
        (EXPECTED) => true,
        _ => false,
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_values(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );

    let nia_ast::ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let nia_ast::ExprKind::Match(matched) = &function
        .body
        .as_ref()
        .and_then(|body| body.tail.as_deref())
        .expect("expected match tail")
        .kind
    else {
        panic!("expected match expression");
    };
    let nia_ast::PatternKind::Expr(pattern_value) = &matched.arms[0].patterns[0].kind else {
        panic!("expected expression pattern");
    };
    assert!(matches!(
        resolved.node_names.get(&pattern_value.node_key),
        Some(ValueNameResolution::Def(_))
    ));
}

#[test]
fn resolves_primitive_associated_limits() {
    let (module, errors) = parse_module(
        r#"
fn main() i32 {
    i32::MIN
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let defs = collect_module_defs(module_id, &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);

    let resolved = resolve_module_values(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    assert_eq!(resolved.node_builtin_associated_values.len(), 1);
    assert!(matches!(
        resolved.node_builtin_associated_values.values().next(),
        Some(BuiltinAssociatedValue::PrimitiveIntLimit {
            primitive: PrimitiveTy::I32,
            kind: PrimitiveIntLimit::Min,
        })
    ));
}

#[test]
fn resolves_nominal_associated_values_through_provider() {
    let (module, errors) = parse_module(
        r#"
struct Box {}

fn main() i32 {
    Box::VALUE
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let defs = collect_module_defs(module_id, &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let type_id = GlobalDefId {
        module_id,
        def_id: defs
            .module_scope
            .types
            .get(&sym("Box"))
            .expect("missing Box definition"),
    };
    let associated_id = GlobalDefId {
        module_id,
        def_id: DefId(0xfeed),
    };
    let provider = |target: AssociatedValueTarget, name: &SymbolId| {
        (target == AssociatedValueTarget::Nominal(type_id) && *name == sym("VALUE"))
            .then_some(associated_id)
    };
    let resolved = resolve_module_values_from_active_item_tree_with_associated_values(
        &ModuleItemTree::from_module(&module)
            .active_items(&mut BoolResolver(true))
            .expect("active item tree"),
        &defs,
        ProgramDefsContext::empty(),
        &nia_defs::PublicSurfaces::default(),
        &nia_defs::ModuleUsingScope::default(),
        Some(&provider),
    );
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    assert!(
        resolved
            .node_qualified_values
            .values()
            .any(|value| *value == associated_id)
    );
}

#[test]
fn records_enum_variant_and_qualified_type_prefix() {
    let (module, errors) = parse_module(
        r#"
enum Color {
    Red,
}

fn main() Color {
    Color::Red
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let defs = collect_module_defs(module_id, &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let enum_def_id = defs
        .module_scope
        .types
        .get(&sym("Color"))
        .expect("missing Color definition");
    let enum_id = GlobalDefId {
        module_id,
        def_id: enum_def_id,
    };
    let variant_def_id = defs
        .scopes
        .enum_members
        .get(&enum_def_id)
        .and_then(|scope| scope.variants.get(&sym("Red")))
        .expect("missing Red variant");
    let variant_id = GlobalDefId {
        module_id,
        def_id: variant_def_id,
    };

    let resolved = resolve_module_values(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    assert!(
        resolved
            .node_qualified_type_prefixes
            .values()
            .any(|value| *value == enum_id)
    );
    assert!(
        resolved
            .node_qualified_values
            .values()
            .any(|value| *value == variant_id)
    );
    assert!(
        resolved
            .node_variant_enums
            .values()
            .any(|value| *value == enum_id)
    );
}

#[test]
fn standalone_expression_resolution_uses_caller_store() {
    let (module, errors) = parse_module(
        r#"
const VALUE: i32 = 1;

fn main() i32 {
    VALUE
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let defs = collect_module_defs(module_id, &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let nia_ast::ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let expr = function
        .body
        .as_ref()
        .and_then(|body| body.tail.as_deref())
        .expect("expected function tail")
        .clone();
    let store = nia_node_id::NodeStore::new();
    let resolved = resolve_module_values_from_exprs_with_associated_values_and_symbols_in_store(
        std::iter::once(expr),
        &defs,
        ProgramDefsContext::empty(),
        &nia_defs::PublicSurfaces::default(),
        &nia_defs::ModuleUsingScope::default(),
        ValueResolveOptions::with_store(None, None, &store),
    );
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    assert_eq!(resolved.node_names.store_id(), store.id());
    assert_eq!(resolved.node_qualified_values.store_id(), store.id());
    assert_eq!(
        resolved.node_builtin_associated_values.store_id(),
        store.id()
    );
    assert!(resolved.node_names.values().any(|resolution| {
        matches!(resolution, ValueNameResolution::Def(def_id) if *def_id == defs.module_scope.values.get(&sym("VALUE")).expect("missing VALUE"))
    }));
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
