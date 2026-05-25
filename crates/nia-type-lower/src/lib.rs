// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{
    ArrayLen, Expr, ExprKind, FunctionItem, Item, ItemKind, Module, TypeArg, TypeKind,
    TypePathSegment, TypeRef,
};
use nia_ast_walk::{Visitor, walk_module};
use nia_defs::DefCollection;
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, ModuleId, TyId};
use nia_span::Span;
use nia_ty::{ArrayLenTy, PrimitiveTy, TyInterner, TyKind};
use nia_type_resolve::{PrimitiveType, TypeNameResolution, TypeResolution};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeLowering {
    pub interner: TyInterner,
    pub type_uses: HashMap<Span, TyId>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn lower_module_types(module: &Module, resolved: &TypeResolution) -> TypeLowering {
    lower_module_types_with_id(ModuleId(0), module, resolved)
}

pub fn lower_module_types_with_id(
    module_id: ModuleId,
    module: &Module,
    resolved: &TypeResolution,
) -> TypeLowering {
    lower_module_types_with_defs(module_id, module, resolved, &[])
}

pub fn lower_module_types_with_defs(
    module_id: ModuleId,
    module: &Module,
    resolved: &TypeResolution,
    all_defs: &[DefCollection],
) -> TypeLowering {
    let mut lowerer = TypeLowerer {
        module_id,
        resolved,
        all_defs,
        interner: TyInterner::new(),
        type_uses: HashMap::new(),
        diagnostics: Vec::new(),
        generic_stack: Vec::new(),
    };
    walk_module(&mut lowerer, module);
    TypeLowering {
        interner: lowerer.interner,
        type_uses: lowerer.type_uses,
        diagnostics: lowerer.diagnostics,
    }
}

struct TypeLowerer<'a> {
    module_id: ModuleId,
    resolved: &'a TypeResolution,
    all_defs: &'a [DefCollection],
    interner: TyInterner,
    type_uses: HashMap<Span, TyId>,
    diagnostics: Vec<Diagnostic>,
    generic_stack: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeContext {
    Value,
    Return,
    Alias,
    SizeQuery,
}

impl<'ast> Visitor<'ast> for TypeLowerer<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        match &item.kind {
            ItemKind::Struct(item_struct) => {
                self.with_generics(&item_struct.generics, |lowerer| {
                    for field in &item_struct.fields {
                        lowerer.lower_type_in_context(&field.ty, TypeContext::Value);
                    }
                });
            }
            ItemKind::Extend(extend) => {
                self.with_generics(&extend.generics, |lowerer| {
                    lowerer.lower_type_in_context(&extend.target, TypeContext::Value);
                    for method in &extend.methods {
                        lowerer.visit_function(&method.function);
                    }
                });
            }
            ItemKind::Enum(item_enum) => {
                if let Some(backing_type) = &item_enum.backing_type {
                    let ty = self.lower_type_in_context(backing_type, TypeContext::Value);
                    if !self.is_integer(ty) {
                        self.diagnostics.push(Diagnostic::error(
                            backing_type.span,
                            "enum backing type must be an integer type",
                        ));
                    }
                }
            }
            ItemKind::TypeAlias(alias) => {
                self.with_generics(&alias.generics, |lowerer| {
                    lowerer.lower_type_in_context(&alias.ty, TypeContext::Alias);
                });
            }
            ItemKind::Binding(binding) => {
                if let Some(ty) = &binding.ty {
                    self.lower_type_in_context(ty, TypeContext::Value);
                }
                if let Some(value) = &binding.value {
                    nia_ast_walk::walk_expr(self, value);
                }
            }
            ItemKind::Function(function) => self.visit_function(function),
            ItemKind::Import(_) | ItemKind::Using(_) => {}
        }
    }

    fn visit_function(&mut self, function: &'ast FunctionItem) {
        self.with_generics(&function.generics, |lowerer| {
            for param in &function.params {
                if let Some(ty) = &param.ty {
                    lowerer.lower_type_in_context(ty, TypeContext::Value);
                }
            }
            if let Some(return_type) = &function.return_type {
                lowerer.lower_type_in_context(return_type, TypeContext::Return);
            }
            if let Some(body) = &function.body {
                lowerer.visit_block(body);
            }
        });
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        self.lower_type_in_context(ty, TypeContext::Value);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        match &expr.kind {
            ExprKind::BracketSuffix { callee, args } => {
                self.visit_expr(callee);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        self.visit_expr(expr);
                    }
                    if let Some(ty) = &arg.ty {
                        self.visit_type(ty);
                    }
                }
            }
            _ => nia_ast_walk::walk_expr(self, expr),
        }
    }
}

