// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    ArrayElements, ArrayLen, Attribute, AttributeKind, Block, Expr, ExprKind, FunctionItem,
    IndexArg, Item, ItemKind, Module, Pattern, PatternKind, Stmt, StmtKind, SwitchArmBody, TypeArg,
    TypeKind, TypeRef, WhereClause,
};

pub trait Visitor<'ast> {
    fn visit_item(&mut self, item: &'ast Item) {
        walk_item(self, item);
    }

    fn visit_function(&mut self, function: &'ast FunctionItem) {
        walk_function(self, function);
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        walk_type(self, ty);
    }

    fn visit_block(&mut self, block: &'ast Block) {
        walk_block(self, block);
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        walk_expr(self, expr);
    }
}

pub fn walk_module<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, module: &'ast Module) {
    for item in &module.items {
        visitor.visit_item(item);
    }
}

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
            walk_where_clause(visitor, &item_struct.where_clause);
            for field in &item_struct.fields {
                visitor.visit_type(&field.ty);
            }
        }
        ItemKind::Union(item_union) => {
            walk_where_clause(visitor, &item_union.where_clause);
            for field in &item_union.fields {
                visitor.visit_type(&field.ty);
            }
        }
        ItemKind::Trait(item_trait) => {
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
            walk_where_clause(visitor, &alias.where_clause);
            if let Some(ty) = &alias.ty {
                visitor.visit_type(ty);
            }
        }
        ItemKind::Function(function) => visitor.visit_function(function),
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

pub fn walk_function<'ast, V: Visitor<'ast> + ?Sized>(
    visitor: &mut V,
    function: &'ast FunctionItem,
) {
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

pub fn walk_type<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, ty: &'ast TypeRef) {
    match &ty.kind {
        TypeKind::Error
        | TypeKind::Infer
        | TypeKind::SelfType
        | TypeKind::Void
        | TypeKind::Never => {}
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

pub fn walk_block<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, block: &'ast Block) {
    for stmt in &block.stmts {
        visitor.visit_stmt(stmt);
    }
    if let Some(tail) = &block.tail {
        visitor.visit_expr(tail);
    }
}

pub fn walk_stmt<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, stmt: &'ast Stmt) {
    for attribute in &stmt.attributes {
        walk_attribute(visitor, attribute);
    }
    match &stmt.kind {
        StmtKind::Binding(binding) => {
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
        ExprKind::StructLiteral { fields } => {
            for field in fields {
                visitor.visit_expr(&field.value);
            }
        }
        ExprKind::TypedArrayLiteral { ty, elems } => {
            visitor.visit_type(ty);
            match elems {
                ArrayElements::List(elems) => {
                    for elem in elems {
                        visitor.visit_expr(elem);
                    }
                }
                ArrayElements::Repeat { value, count } => {
                    visitor.visit_expr(value);
                    visitor.visit_expr(count);
                }
            }
        }
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
        ExprKind::Qualified { lhs, .. } | ExprKind::Field { lhs, .. } => visitor.visit_expr(lhs),
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
            visit_pattern(visitor, &if_pattern.pattern);
            visitor.visit_block(&if_pattern.then_branch);
            if let Some(else_branch) = &if_pattern.else_branch {
                visitor.visit_expr(else_branch);
            }
        }
        ExprKind::Switch(switch) => {
            visitor.visit_expr(&switch.target);
            for arm in &switch.arms {
                for pattern in &arm.patterns {
                    visit_pattern(visitor, pattern);
                }
                match &arm.body {
                    SwitchArmBody::Expr(expr) => visitor.visit_expr(expr),
                    SwitchArmBody::Stmt(stmt) => visitor.visit_stmt(stmt),
                    SwitchArmBody::Block(block) => visitor.visit_block(block),
                }
            }
        }
    }
}

fn visit_pattern<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, pattern: &'ast Pattern) {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Bind { .. } | PatternKind::OptionalNull => {}
        PatternKind::Pointer(pattern)
        | PatternKind::MutPointer(pattern)
        | PatternKind::OptionalSome(pattern)
        | PatternKind::ErrorOk(pattern)
        | PatternKind::ErrorErr(pattern) => visit_pattern(visitor, pattern),
        PatternKind::EnumVariant { variant, fields } => {
            visitor.visit_expr(variant);
            match fields {
                nia_ast::EnumVariantPatternFields::Tuple(fields) => {
                    for field in fields {
                        visit_pattern(visitor, field);
                    }
                }
                nia_ast::EnumVariantPatternFields::Named(fields) => {
                    for field in fields {
                        visit_pattern(visitor, &field.pattern);
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
