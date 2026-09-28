// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use crate::{LoweredModule, lower_module, parse_module};
pub(super) use nia_ast::*;
pub(super) use nia_node_id::{NodePosition, NodeStore, SyntaxKind};
pub(super) use nia_source::{SourceId, SourceRevision, SourceVersion};
pub(super) use nia_span::Span;
pub(super) use nia_symbol::{SymbolId, stable_hash};
pub(super) use nia_syntax::{ParseError, ParseErrorKind};

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

pub(super) fn version(revision: u64) -> SourceVersion {
    SourceVersion {
        id: SourceId::isolated(),
        revision: SourceRevision(revision),
    }
}

pub(super) fn lower_with(source: &str, version: Option<SourceVersion>) -> LoweredModule {
    let parse = nia_syntax::parse(source, version).expect("grammar tree");
    lower_module(
        &parse,
        &NodeStore::new(),
        nia_symbol_table::SymbolTable::new(),
    )
}
