// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical structural traversal for the source AST.
//!
//! A default [`Visitor`] reaches every nested type, expression, statement, and
//! pattern that is structurally owned by the node it receives. Semantic passes
//! may override callbacks to establish scopes or intentionally exclude a class
//! of nodes, but should delegate back to the corresponding `walk_*` function
//! when child traversal remains part of their contract.

use nia_ast::{
    ArrayElements, ArrayLen, Attribute, AttributeKind, Block, Expr, ExprKind, FunctionItem,
    GenericParam, GenericParamKind, IndexArg, Item, ItemKind, MatchArmBody, Module, Pattern,
    PatternKind, Stmt, StmtKind, TypeArg, TypeKind, TypeRef, WhereClause,
};

/// Callback surface for recursively walking a source AST.
///
/// Overriding a callback replaces traversal for that node. Call the matching
/// `walk_*` function from the override unless omitting its children is an
/// intentional semantic boundary.
pub trait Visitor<'ast> {
    /// Visits one item and its structurally owned children.
    fn visit_item(&mut self, item: &'ast Item) {
        walk_item(self, item);
    }

    /// Visits one function declaration and its children.
    fn visit_function(&mut self, function: &'ast FunctionItem) {
        walk_function(self, function);
    }

    /// Visits one type reference and its nested arguments.
    fn visit_type(&mut self, ty: &'ast TypeRef) {
        walk_type(self, ty);
    }

    /// Visits one lexical block and its statements/tail.
    fn visit_block(&mut self, block: &'ast Block) {
        walk_block(self, block);
    }

    /// Visits one statement and its nested expressions.
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        walk_stmt(self, stmt);
    }

    /// Visits one expression and its nested operands.
    fn visit_expr(&mut self, expr: &'ast Expr) {
        walk_expr(self, expr);
    }

    /// Visits one pattern and its nested expressions/types.
    fn visit_pattern(&mut self, pattern: &'ast Pattern) {
        walk_pattern(self, pattern);
    }

    /// Visits one generic parameter and its optional type bound.
    fn visit_generic_param(&mut self, generic: &'ast GenericParam) {
        walk_generic_param(self, generic);
    }
}

/// Visits every `static` statement reachable from a block, including blocks
/// nested inside expressions (`if`, `match`, closures, and block tails).
///
/// Static declarations are semantically item-like, but syntactically they can
/// occur anywhere a statement block is accepted. Consumers that build
/// definitions, signatures, diagnostics, or backend globals should use this
/// traversal instead of recursing over only loop statements.
pub fn walk_static_bindings<'ast>(block: &'ast Block, callback: &mut impl FnMut(&'ast Stmt)) {
    struct StaticVisitor<'a, F> {
        callback: &'a mut F,
    }

    impl<'ast, F: FnMut(&'ast Stmt)> Visitor<'ast> for StaticVisitor<'_, F> {
        fn visit_stmt(&mut self, stmt: &'ast Stmt) {
            if matches!(stmt.kind, StmtKind::Static(_)) {
                (self.callback)(stmt);
            }
            walk_stmt(self, stmt);
        }
    }

    let mut visitor = StaticVisitor { callback };
    visitor.visit_block(block);
}

/// Walks every item in a module in source order.
pub fn walk_module<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, module: &'ast Module) {
    for item in &module.items {
        visitor.visit_item(item);
    }
}

