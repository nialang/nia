// SPDX-License-Identifier: GPL-3.0-or-later
//! Type, type-argument, and `where` clause lowering.

use nia_ast::{
    ArrayLen, AssocBindingKey, PathSegmentKind, TypeArg, TypeKind, TypePathSegment, TypeRef,
    WhereClause, WherePredicate,
};
use nia_lexer::TokenKind;
use nia_node_id::SyntaxKind as NodeSyntaxKind;
use nia_span::Span;
use nia_syntax::{AlternateKind, GreenNode, SyntaxKind};

use super::{Child, Lower, children, has_token, nodes_where, token_kind};

impl Lower<'_> {
    /// Lowers the first direct type child.
    pub(crate) fn child_type(&mut self, node: &GreenNode) -> Option<TypeRef> {
        let ty = node.child_nodes().find(|child| child.kind().is_type())?;
        Some(self.type_ref(ty))
    }

    pub(crate) fn type_ref(&mut self, node: &GreenNode) -> TypeRef {
        let span = node.span();
        let kind = self.type_kind(node);
        self.make_type(span, kind)
    }

    fn make_type(&mut self, span: Span, kind: TypeKind) -> TypeRef {
        let node_key = self.node_key(NodeSyntaxKind::Type, span);
        TypeRef {
            span,
            node_key,
            text: self.text(span),
            kind,
        }
    }

    fn type_children(&mut self, node: &GreenNode) -> Vec<TypeRef> {
        nodes_where(node, SyntaxKind::is_type)
            .map(|ty| self.type_ref(ty))
            .collect()
    }

    fn boxed_child_type(&mut self, node: &GreenNode) -> Box<TypeRef> {
        Box::new(
            self.child_type(node)
                .unwrap_or_else(|| self.error_type(node.span())),
        )
    }

    fn error_type(&mut self, span: Span) -> TypeRef {
        self.make_type(span, TypeKind::Error)
    }

    fn type_kind(&mut self, node: &GreenNode) -> TypeKind {
        let readonly = !has_token(node, TokenKind::Mut);
        match node.kind() {
            SyntaxKind::PathType => TypeKind::Path {
                segments: nodes_where(node, |kind| *kind == SyntaxKind::PathSegment)
                    .map(|segment| self.path_segment(segment))
                    .collect(),
            },
            SyntaxKind::ProjectionType => {
                let mut types = nodes_where(node, SyntaxKind::is_type);
                let (Some(ty), Some(trait_ref)) = (types.next(), types.next()) else {
                    return TypeKind::Error;
                };
                let ty = Box::new(self.type_ref(ty));
                let trait_ref = Box::new(self.type_ref(trait_ref));
                let name = node
                    .child_tokens()
                    .filter(|token| token.is(&TokenKind::Ident))
                    .last()
                    .map_or(nia_symbol::SymbolId::EMPTY, |token| self.name(token));
                TypeKind::Projection {
                    ty,
                    trait_ref,
                    name,
                }
            }
            SyntaxKind::PointerType => TypeKind::Pointer {
                is_readonly: readonly,
                elem: self.boxed_child_type(node),
            },
            SyntaxKind::VolatilePointerType => TypeKind::VolatilePointer {
                is_readonly: readonly,
                elem: self.boxed_child_type(node),
            },
            SyntaxKind::SliceType => TypeKind::Slice {
                is_readonly: readonly,
                elem: self.boxed_child_type(node),
            },
            SyntaxKind::SlicePointeeType => TypeKind::SlicePointee {
                elem: self.boxed_child_type(node),
            },
            SyntaxKind::ArrayType => {
                let elem = self.boxed_child_type(node);
                let len = match self.child_expr(node) {
                    Some(expr) => ArrayLen::Expr(Box::new(expr)),
                    None => ArrayLen::Infer,
                };
                TypeKind::Array { len, elem }
            }
            SyntaxKind::TupleType => TypeKind::Tuple {
                elems: self.type_children(node),
            },
            SyntaxKind::ParenType => {
                match node.child_nodes().find(|child| child.kind().is_type()) {
                    Some(inner) => self.type_ref(inner).kind,
                    None => TypeKind::Error,
                }
            }
            SyntaxKind::RangeType => self.range_type(node),
            SyntaxKind::FnPointerType => {
                let (params, return_type) = self.signature_types(node);
                TypeKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic: has_token(node, TokenKind::Ellipsis),
                }
            }
            SyntaxKind::CallableType => {
                let (params, return_type) = self.signature_types(node);
                TypeKind::Callable {
                    params,
                    return_type,
                }
            }
            SyntaxKind::OptionalType => TypeKind::Optional {
                elem: self.boxed_child_type(node),
            },
            SyntaxKind::ErrorUnionType => {
                let mut types = nodes_where(node, SyntaxKind::is_type);
                let (Some(error), Some(value)) = (types.next(), types.next()) else {
                    return TypeKind::Error;
                };
                TypeKind::ErrorUnion {
                    error: Box::new(self.type_ref(error)),
                    value: Box::new(self.type_ref(value)),
                }
            }
            SyntaxKind::SelfType => TypeKind::SelfType,
            SyntaxKind::OpaqueType => TypeKind::Opaque,
            SyntaxKind::NeverType => TypeKind::Never,
            SyntaxKind::InferType => TypeKind::Infer,
            _ => TypeKind::Error,
        }
    }

    /// Parameter types inside `( ... )` and the optional return type after it.
    fn signature_types(&mut self, node: &GreenNode) -> (Vec<TypeRef>, Option<Box<TypeRef>>) {
        let mut params = Vec::new();
        let mut return_type = None;
        let mut closed = false;
        for child in children(node) {
            match child {
                Child::Token(token) if token.is(&TokenKind::RParen) => closed = true,
                Child::Node(ty) if ty.kind().is_type() => {
                    let ty = self.type_ref(ty);
                    if closed {
                        return_type = Some(Box::new(ty));
                    } else {
                        params.push(ty);
                    }
                }
                _ => {}
            }
        }
        (params, return_type)
    }

    fn range_type(&mut self, node: &GreenNode) -> TypeKind {
        let mut start = None;
        let mut end = None;
        let mut inclusive = false;
        let mut after_operator = false;
        for child in children(node) {
            match child {
                Child::Token(token)
                    if matches!(token_kind(token), TokenKind::DotDot | TokenKind::DotDotEq) =>
                {
                    inclusive = token.is(&TokenKind::DotDotEq);
                    after_operator = true;
                }
                Child::Node(ty) if ty.kind().is_type() => {
                    let ty = Box::new(self.type_ref(ty));
                    if after_operator {
                        end = Some(ty);
                    } else {
                        start = Some(ty);
                    }
                }
                _ => {}
            }
        }
        TypeKind::Range {
            start,
            end,
            inclusive,
        }
    }

    fn path_segment(&mut self, node: &GreenNode) -> TypePathSegment {
        let Some(token) = node.child_tokens().next() else {
            return TypePathSegment {
                kind: PathSegmentKind::Package,
                span: node.span(),
                args: Vec::new(),
            };
        };
        let span = token.span();
        let kind = self.path_segment_kind(token);
        let args = node
            .child(&SyntaxKind::TypeArgList)
            .map_or_else(Vec::new, |list| self.type_args(list));
        TypePathSegment { kind, span, args }
    }

    pub(crate) fn type_args(&mut self, list: &GreenNode) -> Vec<TypeArg> {
        let mut args = Vec::new();
        for arg in list.child_nodes() {
            match arg.kind() {
                SyntaxKind::TypeArg => {
                    if let Some(ty) = self.child_type(arg) {
                        args.push(TypeArg::Type(ty));
                    }
                }
                SyntaxKind::ConstArg => {
                    if let Some(expr) = self.child_expr(arg) {
                        args.push(TypeArg::Const(expr));
                    }
                }
                SyntaxKind::TypeOrConstArg => {
                    let Some(ty) = self.child_type(arg) else {
                        continue;
                    };
                    let expr = self
                        .alternate(AlternateKind::ConstArg, arg.span())
                        .cloned()
                        .and_then(|alternate| self.child_expr(&alternate));
                    args.push(match expr {
                        Some(expr) => TypeArg::TypeOrConst { ty, expr },
                        None => TypeArg::Type(ty),
                    });
                }
                SyntaxKind::AssocBindingArg => {
                    let mut types = nodes_where(arg, SyntaxKind::is_type);
                    let (Some(key), Some(value)) = (types.next(), types.next()) else {
                        continue;
                    };
                    let key = self.type_ref(key);
                    let ty = self.type_ref(value);
                    let key = match &key.kind {
                        TypeKind::Path { segments } if segments.len() == 1 => {
                            match segments[0].kind {
                                PathSegmentKind::Name(name) => AssocBindingKey::Name(name),
                                _ => AssocBindingKey::Projection(key),
                            }
                        }
                        _ => AssocBindingKey::Projection(key),
                    };
                    args.push(TypeArg::AssocBinding {
                        key,
                        span: arg.span(),
                        ty,
                    });
                }
                _ => {}
            }
        }
        args
    }

    pub(crate) fn where_clause(&mut self, node: &GreenNode) -> WhereClause {
        let Some(clause) = node.child(&SyntaxKind::WhereClause) else {
            return WhereClause::default();
        };
        let predicates = nodes_where(clause, |kind| *kind == SyntaxKind::WherePredicate)
            .filter_map(|predicate| {
                let mut types = self.type_children(predicate).into_iter();
                let ty = types.next()?;
                Some(WherePredicate {
                    ty,
                    bounds: types.collect(),
                    span: predicate.span(),
                })
            })
            .collect();
        WhereClause { predicates }
    }
}
