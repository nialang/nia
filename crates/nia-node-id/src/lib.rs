// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{HashMap, hash_map},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeStoreId(u32);

impl NodeStoreId {
    fn fresh() -> Self {
        static NEXT_NODE_STORE_ID: AtomicU32 = AtomicU32::new(1);
        let id = NEXT_NODE_STORE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("node store identity space exhausted");
        Self(id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeIndex(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    store_id: NodeStoreId,
    index: NodeIndex,
}

#[derive(Debug, Clone)]
pub struct NodeStore {
    id: NodeStoreId,
    core: Arc<Mutex<NodeStoreCore>>,
}

#[derive(Debug, Default)]
struct NodeStoreCore {
    by_locator: HashMap<VersionedNodeKey, NodeIndex>,
    locators: Vec<VersionedNodeKey>,
}

#[derive(Debug, Clone)]
pub struct NodeStoreAppend {
    store: NodeStore,
}

impl Default for NodeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeStore {
    pub fn new() -> Self {
        Self {
            id: NodeStoreId::fresh(),
            core: Arc::new(Mutex::new(NodeStoreCore::default())),
        }
    }

    pub fn append(&self) -> NodeStoreAppend {
        NodeStoreAppend {
            store: self.clone(),
        }
    }

    pub fn id(&self) -> NodeStoreId {
        self.id
    }

    pub fn locator(&self, node_id: NodeId) -> Option<VersionedNodeKey> {
        if node_id.store_id != self.id {
            return None;
        }
        self.core
            .lock()
            .expect("node store lock poisoned")
            .locators
            .get(node_id.index.0 as usize)
            .cloned()
    }

    pub fn id_for_locator(&self, locator: &VersionedNodeKey) -> Option<NodeId> {
        self.core
            .lock()
            .expect("node store lock poisoned")
            .by_locator
            .get(locator)
            .copied()
            .map(|index| NodeId {
                store_id: self.id,
                index,
            })
    }

    pub fn len(&self) -> usize {
        self.core
            .lock()
            .expect("node store lock poisoned")
            .locators
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl NodeStoreAppend {
    pub fn intern(&self, locator: VersionedNodeKey) -> NodeId {
        let mut core = self.store.core.lock().expect("node store lock poisoned");
        if let Some(index) = core.by_locator.get(&locator).copied() {
            return NodeId {
                store_id: self.store.id,
                index,
            };
        }
        let index =
            NodeIndex(u32::try_from(core.locators.len()).expect("node identity space exhausted"));
        core.locators.push(locator.clone());
        core.by_locator.insert(locator, index);
        NodeId {
            store_id: self.store.id,
            index,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeMap<V> {
    store: NodeStore,
    nodes: HashMap<NodeId, V>,
}

#[derive(Debug)]
pub struct NodeMapBuilder<V> {
    store: NodeStore,
    append: NodeStoreAppend,
    nodes: HashMap<NodeId, V>,
}

pub struct NodeMapIter<'a, V> {
    store: &'a NodeStore,
    entries: hash_map::Iter<'a, NodeId, V>,
}

impl<V> Default for NodeMap<V> {
    fn default() -> Self {
        Self::with_store(&NodeStore::new())
    }
}

impl<V: PartialEq> PartialEq for NodeMap<V> {
    fn eq(&self, other: &Self) -> bool {
        if self.store.id == other.store.id {
            return self.nodes == other.nodes;
        }
        self.nodes.len() == other.nodes.len()
            && self.nodes.iter().all(|(node_id, value)| {
                self.store.locator(*node_id).is_some_and(|locator| {
                    other
                        .store
                        .id_for_locator(&locator)
                        .and_then(|other_id| other.nodes.get(&other_id))
                        == Some(value)
                })
            })
    }
}

impl<V: Eq> Eq for NodeMap<V> {}

impl<V> NodeMap<V> {
    pub fn with_store(store: &NodeStore) -> Self {
        Self {
            store: store.clone(),
            nodes: HashMap::new(),
        }
    }

    pub fn builder(store: &NodeStore) -> NodeMapBuilder<V> {
        NodeMapBuilder {
            store: store.clone(),
            append: store.append(),
            nodes: HashMap::new(),
        }
    }

    pub fn get(&self, locator: &VersionedNodeKey) -> Option<&V> {
        self.node_id(locator)
            .and_then(|node_id| self.nodes.get(&node_id))
    }

    pub fn get_by_id(&self, node_id: NodeId) -> Option<&V> {
        self.nodes.get(&node_id)
    }

    pub fn node_id(&self, locator: &VersionedNodeKey) -> Option<NodeId> {
        self.store
            .id_for_locator(locator)
            .filter(|node_id| self.nodes.contains_key(node_id))
    }

    pub fn store_id(&self) -> NodeStoreId {
        self.store.id()
    }

    pub fn node_store(&self) -> &NodeStore {
        &self.store
    }

    pub fn iter(&self) -> NodeMapIter<'_, V> {
        NodeMapIter {
            store: &self.store,
            entries: self.nodes.iter(),
        }
    }

    pub fn values(&self) -> hash_map::Values<'_, NodeId, V> {
        self.nodes.values()
    }

    pub fn keys(&self) -> impl Iterator<Item = VersionedNodeKey> + '_ {
        self.nodes.keys().map(|node_id| {
            self.store
                .locator(*node_id)
                .expect("node map id belongs to its node store")
        })
    }

    pub fn into_entries(self) -> impl Iterator<Item = (VersionedNodeKey, V)> {
        let Self { store, nodes } = self;
        nodes.into_iter().map(move |(node_id, value)| {
            (
                store
                    .locator(node_id)
                    .expect("node map id belongs to its node store"),
                value,
            )
        })
    }

    pub fn into_builder(self) -> NodeMapBuilder<V> {
        let append = self.store.append();
        NodeMapBuilder {
            store: self.store,
            append,
            nodes: self.nodes,
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

impl<V> NodeMapBuilder<V> {
    pub fn insert(&mut self, locator: VersionedNodeKey, value: V) -> Option<V> {
        self.nodes.insert(self.append.intern(locator), value)
    }

    pub fn insert_if_absent(&mut self, locator: VersionedNodeKey, value: V) {
        self.nodes
            .entry(self.append.intern(locator))
            .or_insert(value);
    }

    pub fn extend(&mut self, entries: impl IntoIterator<Item = (VersionedNodeKey, V)>) {
        for (locator, value) in entries {
            self.insert(locator, value);
        }
    }

    pub fn extend_map(&mut self, nodes: NodeMap<V>) {
        if self.store.id == nodes.store.id {
            self.nodes.extend(nodes.nodes);
        } else {
            self.extend(nodes.into_entries());
        }
    }

    pub fn finish(self) -> NodeMap<V> {
        NodeMap {
            store: self.store,
            nodes: self.nodes,
        }
    }
}

impl<'a, V> Iterator for NodeMapIter<'a, V> {
    type Item = (VersionedNodeKey, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        self.entries.next().map(|(node_id, value)| {
            (
                self.store
                    .locator(*node_id)
                    .expect("node map id belongs to its node store"),
                value,
            )
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.entries.size_hint()
    }
}

impl<V> ExactSizeIterator for NodeMapIter<'_, V> {}

impl<'a, V> IntoIterator for &'a NodeMap<V> {
    type Item = (VersionedNodeKey, &'a V);
    type IntoIter = NodeMapIter<'a, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Debug, Clone)]
pub struct NodeOriginTable {
    store: NodeStore,
    nodes: HashMap<(SyntaxKind, Span), NodeId>,
}

#[derive(Debug)]
pub struct NodeOriginTableBuilder {
    store: NodeStore,
    append: NodeStoreAppend,
    nodes: HashMap<(SyntaxKind, Span), NodeId>,
}

impl Default for NodeOriginTable {
    fn default() -> Self {
        Self::with_store(&NodeStore::new())
    }
}

impl PartialEq for NodeOriginTable {
    fn eq(&self, other: &Self) -> bool {
        if self.store.id == other.store.id {
            return self.nodes == other.nodes;
        }
        self.nodes.len() == other.nodes.len()
            && self.nodes.iter().all(|(origin, node_id)| {
                other.nodes.get(origin).is_some_and(|other_id| {
                    self.store.locator(*node_id) == other.store.locator(*other_id)
                })
            })
    }
}

impl Eq for NodeOriginTable {}

impl NodeOriginTable {
    pub fn with_store(store: &NodeStore) -> Self {
        Self {
            store: store.clone(),
            nodes: HashMap::new(),
        }
    }

    pub fn builder(store: &NodeStore) -> NodeOriginTableBuilder {
        NodeOriginTableBuilder {
            store: store.clone(),
            append: store.append(),
            nodes: HashMap::new(),
        }
    }

    pub fn node_id(&self, kind: SyntaxKind, span: Span) -> Option<NodeId> {
        self.nodes.get(&(kind, span)).copied()
    }

    pub fn locator(&self, kind: SyntaxKind, span: Span) -> Option<VersionedNodeKey> {
        self.node_id(kind, span)
            .and_then(|node_id| self.store.locator(node_id))
    }

    pub fn store_id(&self) -> NodeStoreId {
        self.store.id()
    }

    pub fn node_store(&self) -> &NodeStore {
        &self.store
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
}

impl NodeOriginTableBuilder {
    pub fn insert(&mut self, kind: SyntaxKind, span: Span, locator: VersionedNodeKey) -> NodeId {
        let node_id = self.append.intern(locator);
        self.nodes.insert((kind, span), node_id);
        node_id
    }

    pub fn finish(self) -> NodeOriginTable {
        NodeOriginTable {
            store: self.store,
            nodes: self.nodes,
        }
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
        let store = NodeStore::new();
        let mut origins = NodeOriginTable::builder(&store);

        let node_id = origins.insert(SyntaxKind::Expr, span, key.clone());
        let origins = origins.finish();

        assert_eq!(origins.node_id(SyntaxKind::Expr, span), Some(node_id));
        assert_eq!(origins.locator(SyntaxKind::Expr, span), Some(key));
        assert_eq!(origins.store_id(), store.id());
    }

    #[test]
    fn origin_tables_compare_by_locator_across_stores() {
        let version = SourceVersion {
            id: SourceId(2),
            revision: SourceRevision(1),
        };
        let span = Span::new(4, 9);
        let locator = VersionedNodeKey::span(version, SyntaxKind::Expr, span);
        let first_store = NodeStore::new();
        let second_store = NodeStore::new();
        let mut first = NodeOriginTable::builder(&first_store);
        let mut second = NodeOriginTable::builder(&second_store);

        first.insert(SyntaxKind::Expr, span, locator.clone());
        second.insert(SyntaxKind::Expr, span, locator);
        let first = first.finish();
        let second = second.finish();

        assert_ne!(first.store_id(), second.store_id());
        assert_eq!(first, second);
        assert_eq!(NodeOriginTable::default(), NodeOriginTable::default());
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

    #[test]
    fn node_store_interns_word_sized_session_handles() {
        assert_eq!(std::mem::size_of::<NodeId>(), 8);
        let store = NodeStore::new();
        let append = store.append();
        let locator = VersionedNodeKey::span(
            SourceVersion {
                id: SourceId(4),
                revision: SourceRevision(2),
            },
            SyntaxKind::Expr,
            Span::new(3, 7),
        );

        let first = append.intern(locator.clone());
        let second = append.intern(locator.clone());

        assert_eq!(first, second);
        assert_eq!(store.locator(first), Some(locator));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn node_store_keeps_revisions_distinct_and_old_slots_stable() {
        let store = NodeStore::new();
        let append = store.append();
        let first_locator = VersionedNodeKey::span(
            SourceVersion {
                id: SourceId(1),
                revision: SourceRevision::INITIAL,
            },
            SyntaxKind::Type,
            Span::new(2, 5),
        );
        let second_locator = VersionedNodeKey::span(
            SourceVersion {
                id: SourceId(1),
                revision: SourceRevision(1),
            },
            SyntaxKind::Type,
            Span::new(2, 5),
        );

        let first = append.intern(first_locator.clone());
        let second = append.intern(second_locator.clone());

        assert_ne!(first, second);
        assert_eq!(store.locator(first), Some(first_locator));
        assert_eq!(store.locator(second), Some(second_locator));
    }

    #[test]
    fn node_map_round_trips_locators_through_owner_handles() {
        let store = NodeStore::new();
        let locator = VersionedNodeKey::span(
            SourceVersion {
                id: SourceId(3),
                revision: SourceRevision(2),
            },
            SyntaxKind::Expr,
            Span::new(4, 8),
        );
        let mut builder = NodeMap::builder(&store);
        builder.insert(locator.clone(), 17);
        let nodes = builder.finish();
        let node_id = nodes.node_id(&locator).expect("interned node handle");

        assert_eq!(nodes.store_id(), store.id());
        assert_eq!(nodes.get(&locator), Some(&17));
        assert_eq!(nodes.get_by_id(node_id), Some(&17));
        assert_eq!(nodes.iter().collect::<Vec<_>>(), vec![(locator, &17)]);
    }

    #[test]
    fn node_maps_compare_by_locator_across_owners() {
        let locator = VersionedNodeKey::span(
            SourceVersion {
                id: SourceId(3),
                revision: SourceRevision(2),
            },
            SyntaxKind::Type,
            Span::new(1, 5),
        );
        let first_store = NodeStore::new();
        let second_store = NodeStore::new();
        let mut first = NodeMap::builder(&first_store);
        let mut second = NodeMap::builder(&second_store);
        first.insert(locator.clone(), 23);
        second.insert(locator, 23);

        assert_ne!(first_store.id(), second_store.id());
        assert_eq!(first.finish(), second.finish());
    }

    #[test]
    fn node_map_builder_merges_handles_and_rehomes_foreign_maps() {
        let first_store = NodeStore::new();
        let second_store = NodeStore::new();
        let first_locator = VersionedNodeKey::span(
            SourceVersion {
                id: SourceId(7),
                revision: SourceRevision::INITIAL,
            },
            SyntaxKind::Expr,
            Span::new(0, 1),
        );
        let second_locator = VersionedNodeKey::span(
            SourceVersion {
                id: SourceId(7),
                revision: SourceRevision::INITIAL,
            },
            SyntaxKind::Expr,
            Span::new(2, 3),
        );
        let mut first = NodeMap::builder(&first_store);
        first.insert(first_locator.clone(), 1);
        let first = first.finish();
        let first_id = first.node_id(&first_locator).expect("first node handle");
        let mut second = NodeMap::builder(&second_store);
        second.insert(second_locator.clone(), 2);

        let mut merged = first.into_builder();
        merged.extend_map(second.finish());
        let merged = merged.finish();

        assert_eq!(merged.store_id(), first_store.id());
        assert_eq!(merged.node_id(&first_locator), Some(first_id));
        assert_eq!(merged.get(&second_locator), Some(&2));
        assert_eq!(
            merged
                .node_id(&second_locator)
                .expect("re-homed node handle")
                .store_id,
            first_store.id()
        );
    }

    #[test]
    fn node_map_rejects_foreign_handles() {
        let first_store = NodeStore::new();
        let second_store = NodeStore::new();
        let locator = VersionedNodeKey::span(
            SourceVersion {
                id: SourceId(0),
                revision: SourceRevision::INITIAL,
            },
            SyntaxKind::Stmt,
            Span::new(0, 1),
        );
        let foreign_id = second_store.append().intern(locator.clone());
        let mut builder = NodeMap::builder(&first_store);
        builder.insert(locator, 5);

        assert_eq!(builder.finish().get_by_id(foreign_id), None);
    }

    #[test]
    fn node_store_rejects_foreign_handles() {
        let first = NodeStore::new();
        let second = NodeStore::new();
        let locator = VersionedNodeKey::span(
            SourceVersion {
                id: SourceId(2),
                revision: SourceRevision::INITIAL,
            },
            SyntaxKind::Item,
            Span::new(0, 4),
        );

        let first_id = first.append().intern(locator.clone());
        let second_id = second.append().intern(locator);

        assert_ne!(first_id, second_id);
        assert_eq!(first_id.index, second_id.index);
        assert_ne!(first_id.store_id, second_id.store_id);
        assert_eq!(second.locator(first_id), None);
    }
}