impl<'a> TypeLowerer<'a> {
    fn lower_type_in_context(&mut self, ty: &TypeRef, context: TypeContext) -> TyId {
        let lowered = self.lower_type(ty, context);
        self.type_uses.insert(ty.span, lowered);
        if context == TypeContext::Value && self.is_invalid_value_type(lowered) {
            self.diagnostics.push(Diagnostic::error(
                ty.span,
                "`void` and `!` are not valid as value, field, parameter, or array element types",
            ));
        }
        lowered
    }

    fn lower_type(&mut self, ty: &TypeRef, _context: TypeContext) -> TyId {
        match &ty.kind {
            TypeKind::Error => self.interner.error(),
            TypeKind::Infer => {
                self.diagnostics.push(Diagnostic::error(
                    ty.span,
                    "`_` type inference is not valid in this type lowering context",
                ));
                self.interner.error()
            }
            TypeKind::Void => self.interner.primitive(PrimitiveTy::Void),
            TypeKind::Never => self.interner.primitive(PrimitiveTy::Never),
            TypeKind::Pointer { is_const, elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.interner.intern(TyKind::Pointer {
                    is_const: *is_const,
                    elem,
                })
            }
            TypeKind::Slice { is_const, elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.interner.intern(TyKind::Slice {
                    is_const: *is_const,
                    elem,
                })
            }
            TypeKind::Array { len, elem } => {
                let len = self.lower_array_len(len);
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.interner.intern(TyKind::Array { len, elem })
            }
            TypeKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => {
                let params = params
                    .iter()
                    .map(|param| self.lower_type_in_context(param, TypeContext::Value))
                    .collect();
                let return_type = match return_type {
                    Some(return_type) => {
                        self.lower_type_in_context(return_type, TypeContext::Return)
                    }
                    None => self.interner.primitive(PrimitiveTy::Void),
                };
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic: *is_variadic,
                })
            }
            TypeKind::Path { segments } => {
                let Some(first) = segments.first() else {
                    return self.interner.error();
                };
                let Some(type_segment) = type_name_segment(segments) else {
                    return self.interner.error();
                };
                match self.resolved.type_names.get(&ty.span).copied() {
                    Some(TypeNameResolution::Primitive(primitive)) => {
                        self.interner.primitive(lower_primitive(primitive))
                    }
                    Some(TypeNameResolution::GenericParam) => self
                        .interner
                        .intern(TyKind::GenericParam(first.name.clone())),
                    Some(TypeNameResolution::Def(def_id)) => {
                        let def_id = self
                            .resolved
                            .qualified_type_names
                            .get(&ty.span)
                            .copied()
                            .unwrap_or(GlobalDefId {
                                module_id: self.module_id,
                                def_id,
                            });
                        let mut args = Vec::new();
                        for arg in &type_segment.args {
                            match arg {
                                TypeArg::Type(arg_ty) => args
                                    .push(self.lower_type_in_context(arg_ty, TypeContext::Value)),
                                TypeArg::Const(expr) => {
                                    self.diagnostics.push(Diagnostic::error(
                                        expr.span,
                                        "const generic type arguments are not supported",
                                    ));
                                }
                            }
                        }
                        self.check_type_arg_count(ty.span, def_id, args.len());
                        self.interner.intern(TyKind::Nominal { def_id, args })
                    }
                    Some(TypeNameResolution::External(global_id)) => {
                        let mut args = Vec::new();
                        for arg in &type_segment.args {
                            match arg {
                                TypeArg::Type(arg_ty) => args
                                    .push(self.lower_type_in_context(arg_ty, TypeContext::Value)),
                                TypeArg::Const(expr) => {
                                    self.diagnostics.push(Diagnostic::error(
                                        expr.span,
                                        "const generic type arguments are not supported",
                                    ));
                                }
                            }
                        }
                        self.check_type_arg_count(ty.span, global_id, args.len());
                        self.interner.intern(TyKind::Nominal {
                            def_id: global_id,
                            args,
                        })
                    }
                    Some(TypeNameResolution::Error) | None => self.interner.error(),
                }
            }
        }
    }

    fn check_type_arg_count(&mut self, span: Span, def_id: GlobalDefId, actual: usize) {
        let Some(defs) = self
            .all_defs
            .iter()
            .find(|defs| defs.module_id == def_id.module_id)
        else {
            return;
        };
        let Some(def) = defs.defs.get(def_id.def_id) else {
            return;
        };
        let expected = def.generics.len();
        if expected != actual {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "generic argument count mismatch for `{}`: expected {expected}, got {actual}",
                    def.name
                ),
            ));
        }
    }

    fn with_generics(&mut self, generics: &[String], f: impl FnOnce(&mut Self)) {
        self.generic_stack.push(generics.to_vec());
        f(self);
        self.generic_stack.pop();
    }

    fn lower_array_len(&mut self, len: &ArrayLen) -> ArrayLenTy {
        match len {
            ArrayLen::Infer => ArrayLenTy::Infer,
            ArrayLen::Expr(expr) => self.lower_array_len_expr(expr),
        }
    }

    fn lower_array_len_expr(&mut self, expr: &Expr) -> ArrayLenTy {
        match &expr.kind {
            ExprKind::Builtin {
                name,
                type_arg: Some(type_arg),
            } if name == "size" || name == "align" => ArrayLenTy::Builtin {
                name: name.clone(),
                ty: self.lower_type_in_context(type_arg, TypeContext::SizeQuery),
            },
            ExprKind::Call { callee, args }
                if args.is_empty()
                    && matches!(
                        &callee.kind,
                        ExprKind::Builtin {
                            name,
                            type_arg: Some(_),
                        } if name == "size" || name == "align"
                    ) =>
            {
                let ExprKind::Builtin {
                    name,
                    type_arg: Some(type_arg),
                } = &callee.kind
                else {
                    unreachable!();
                };
                ArrayLenTy::Builtin {
                    name: name.clone(),
                    ty: self.lower_type_in_context(type_arg, TypeContext::SizeQuery),
                }
            }
            _ => ArrayLenTy::ConstExpr(expr_text(expr)),
        }
    }

    fn is_integer(&self, ty: TyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
            ))
        )
    }

    fn is_invalid_value_type(&self, ty: TyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::Void | PrimitiveTy::Never))
        )
    }
}

