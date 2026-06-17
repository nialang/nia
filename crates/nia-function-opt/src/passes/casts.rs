use super::*;

pub(crate) fn simplify_same_type_casts_in_blocks(blocks: &mut [FunctionBlock]) -> bool {
    rewrite_blocks_exprs(blocks, ExprTraversal::all(), &mut |expr| {
        simplify_same_type_casts_in_expr(expr)
    })
}

pub(crate) fn simplify_same_type_casts_in_expr(expr: &mut FunctionExpr) -> bool {
    let mut changed = rewrite_expr_children(expr, ExprTraversal::all(), &mut |expr| {
        simplify_same_type_casts_in_expr(expr)
    });
    if let FunctionExprKind::Cast { expr: inner, ty } = &mut expr.kind
        && inner.ty == *ty
    {
        let mut inner = (**inner).clone();
        inner.span = expr.span;
        *expr = inner;
        changed = true;
    }
    changed
}
