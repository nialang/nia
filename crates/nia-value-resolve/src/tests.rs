use super::*;
use nia_defs::{collect_module_defs, collect_module_defs_from_active_item_tree};
use nia_ids::ModuleIdAllocator;
use nia_item_tree::ModuleItemTree;
use nia_parser::parse_module;

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