fn expr_text(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Integer(text) | ExprKind::Raw(text) => text.clone(),
        ExprKind::Unary {
            op: nia_ast::UnaryOp::Neg,
            expr,
        } => format!("-{}", expr_text(expr)),
        ExprKind::Unary {
            op: nia_ast::UnaryOp::Not,
            expr,
        } => format!("!{}", expr_text(expr)),
        ExprKind::Binary { lhs, op, rhs } => {
            format!(
                "{} {} {}",
                expr_text(lhs),
                binary_op_text(*op),
                expr_text(rhs)
            )
        }
        ExprKind::Builtin { name, .. } => format!("@{name}"),
        _ => "<const-expr>".to_string(),
    }
}

fn binary_op_text(op: nia_ast::BinaryOp) -> &'static str {
    match op {
        nia_ast::BinaryOp::Mul => "*",
        nia_ast::BinaryOp::Div => "/",
        nia_ast::BinaryOp::Rem => "%",
        nia_ast::BinaryOp::Add => "+",
        nia_ast::BinaryOp::Sub => "-",
        nia_ast::BinaryOp::Shl => "<<",
        nia_ast::BinaryOp::Shr => ">>",
        nia_ast::BinaryOp::Lt => "<",
        nia_ast::BinaryOp::Le => "<=",
        nia_ast::BinaryOp::Gt => ">",
        nia_ast::BinaryOp::Ge => ">=",
        nia_ast::BinaryOp::Eq => "==",
        nia_ast::BinaryOp::Ne => "!=",
        nia_ast::BinaryOp::BitAnd => "&",
        nia_ast::BinaryOp::BitXor => "^",
        nia_ast::BinaryOp::BitOr => "|",
        nia_ast::BinaryOp::And => "and",
        nia_ast::BinaryOp::Or => "or",
    }
}

fn type_name_segment(segments: &[TypePathSegment]) -> Option<&TypePathSegment> {
    match segments {
        [segment] => Some(segment),
        [_, segment] => Some(segment),
        _ => None,
    }
}

