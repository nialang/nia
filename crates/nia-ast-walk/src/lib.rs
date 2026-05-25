// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    ArrayElements, ArrayLen, Block, Expr, ExprKind, ForHeader, ForInit, FunctionItem, IndexArg,
    Item, ItemKind, Module, Stmt, StmtKind, SwitchArmBody, SwitchPattern, TypeArg, TypeKind,
    TypeRef,
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
    match &item.kind {
        ItemKind::Import(_) | ItemKind::Using(_) => {}
        ItemKind::Binding(binding) => {
            if let Some(ty) = &binding.ty {
                visitor.visit_type(ty);
            }
            if let Some(value) = &binding.value {
                visitor.visit_expr(value);
            }
        }
        ItemKind::Struct(item_struct) => {
            for field in &item_struct.fields {
                visitor.visit_type(&field.ty);
            }
        }
        ItemKind::Extend(extend) => {
            visitor.visit_type(&extend.target);
            for method in &extend.methods {
                visitor.visit_function(&method.function);
            }
        }
        ItemKind::Enum(item_enum) => {
            if let Some(backing_type) = &item_enum.backing_type {
                visitor.visit_type(backing_type);
            }
            for variant in &item_enum.variants {
                if let Some(value) = &variant.value {
                    visitor.visit_expr(value);
                }
            }
        }
        ItemKind::TypeAlias(alias) => visitor.visit_type(&alias.ty),
        ItemKind::Function(function) => visitor.visit_function(function),
    }
}

pub fn walk_function<'ast, V: Visitor<'ast> + ?Sized>(
    visitor: &mut V,
    function: &'ast FunctionItem,
) {
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
        TypeKind::Error | TypeKind::Infer | TypeKind::Void | TypeKind::Never => {}
        TypeKind::Path { segments } => {
            for segment in segments {
                for arg in &segment.args {
                    if let TypeArg::Type(ty) = arg {
                        visitor.visit_type(ty);
                    }
                }
            }
        }
        TypeKind::Pointer { elem, .. } | TypeKind::Slice { elem, .. } => visitor.visit_type(elem),
        TypeKind::Array { len, elem } => {
            if let ArrayLen::Expr(expr) = len {
                visitor.visit_expr(expr);
            }
            visitor.visit_type(elem);
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
    match &stmt.kind {
        StmtKind::Binding(binding) => {
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
        StmtKind::For(for_stmt) => {
            match &for_stmt.header {
                ForHeader::Infinite => {}
                ForHeader::Condition(cond) => visitor.visit_expr(cond),
                ForHeader::CStyle { init, cond, step } => {
                    if let Some(init) = init {
                        walk_for_init(visitor, init);
                    }
                    if let Some(cond) = cond {
                        visitor.visit_expr(cond);
                    }
                    if let Some(step) = step {
                        visitor.visit_expr(step);
                    }
                }
            }
            visitor.visit_block(&for_stmt.body);
        }
        StmtKind::Switch(switch) => {
            visitor.visit_expr(&switch.target);
            for arm in &switch.arms {
                if let SwitchPattern::Expr(pattern) = &arm.pattern {
                    visitor.visit_expr(pattern);
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

pub fn walk_for_init<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, init: &'ast ForInit) {
    match init {
        ForInit::Binding { binding, .. } => {
            if let Some(ty) = &binding.ty {
                visitor.visit_type(ty);
            }
            if let Some(value) = &binding.value {
                visitor.visit_expr(value);
            }
        }
        ForInit::Expr(expr) => visitor.visit_expr(expr),
    }
}

pub fn walk_expr<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, expr: &'ast Expr) {
    match &expr.kind {
        ExprKind::Error
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Char(_)
        | ExprKind::ByteChar(_)
        | ExprKind::Raw(_)
        | ExprKind::Bool(_)
        | ExprKind::Ident(_)
        | ExprKind::Underscore => {}
        ExprKind::Builtin { type_arg, .. } => {
            if let Some(type_arg) = type_arg {
                visitor.visit_type(type_arg);
            }
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
            ArrayElements::Repeat { value, .. } => visitor.visit_expr(value),
        },
        ExprKind::StructLiteral { fields } => {
            for field in fields {
                visitor.visit_expr(&field.value);
            }
        }
        ExprKind::Unary { expr, .. } => visitor.visit_expr(expr),
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
    }
}