/// Walks one item and all structurally owned children.
pub fn walk_item<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, item: &'ast Item) {
    for attribute in &item.attributes {
        walk_attribute(visitor, attribute);
    }
    match &item.kind {
        ItemKind::Module(_) | ItemKind::Using(_) => {}
        ItemKind::Binding(binding) => {
            if let Some(ty) = &binding.ty {
                visitor.visit_type(ty);
            }
            if let Some(value) = &binding.value {
                visitor.visit_expr(value);
            }
        }
        ItemKind::Struct(item_struct) => {
            walk_generic_params(visitor, &item_struct.generics);
            walk_where_clause(visitor, &item_struct.where_clause);
            for field in &item_struct.fields {
                visitor.visit_type(&field.ty);
            }
        }
        ItemKind::Union(item_union) => {
            walk_generic_params(visitor, &item_union.generics);
            walk_where_clause(visitor, &item_union.where_clause);
            for field in &item_union.fields {
                visitor.visit_type(&field.ty);
            }
        }
        ItemKind::Trait(item_trait) => {
            walk_generic_params(visitor, &item_trait.generics);
            for supertrait in &item_trait.supertraits {
                visitor.visit_type(supertrait);
            }
            walk_where_clause(visitor, &item_trait.where_clause);
            for associated_value in &item_trait.associated_values {
                visitor.visit_type(&associated_value.ty);
            }
            for method in &item_trait.methods {
                visitor.visit_function(&method.function);
            }
        }
        ItemKind::Extend(extend) => {
            walk_generic_params(visitor, &extend.generics);
            visitor.visit_type(&extend.target);
            if let Some(trait_ref) = &extend.trait_ref {
                visitor.visit_type(trait_ref);
            }
            walk_where_clause(visitor, &extend.where_clause);
            for associated_type in &extend.associated_types {
                visitor.visit_type(&associated_type.ty);
            }
            for associated_value in &extend.associated_values {
                if let Some(ty) = &associated_value.binding.ty {
                    visitor.visit_type(ty);
                }
                if let Some(value) = &associated_value.binding.value {
                    visitor.visit_expr(value);
                }
            }
            for method in &extend.methods {
                visitor.visit_function(&method.function);
            }
        }
        ItemKind::Enum(item_enum) => {
            if let Some(backing_type) = &item_enum.backing_type {
                visitor.visit_type(backing_type);
            }
            for variant in &item_enum.variants {
                match &variant.payload {
                    nia_ast::EnumVariantPayload::Unit => {}
                    nia_ast::EnumVariantPayload::Tuple(fields) => {
                        for field in fields {
                            visitor.visit_type(field);
                        }
                    }
                    nia_ast::EnumVariantPayload::Named(fields) => {
                        for field in fields {
                            visitor.visit_type(&field.ty);
                        }
                    }
                }
                if let Some(value) = &variant.value {
                    visitor.visit_expr(value);
                }
            }
        }
        ItemKind::TypeAlias(alias) => {
            walk_generic_params(visitor, &alias.generics);
            walk_where_clause(visitor, &alias.where_clause);
            if let Some(ty) = &alias.ty {
                visitor.visit_type(ty);
            }
        }
        ItemKind::Function(function) => visitor.visit_function(function),
    }
}

/// Visits the types carried by const generic parameters in declaration order.
///
/// This function performs structural traversal only. Consumers that resolve or
/// lower generic names still own the lexical scope in which these callbacks
/// execute.
pub fn walk_generic_params<'ast, V: Visitor<'ast> + ?Sized>(
    visitor: &mut V,
    generics: &'ast [GenericParam],
) {
    for generic in generics {
        visitor.visit_generic_param(generic);
    }
}

/// Visits the type of one const generic parameter; type parameters have no
/// structurally owned child nodes.
pub fn walk_generic_param<'ast, V: Visitor<'ast> + ?Sized>(
    visitor: &mut V,
    generic: &'ast GenericParam,
) {
    if let GenericParamKind::Const { ty } = &generic.kind {
        visitor.visit_type(ty);
    }
}

fn walk_attribute<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, attribute: &'ast Attribute) {
    match &attribute.kind {
        AttributeKind::If(_) => {}
        AttributeKind::Meta(meta) => {
            for arg in &meta.args {
                visitor.visit_expr(arg);
            }
        }
    }
}

/// Walks all types in a where-clause predicate list.
pub fn walk_where_clause<'ast, V: Visitor<'ast> + ?Sized>(
    visitor: &mut V,
    clause: &'ast WhereClause,
) {
    for predicate in &clause.predicates {
        visitor.visit_type(&predicate.ty);
        for bound in &predicate.bounds {
            visitor.visit_type(bound);
        }
    }
}

/// Walks generic parameters, signature types, and the optional body.
pub fn walk_function<'ast, V: Visitor<'ast> + ?Sized>(
    visitor: &mut V,
    function: &'ast FunctionItem,
) {
    walk_generic_params(visitor, &function.generics);
    walk_where_clause(visitor, &function.where_clause);
    for param in &function.params {
        if let Some(ty) = &param.ty {
            visitor.visit_type(ty);
        }
    }
    if let Some(return_type) = &function.return_type {
        visitor.visit_type(return_type);
    }
    if let Some(body) = &function.body {
        visitor.visit_block(body);
    }
}

