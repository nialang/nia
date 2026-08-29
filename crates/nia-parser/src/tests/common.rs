// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use crate::parser::*;
pub(super) use nia_ast::{
    ExprKind, MatchArmBody, PathSegmentKind, Pattern, PatternKind, TypeArg, TypeKind, TypeRef,
};
pub(super) use nia_node_id::{NodePosition, NodeStore, SyntaxKind};
pub(super) use nia_source::{SourceId, SourceRevision, SourceVersion};
pub(super) use nia_symbol::{SymbolId, stable_hash};

pub(super) fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

pub(super) fn bind_pattern_name(pattern: &Pattern) -> Option<SymbolId> {
    match &pattern.kind {
        PatternKind::Bind { name, .. } => Some(*name),
        PatternKind::Pointer(inner) | PatternKind::MutPointer(inner) => bind_pattern_name(inner),
        _ => None,
    }
}

pub(super) fn host_name(segment: &nia_ast::UsingHostSegment) -> Option<SymbolId> {
    match segment.kind {
        PathSegmentKind::Name(name) => Some(name),
        PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
    }
}

pub(super) fn type_path_name(segment: &nia_ast::TypePathSegment) -> Option<SymbolId> {
    match segment.kind {
        PathSegmentKind::Name(name) => Some(name),
        PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
    }
}
