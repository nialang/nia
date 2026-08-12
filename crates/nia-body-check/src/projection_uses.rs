//! Locates associated-const projections inside const-expression ASTs.

use nia_ast::Expr;
use nia_ast_walk::{Visitor, walk_expr};
use nia_sema_ir::{AssociatedConstProjection, SemanticUseTable};
use nia_span::Span;

struct AssociatedConstProjectionUseCollector<'a> {
    semantic_uses: &'a SemanticUseTable,
    projections: Vec<(Span, AssociatedConstProjection)>,
}

impl<'ast> Visitor<'ast> for AssociatedConstProjectionUseCollector<'_> {
    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let Some(projection) = self
            .semantic_uses
            .node_associated_const_projection(&expr.node_key)
            .cloned()
        {
            self.projections.push((expr.span, projection));
        }
        // Delegate child traversal to the canonical AST walker so local
        // statics and newly added expression shapes follow the same policy as
        // every other semantic AST consumer.
        walk_expr(self, expr);
    }
}

pub(super) fn associated_const_projection_uses(
    expr: &Expr,
    semantic_uses: &SemanticUseTable,
) -> Vec<(Span, AssociatedConstProjection)> {
    let mut collector = AssociatedConstProjectionUseCollector {
        semantic_uses,
        projections: Vec::new(),
    };
    collector.visit_expr(expr);
    collector.projections
}

#[cfg(test)]
mod tests {
    use nia_ast::{ItemKind, Module};
    use nia_ast_walk::{Visitor, walk_expr};
    use nia_defs::DefId;
    use nia_ids::{GlobalDefId, ModuleIdAllocator, TraitId};
    use nia_node_id::{NodeStore, VersionedNodeKey};
    use nia_parser::parse_module_syntax_with_node_store_and_symbols;
    use nia_source::{SourceId, SourceRevision, SourceVersion};
    use nia_symbol::known;
    use nia_symbol_table::SymbolTable;
    use nia_ty::{PrimitiveTy, TypeStore};

    use super::*;

    struct ExprKeyCollector(Vec<VersionedNodeKey>);

    impl<'ast> Visitor<'ast> for ExprKeyCollector {
        fn visit_expr(&mut self, expr: &'ast Expr) {
            self.0.push(expr.node_key.clone());
            walk_expr(self, expr);
        }
    }

    fn const_value(module: &Module) -> &Expr {
        let ItemKind::Binding(binding) = &module.items[0].kind else {
            panic!("expected const binding");
        };
        binding.value.as_ref().expect("const initializer")
    }

    #[test]
    fn collector_follows_canonical_expression_traversal() {
        let source = r#"
const result: usize = {
    static cached: usize = (1, 2).0;
    let callback = [cached](value: usize) usize { (cached, value).1 };
    (callback(3), cached).0
};
"#;
        let syntax = nia_syntax::parse_source(
            source,
            Some(SourceVersion {
                id: SourceId(1),
                revision: SourceRevision::INITIAL,
            }),
        );
        let node_store = NodeStore::new();
        let (module, errors, _) = parse_module_syntax_with_node_store_and_symbols(
            &syntax,
            &node_store,
            SymbolTable::new(),
        );
        assert!(errors.is_empty(), "{errors:?}");
        let root = const_value(&module);

        let mut keys = ExprKeyCollector(Vec::new());
        keys.visit_expr(root);

        let module_id = ModuleIdAllocator::new().allocate();
        let ty = TypeStore::new()
            .append_for_module(module_id)
            .primitive(PrimitiveTy::Usize);
        let projection = AssociatedConstProjection {
            self_ty: ty,
            trait_id: TraitId::Source(GlobalDefId {
                module_id,
                def_id: DefId(1),
            }),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: known::LEN,
        };
        let mut semantic_uses = SemanticUseTable::builder_with_node_store(&node_store);
        for key in &keys.0 {
            semantic_uses.insert_node_associated_const_projection(key.clone(), projection.clone());
        }
        let semantic_uses = semantic_uses.finish();

        let uses = associated_const_projection_uses(root, &semantic_uses);
        assert_eq!(uses.len(), keys.0.len());
    }
}