/// Walks one type reference and every nested type argument.
pub fn walk_type<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, ty: &'ast TypeRef) {
    match &ty.kind {
        TypeKind::Error
        | TypeKind::Infer
        | TypeKind::SelfType
        | TypeKind::Opaque
        | TypeKind::Never => {}
        TypeKind::Tuple { elems } => {
            for elem in elems {
                visitor.visit_type(elem);
            }
        }
        TypeKind::Path { segments } => {
            for segment in segments {
                for arg in &segment.args {
                    match arg {
                        TypeArg::Type(ty) => visitor.visit_type(ty),
                        TypeArg::AssocBinding { ty, .. } => visitor.visit_type(ty),
                        TypeArg::Const(expr) => visitor.visit_expr(expr),
                        TypeArg::TypeOrConst { ty, .. } => visitor.visit_type(ty),
                    }
                }
            }
        }
        TypeKind::Projection { ty, trait_ref, .. } => {
            visitor.visit_type(ty);
            visitor.visit_type(trait_ref);
        }
        TypeKind::Pointer { elem, .. }
        | TypeKind::VolatilePointer { elem, .. }
        | TypeKind::Slice { elem, .. }
        | TypeKind::SlicePointee { elem } => visitor.visit_type(elem),
        TypeKind::Array { len, elem } => {
            if let ArrayLen::Expr(expr) = len {
                visitor.visit_expr(expr);
            }
            visitor.visit_type(elem);
        }
        TypeKind::Range { start, end, .. } => {
            if let Some(start) = start {
                visitor.visit_type(start);
            }
            if let Some(end) = end {
                visitor.visit_type(end);
            }
        }
        TypeKind::FunctionPointer {
            params,
            return_type,
            ..
        }
        | TypeKind::Callable {
            params,
            return_type,
        } => {
            for param in params {
                visitor.visit_type(param);
            }
            if let Some(return_type) = return_type {
                visitor.visit_type(return_type);
            }
        }
        TypeKind::Optional { elem } => visitor.visit_type(elem),
        TypeKind::ErrorUnion { error, value } => {
            visitor.visit_type(error);
            visitor.visit_type(value);
        }
    }
}

/// Walks statements and the optional tail expression of a block.
pub fn walk_block<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, block: &'ast Block) {
    for stmt in &block.stmts {
        visitor.visit_stmt(stmt);
    }
    if let Some(tail) = &block.tail {
        visitor.visit_expr(tail);
    }
}

/// Walks one statement and all nested child nodes.
pub fn walk_stmt<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, stmt: &'ast Stmt) {
    for attribute in &stmt.attributes {
        walk_attribute(visitor, attribute);
    }
    match &stmt.kind {
        StmtKind::Binding(binding) => {
            visitor.visit_pattern(&binding.pattern);
            if let Some(ty) = &binding.ty {
                visitor.visit_type(ty);
            }
            if let Some(value) = &binding.value {
                visitor.visit_expr(value);
            }
        }
        StmtKind::Static(binding) => {
            if let Some(ty) = &binding.ty {
                visitor.visit_type(ty);
            }
            if let Some(value) = &binding.value {
                visitor.visit_expr(value);
            }
        }
        StmtKind::Expr(expr) | StmtKind::Defer(expr) => visitor.visit_expr(expr),
        StmtKind::Using(_) => {}
        StmtKind::Return(value) => {
            if let Some(value) = value {
                visitor.visit_expr(value);
            }
        }
        StmtKind::Break | StmtKind::Continue => {}
        StmtKind::ForIn(for_stmt) => {
            visitor.visit_pattern(&for_stmt.pattern);
            visitor.visit_expr(&for_stmt.iter);
            visitor.visit_block(&for_stmt.body);
        }
        StmtKind::While(while_stmt) => {
            visitor.visit_expr(&while_stmt.cond);
            visitor.visit_block(&while_stmt.body);
        }
        StmtKind::Loop(loop_stmt) => visitor.visit_block(&loop_stmt.body),
    }
}

