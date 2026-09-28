// SPDX-License-Identifier: GPL-3.0-or-later
//! Pattern lowering.

use nia_ast::{ExprKind, NamedPatternField, NominalPatternFields, Pattern, PatternKind};
use nia_lexer::TokenKind;
use nia_node_id::SyntaxKind as NodeSyntaxKind;
use nia_span::Span;
use nia_syntax::{GreenNode, SyntaxKind};

use super::stmts::mark_bindings_mutable;
use super::{Lower, has_token, nodes_where};

impl Lower<'_> {
    /// Lowers a pattern; `refutable` selects `match`/`is` semantics, where a
    /// parenthesized name is a value expression rather than a binding.
    pub(crate) fn pattern(&mut self, node: &GreenNode, refutable: bool) -> Pattern {
        let span = node.span();
        let kind = match node.kind() {
            SyntaxKind::WildcardPattern => PatternKind::Wildcard,
            SyntaxKind::BindPattern => self.bind(node),
            SyntaxKind::MutPattern => {
                let mut inner = self.inner_pattern(node, refutable);
                mark_bindings_mutable(&mut inner);
                inner.kind
            }
            SyntaxKind::PointerPattern => {
                let inner = Box::new(self.inner_pattern(node, refutable));
                if has_token(node, TokenKind::Mut) {
                    PatternKind::MutPointer(inner)
                } else {
                    PatternKind::Pointer(inner)
                }
            }
            SyntaxKind::OptionalPattern => {
                PatternKind::OptionalSome(Box::new(self.inner_pattern(node, true)))
            }
            SyntaxKind::NullPattern => PatternKind::OptionalNull,
            SyntaxKind::ErrorOkPattern => {
                PatternKind::ErrorOk(Box::new(self.inner_pattern(node, true)))
            }
            SyntaxKind::ErrorErrPattern => {
                PatternKind::ErrorErr(Box::new(self.inner_pattern(node, true)))
            }
            SyntaxKind::TuplePattern => PatternKind::Tuple(
                nodes_where(node, SyntaxKind::is_pattern)
                    .map(|field| self.pattern(field, refutable))
                    .collect(),
            ),
            SyntaxKind::ParenPattern => {
                let inner = self.inner_pattern(node, refutable);
                match inner.kind {
                    PatternKind::Bind { name, .. } if refutable => PatternKind::Expr(Box::new(
                        self.make_expr(inner.span, ExprKind::Ident(name)),
                    )),
                    kind => kind,
                }
            }
            SyntaxKind::NominalPattern => self.nominal(node),
            SyntaxKind::ExprPattern => match self.child_expr(node) {
                Some(expr) => PatternKind::Expr(Box::new(expr)),
                None => PatternKind::Wildcard,
            },
            SyntaxKind::RangePattern => match self.child_expr(node).map(|expr| expr.kind) {
                Some(ExprKind::Range(range)) => match (range.start, range.end) {
                    (Some(start), Some(end)) => PatternKind::Range {
                        start,
                        end,
                        inclusive: range.inclusive,
                    },
                    _ => PatternKind::Wildcard,
                },
                _ => PatternKind::Wildcard,
            },
            _ => PatternKind::Wildcard,
        };
        Pattern { span, kind }
    }

    fn inner_pattern(&mut self, node: &GreenNode, refutable: bool) -> Pattern {
        match node.child_nodes().find(|child| child.kind().is_pattern()) {
            Some(inner) => self.pattern(inner, refutable),
            None => Pattern {
                span: node.span(),
                kind: PatternKind::Wildcard,
            },
        }
    }

    fn bind(&mut self, node: &GreenNode) -> PatternKind {
        let span = node.span();
        let name = self.first_name(node);
        self.bind_at(name, span)
    }

    fn bind_at(&mut self, name: nia_symbol::SymbolId, span: Span) -> PatternKind {
        PatternKind::Bind {
            name,
            node_key: self.node_key(NodeSyntaxKind::Pattern, span),
            is_mutable: false,
        }
    }

    fn nominal(&mut self, node: &GreenNode) -> PatternKind {
        let constructor = node.child_nodes().find(|child| child.kind().is_expr());
        let constructor = Box::new(match constructor {
            Some(constructor) => self.expr(constructor),
            None => self.make_expr(node.span(), ExprKind::Error),
        });
        let fields = if let Some(fields) = node.child(&SyntaxKind::NominalTupleFields) {
            NominalPatternFields::Tuple(
                nodes_where(fields, SyntaxKind::is_pattern)
                    .map(|field| self.pattern(field, true))
                    .collect(),
            )
        } else if let Some(fields) = node.child(&SyntaxKind::NominalNamedFields) {
            let rest = fields
                .child_tokens()
                .filter(|token| token.is(&TokenKind::DotDot))
                .last()
                .map(|token| token.span());
            let fields = nodes_where(fields, |kind| *kind == SyntaxKind::NamedPatternField)
                .filter_map(|field| {
                    let name_token = field.token(&TokenKind::Ident)?;
                    let name = self.name(name_token);
                    let pattern = match field.child_nodes().find(|node| node.kind().is_pattern()) {
                        Some(pattern) => self.pattern(pattern, true),
                        None => Pattern {
                            span: name_token.span(),
                            kind: self.bind_at(name, name_token.span()),
                        },
                    };
                    Some(NamedPatternField {
                        name,
                        span: Span::new(field.span().start, pattern.span.end),
                        pattern,
                    })
                })
                .collect();
            NominalPatternFields::Named { fields, rest }
        } else {
            NominalPatternFields::Tuple(Vec::new())
        };
        PatternKind::Nominal {
            constructor,
            fields,
        }
    }
}
