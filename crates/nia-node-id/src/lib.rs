// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_source::{SourceId, SourceRevision, SourceVersion};
use nia_span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SyntaxKind {
    Module,
    Item,
    Stmt,
    Expr,
    Type,
    Pattern,
    Param,
    Syntax,
    Token,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeChildPath {
    steps: Vec<u32>,
}

impl NodeChildPath {
    pub fn root() -> Self {
        Self { steps: Vec::new() }
    }

    pub fn from_steps(steps: impl Into<Vec<u32>>) -> Self {
        Self {
            steps: steps.into(),
        }
    }

    pub fn steps(&self) -> &[u32] {
        &self.steps
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodePosition {
    Span(Span),
    ChildPath(NodeChildPath),
    ChildPathRange {
        start: NodeChildPath,
        end: NodeChildPath,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeSite {
    pub source_id: SourceId,
    pub kind: SyntaxKind,
    pub position: NodePosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VersionedNodeKey {
    pub site: NodeSite,
    pub revision: SourceRevision,
}

impl VersionedNodeKey {
    pub fn span(version: SourceVersion, kind: SyntaxKind, span: Span) -> Self {
        Self {
            site: NodeSite {
                source_id: version.id,
                kind,
                position: NodePosition::Span(span),
            },
            revision: version.revision,
        }
    }

    pub fn child_path(version: SourceVersion, kind: SyntaxKind, path: NodeChildPath) -> Self {
        Self {
            site: NodeSite {
                source_id: version.id,
                kind,
                position: NodePosition::ChildPath(path),
            },
            revision: version.revision,
        }
    }

    pub fn child_path_range(
        version: SourceVersion,
        kind: SyntaxKind,
        start: NodeChildPath,
        end: NodeChildPath,
    ) -> Self {
        Self {
            site: NodeSite {
                source_id: version.id,
                kind,
                position: NodePosition::ChildPathRange { start, end },
            },
            revision: version.revision,
        }
    }

    pub fn source_version(&self) -> SourceVersion {
        SourceVersion {
            id: self.site.source_id,
            revision: self.revision,
        }
    }

    pub fn site(&self) -> &NodeSite {
        &self.site
    }

    pub fn kind(&self) -> SyntaxKind {
        self.site.kind
    }

    pub fn position(&self) -> &NodePosition {
        &self.site.position
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeOriginTable {
    keys: HashMap<(SyntaxKind, Span), VersionedNodeKey>,
}

impl NodeOriginTable {
    pub fn insert(&mut self, kind: SyntaxKind, span: Span, key: VersionedNodeKey) {
        self.keys.insert((kind, span), key);
    }

    pub fn get(&self, kind: SyntaxKind, span: Span) -> Option<&VersionedNodeKey> {
        self.keys.get(&(kind, span))
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_source::{SourceId, SourceRevision};

    #[test]
    fn span_node_key_carries_source_version_kind_and_span() {
        let version = SourceVersion {
            id: SourceId(3),
            revision: SourceRevision(9),
        };
        let key = VersionedNodeKey::span(version, SyntaxKind::Expr, Span::new(4, 8));

        assert_eq!(key.source_version(), version);
        assert_eq!(key.kind(), SyntaxKind::Expr);
        assert_eq!(key.position(), &NodePosition::Span(Span::new(4, 8)));
    }

    #[test]
    fn child_path_position_is_available_for_future_green_trees() {
        let version = SourceVersion {
            id: SourceId(1),
            revision: SourceRevision::INITIAL,
        };
        let path = NodeChildPath::from_steps([0, 2, 1]);
        let key = VersionedNodeKey::child_path(version, SyntaxKind::Type, path.clone());

        assert_eq!(key.position(), &NodePosition::ChildPath(path));
    }

    #[test]
    fn child_path_range_position_can_key_lowered_ast_nodes() {
        let version = SourceVersion {
            id: SourceId(1),
            revision: SourceRevision::INITIAL,
        };
        let start = NodeChildPath::from_steps([0, 1]);
        let end = NodeChildPath::from_steps([0, 3]);
        let key = VersionedNodeKey::child_path_range(
            version,
            SyntaxKind::Expr,
            start.clone(),
            end.clone(),
        );

        assert_eq!(key.position(), &NodePosition::ChildPathRange { start, end });
    }

    #[test]
    fn origin_table_maps_kind_and_span_to_node_key() {
        let version = SourceVersion {
            id: SourceId(2),
            revision: SourceRevision(1),
        };
        let span = Span::new(4, 9);
        let key = VersionedNodeKey::span(version, SyntaxKind::Expr, span);
        let mut origins = NodeOriginTable::default();

        origins.insert(SyntaxKind::Expr, span, key.clone());

        assert_eq!(origins.get(SyntaxKind::Expr, span), Some(&key));
    }

    #[test]
    fn source_revision_is_part_of_node_identity() {
        let first = SourceVersion {
            id: SourceId(0),
            revision: SourceRevision::INITIAL,
        };
        let second = SourceVersion {
            id: SourceId(0),
            revision: SourceRevision(1),
        };

        assert_ne!(
            VersionedNodeKey::span(first, SyntaxKind::Expr, Span::new(0, 1)),
            VersionedNodeKey::span(second, SyntaxKind::Expr, Span::new(0, 1))
        );
    }

    #[test]
    fn node_site_keeps_source_position_identity_across_revisions() {
        let first = SourceVersion {
            id: SourceId(0),
            revision: SourceRevision::INITIAL,
        };
        let second = SourceVersion {
            id: SourceId(0),
            revision: SourceRevision(1),
        };

        let first_key = VersionedNodeKey::span(first, SyntaxKind::Type, Span::new(4, 9));
        let second_key = VersionedNodeKey::span(second, SyntaxKind::Type, Span::new(4, 9));

        assert_ne!(first_key, second_key);
        assert_eq!(first_key.site(), second_key.site());
    }
}