/// Walks one expression and all nested operands, blocks, and patterns.
pub fn walk_expr<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, expr: &'ast Expr) {
    match &expr.kind {
        ExprKind::Error
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::ByteString(_)
        | ExprKind::Char(_)
        | ExprKind::ByteChar(_)
        | ExprKind::Raw(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::Ident(_)
        | ExprKind::SelfValue
        | ExprKind::PathRoot(_)
        | ExprKind::Underscore => {}
        ExprKind::TypeTarget { ty } => visitor.visit_type(ty),
        ExprKind::TraitTarget { ty, trait_ref } => {
            visitor.visit_type(ty);
            visitor.visit_type(trait_ref);
        }
        ExprKind::BracketSuffix { callee, args } => {
            visitor.visit_expr(callee);
            for arg in args {
                if let Some(expr) = &arg.expr {
                    visitor.visit_expr(expr);
                }
                if let Some(ty) = &arg.ty {
                    visitor.visit_type(ty);
                }
            }
        }
        ExprKind::Tuple(elems) => {
            for elem in elems {
                visitor.visit_expr(elem);
            }
        }
        ExprKind::Closure {
            captures,
            params,
            body,
        } => {
            for capture in captures {
                visitor.visit_expr(&capture.value);
            }
            for param in params {
                if let Some(ty) = &param.ty {
                    visitor.visit_type(ty);
                }
            }
            visitor.visit_expr(body);
        }
        ExprKind::ArrayLiteral { elems } => match elems {
            ArrayElements::List(elems) => {
                for elem in elems {
                    visitor.visit_expr(elem);
                }
            }
            ArrayElements::Repeat { value, count } => {
                visitor.visit_expr(value);
                visitor.visit_expr(count);
            }
        },
        ExprKind::TypedStructLiteral { ty, fields } => {
            visitor.visit_type(ty);
            for field in fields {
                visitor.visit_expr(&field.value);
            }
        }
        ExprKind::QualifiedStructLiteral { target, fields } => {
            visitor.visit_expr(target);
            for field in fields {
                visitor.visit_expr(&field.value);
            }
        }
        ExprKind::OmittedAggregateLiteral { fields } => {
            for field in fields {
                visitor.visit_expr(&field.value);
            }
        }
        ExprKind::OmittedMember { .. } => {}
        ExprKind::Unary { expr, .. }
        | ExprKind::OptionalSome { expr }
        | ExprKind::ErrorOk { expr }
        | ExprKind::ErrorErr { expr }
        | ExprKind::Try { expr } => visitor.visit_expr(expr),
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs, .. } => {
            visitor.visit_expr(lhs);
            visitor.visit_expr(rhs);
        }
        ExprKind::Cast { expr, ty } => {
            visitor.visit_expr(expr);
            visitor.visit_type(ty);
        }
        ExprKind::Call { callee, args } => {
            visitor.visit_expr(callee);
            for arg in args {
                visitor.visit_expr(arg);
            }
        }
        ExprKind::Qualified { lhs, .. }
        | ExprKind::Field { lhs, .. }
        | ExprKind::TupleField { lhs, .. } => visitor.visit_expr(lhs),
        ExprKind::Index { lhs, index } => {
            visitor.visit_expr(lhs);
            match index {
                IndexArg::Expr(expr) => visitor.visit_expr(expr),
                IndexArg::Range(range) => {
                    if let Some(start) = &range.start {
                        visitor.visit_expr(start);
                    }
                    if let Some(end) = &range.end {
                        visitor.visit_expr(end);
                    }
                }
            }
        }
        ExprKind::Range(range) => {
            if let Some(start) = &range.start {
                visitor.visit_expr(start);
            }
            if let Some(end) = &range.end {
                visitor.visit_expr(end);
            }
        }
        ExprKind::Block(block) => visitor.visit_block(block),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            visitor.visit_expr(cond);
            visitor.visit_block(then_branch);
            if let Some(else_branch) = else_branch {
                visitor.visit_expr(else_branch);
            }
        }
        ExprKind::IfPattern(if_pattern) => {
            visitor.visit_expr(&if_pattern.target);
            visitor.visit_pattern(&if_pattern.pattern);
            visitor.visit_block(&if_pattern.then_branch);
            if let Some(else_branch) = &if_pattern.else_branch {
                visitor.visit_expr(else_branch);
            }
        }
        ExprKind::Match(matched) => {
            visitor.visit_expr(&matched.target);
            for arm in &matched.arms {
                for pattern in &arm.patterns {
                    visitor.visit_pattern(pattern);
                }
                match &arm.body {
                    MatchArmBody::Expr(expr) => visitor.visit_expr(expr),
                    MatchArmBody::Stmt(stmt) => visitor.visit_stmt(stmt),
                    MatchArmBody::Block(block) => visitor.visit_block(block),
                }
            }
        }
    }
}

/// Walks expressions embedded in a pattern and recursively dispatches every
/// nested pattern through [`Visitor::visit_pattern`].
///
/// Constructor paths, expression patterns, and range endpoints participate in
/// name resolution and dependency/reachability collection just like ordinary
/// expressions. Keeping them on the visitor surface prevents those passes from
/// silently losing semantic inputs when a new pattern owner is added.
pub fn walk_pattern<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, pattern: &'ast Pattern) {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Bind { .. } | PatternKind::OptionalNull => {}
        PatternKind::Pointer(pattern)
        | PatternKind::MutPointer(pattern)
        | PatternKind::OptionalSome(pattern)
        | PatternKind::ErrorOk(pattern)
        | PatternKind::ErrorErr(pattern) => visitor.visit_pattern(pattern),
        PatternKind::Tuple(patterns) => {
            for pattern in patterns {
                visitor.visit_pattern(pattern);
            }
        }
        PatternKind::Nominal {
            constructor: variant,
            fields,
        } => {
            visitor.visit_expr(variant);
            match fields {
                nia_ast::NominalPatternFields::Tuple(fields) => {
                    for field in fields {
                        visitor.visit_pattern(field);
                    }
                }
                nia_ast::NominalPatternFields::Named { fields, .. } => {
                    for field in fields {
                        visitor.visit_pattern(&field.pattern);
                    }
                }
            }
        }
        PatternKind::Expr(pattern) => visitor.visit_expr(pattern),
        PatternKind::Range { start, end, .. } => {
            visitor.visit_expr(start);
            visitor.visit_expr(end);
        }
    }
}

#[cfg(test)]
mod tests;
