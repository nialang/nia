// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use crate::parser::*;
pub(super) use nia_ast::{ExprKind, Pattern, PatternKind, SwitchArmBody};
pub(super) use nia_node_id::{NodePosition, SyntaxKind};
pub(super) use nia_source::{SourceId, SourceRevision, SourceVersion};

pub(super) fn bind_pattern_name(pattern: &Pattern) -> Option<&str> {
    match &pattern.kind {
        PatternKind::Bind { name, .. } => Some(name.as_str()),
        PatternKind::Pointer(inner) | PatternKind::MutPointer(inner) => bind_pattern_name(inner),
        _ => None,
    }
}