fn lower_primitive(primitive: PrimitiveType) -> PrimitiveTy {
    match primitive {
        PrimitiveType::I8 => PrimitiveTy::I8,
        PrimitiveType::I16 => PrimitiveTy::I16,
        PrimitiveType::I32 => PrimitiveTy::I32,
        PrimitiveType::I64 => PrimitiveTy::I64,
        PrimitiveType::I128 => PrimitiveTy::I128,
        PrimitiveType::Isize => PrimitiveTy::Isize,
        PrimitiveType::U8 => PrimitiveTy::U8,
        PrimitiveType::U16 => PrimitiveTy::U16,
        PrimitiveType::U32 => PrimitiveTy::U32,
        PrimitiveType::U64 => PrimitiveTy::U64,
        PrimitiveType::U128 => PrimitiveTy::U128,
        PrimitiveType::Usize => PrimitiveTy::Usize,
        PrimitiveType::F32 => PrimitiveTy::F32,
        PrimitiveType::F64 => PrimitiveTy::F64,
        PrimitiveType::Bool => PrimitiveTy::Bool,
        PrimitiveType::Char => PrimitiveTy::Char,
        PrimitiveType::Void => PrimitiveTy::Void,
        PrimitiveType::Never => PrimitiveTy::Never,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_parser::parse_module;
    use nia_type_resolve::resolve_module_types;

    #[test]
    fn lowers_primitive_pointer_array_function_and_nominal_types() {
        let (module, errors) = parse_module(
            r#"
struct Box[T] {
    value: T,
}

fn make(ptr: &const u8, cb: &const fn(i32) void) [4]Box[i32] {
    var tmp: [_]i32 = [1, 2, 3];
    [{ value: 0 }; 4]
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let lowered = lower_module_types(&module, &resolved);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        assert!(
            lowered
                .interner
                .get(TyId(0))
                .is_some_and(|ty| matches!(ty, TyKind::Error))
        );
        assert!(
            lowered
                .interner
                .get(TyId(1))
                .is_some_and(|_| lowered.interner.len() > 1)
        );
        assert!(
            lowered
                .type_uses
                .values()
                .any(|ty_id| matches!(lowered.interner.get(*ty_id), Some(TyKind::Nominal { .. })))
        );
        assert!(
            lowered
                .type_uses
                .values()
                .any(|ty_id| matches!(lowered.interner.get(*ty_id), Some(TyKind::Array { .. })))
        );
        assert!(
            lowered
                .type_uses
                .values()
                .any(|ty_id| matches!(lowered.interner.get(*ty_id), Some(TyKind::Pointer { .. })))
        );
    }

    #[test]
    fn rejects_const_generic_type_arguments() {
        let (module, errors) = parse_module(
            r#"
struct Box[T] {
    value: T,
}

fn make() Box[4] {
    { value: 0 }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types(&module, &resolved);
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("const generic"))
        );
    }

    #[test]
    fn reports_generic_type_argument_count_mismatches() {
        let (module, errors) = parse_module(
            r#"
struct Point {}
struct Box[T] { value: T }
type Pair[T, U] = T;
fn missing_arg(a: Box) {}
fn extra_arg(a: Box[i32, bool]) {}
fn alias_missing_arg(a: Pair[i32]) {}
fn non_generic_arg(a: Point[i32]) {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let lowered = lower_module_types_with_defs(
            ModuleId(0),
            &module,
            &resolved,
            std::slice::from_ref(&defs),
        );
        let mismatch_count = lowered
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .message
                    .contains("generic argument count mismatch")
            })
            .count();
        assert_eq!(mismatch_count, 4, "{:?}", lowered.diagnostics);
    }

    #[test]
    fn rejects_invalid_void_value_types_and_enum_backing_types() {
        let (module, errors) = parse_module(
            r#"
enum Bad: bool {
    A,
}

struct BadFields {
    field: void,
    array: [1]void,
    never_field: !,
}

fn bad_param(x: void) void {}
fn bad_never_param(x: !) void {}
fn good_return() void {}
fn good_never_return() ! {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types(&module, &resolved);
        assert!(
            lowered.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("enum backing type must be an integer type")),
            "{:?}",
            lowered.diagnostics
        );
        assert_eq!(
            lowered
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("`void` and `!` are not valid"))
                .count(),
            5,
            "{:?}",
            lowered.diagnostics
        );
    }
}
