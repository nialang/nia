// SPDX-License-Identifier: GPL-3.0-or-later
//! Session-local node handles and revision-stable syntax locators.
//!
//! [`NodeId`] is a compact handle owned by one [`NodeStore`], while
//! [`VersionedNodeKey`] carries the source revision and lossless syntax path
//! needed to remap semantic facts across compiler layers. [`NodeOriginTable`]
//! bridges AST `(kind, span)` lookups to those handles. Builders support
//! transactional origin-map rollback because parsers may intern several
//! candidates before accepting one ambiguous syntax interpretation.

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
/// Session-local allocator for compact node handles.
///
/// Indices are monotonic and never reused, so retiring a source revision makes
/// store-level lookups stale without allowing an old [`NodeId`] to alias a
/// node allocated later in the same store. Products such as [`NodeMap`] retain
/// their revision owner and can therefore outlive store-level retirement.
pub struct NodeStore {
    id: NodeStoreId,
    core: Arc<Mutex<NodeStoreCore>>,
    next_index: Arc<AtomicU32>,
}

#[derive(Debug, Default)]
struct NodeStoreCore {
    revisions: HashMap<SourceVersion, Arc<NodeRevision>>,
    active_indices: HashMap<NodeIndex, SourceVersion>,
}

#[derive(Debug)]
struct NodeRevision {
    version: SourceVersion,
    core: Mutex<NodeRevisionCore>,
}

#[derive(Debug, Default)]
struct NodeRevisionCore {
    by_locator: HashMap<Arc<VersionedNodeKey>, NodeIndex>,
    locators: HashMap<NodeIndex, Arc<VersionedNodeKey>>,
}

#[derive(Debug, Clone, Default)]
struct NodeRevisionSet {
    revisions: Vec<Arc<NodeRevision>>,
}

#[derive(Debug)]
struct NodeStoreAppend {
    store: NodeStore,
    revisions: NodeRevisionSet,
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
            next_index: Arc::new(AtomicU32::new(0)),
        }
    }

    fn append(&self) -> NodeStoreAppend {
        NodeStoreAppend {
            store: self.clone(),
            revisions: NodeRevisionSet::default(),
        }
    }

    pub fn id(&self) -> NodeStoreId {
        self.id
    }

    pub fn locator(&self, node_id: NodeId) -> Option<VersionedNodeKey> {
        if node_id.store_id != self.id {
            return None;
        }
        let revision = {
            let core = self.core.lock().expect("node store lock poisoned");
            core.active_indices
                .get(&node_id.index)
                .and_then(|version| core.revisions.get(version))
                .cloned()
        };
        revision.and_then(|revision| revision.locator(node_id.index))
    }

    pub fn id_for_locator(&self, locator: &VersionedNodeKey) -> Option<NodeId> {
        let revision = self
            .core
            .lock()
            .expect("node store lock poisoned")
            .revisions
            .get(&locator.source_version())
            .cloned();
        revision
            .and_then(|revision| revision.id_for_locator(locator))
            .map(|index| NodeId {
                store_id: self.id,
                index,
            })
    }

    pub fn len(&self) -> usize {
        let revisions = self
            .core
            .lock()
            .expect("node store lock poisoned")
            .revisions
            .values()
            .cloned()
            .collect::<Vec<_>>();
        revisions.into_iter().map(|revision| revision.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn active_revision_count(&self) -> usize {
        self.core
            .lock()
            .expect("node store lock poisoned")
            .revisions
            .len()
    }

    pub fn retire_revision(&self, version: SourceVersion) -> usize {
        let mut core = self.core.lock().expect("node store lock poisoned");
        let Some(revision) = core.revisions.remove(&version) else {
            return 0;
        };
        let indices = revision.indices();
        for index in &indices {
            core.active_indices.remove(index);
        }
        indices.len()
    }

    fn acquire_revision(&self, version: SourceVersion) -> Arc<NodeRevision> {
        let mut core = self.core.lock().expect("node store lock poisoned");
        Arc::clone(core.revisions.entry(version).or_insert_with(|| {
            Arc::new(NodeRevision {
                version,
                core: Mutex::new(NodeRevisionCore::default()),
            })
        }))
    }

    fn intern(&self, revision: &Arc<NodeRevision>, locator: VersionedNodeKey) -> NodeId {
        let index = revision.intern(&self.next_index, locator);
        let mut core = self.core.lock().expect("node store lock poisoned");
        if core
            .revisions
            .get(&revision.version)
            .is_some_and(|active| Arc::ptr_eq(active, revision))
        {
            core.active_indices.insert(index, revision.version);
        }
        NodeId {
            store_id: self.id,
            index,
        }
    }
}

impl NodeStoreAppend {
    fn intern(&mut self, locator: VersionedNodeKey) -> NodeId {
        let version = locator.source_version();
        let revision = self.revisions.revision(version).unwrap_or_else(|| {
            let revision = self.store.acquire_revision(version);
            self.revisions.insert(Arc::clone(&revision));
            revision
        });
        self.store.intern(&revision, locator)
    }

    fn id_for_locator(&self, locator: &VersionedNodeKey) -> Option<NodeId> {
        self.revisions.id_for_locator(locator).map(|index| NodeId {
            store_id: self.store.id,
            index,
        })
    }
}

impl NodeRevision {
    fn intern(&self, next_index: &AtomicU32, locator: VersionedNodeKey) -> NodeIndex {
        assert_eq!(
            locator.source_version(),
            self.version,
            "node locator revision must match its owner"
        );
        let mut core = self.core.lock().expect("node revision lock poisoned");
        if let Some(index) = core.by_locator.get(&locator).copied() {
            return index;
        }
        let index = NodeIndex(
            next_index
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |index| {
                    index.checked_add(1)
                })
                .expect("node identity space exhausted"),
        );
        let locator = Arc::new(locator);
        core.locators.insert(index, Arc::clone(&locator));
        core.by_locator.insert(locator, index);
        index
    }

    fn id_for_locator(&self, locator: &VersionedNodeKey) -> Option<NodeIndex> {
        self.core
            .lock()
            .expect("node revision lock poisoned")
            .by_locator
            .get(locator)
            .copied()
    }

    fn locator(&self, index: NodeIndex) -> Option<VersionedNodeKey> {
        self.core
            .lock()
            .expect("node revision lock poisoned")
            .locators
            .get(&index)
            .map(|locator| locator.as_ref().clone())
    }

    fn indices(&self) -> Vec<NodeIndex> {
        self.core
            .lock()
            .expect("node revision lock poisoned")
            .locators
            .keys()
            .copied()
            .collect()
    }

    fn len(&self) -> usize {
        self.core
            .lock()
            .expect("node revision lock poisoned")
            .locators
            .len()
    }
}

impl NodeRevisionSet {
    fn revision(&self, version: SourceVersion) -> Option<Arc<NodeRevision>> {
        self.revisions
            .iter()
            .find(|revision| revision.version == version)
            .cloned()
    }

    fn insert(&mut self, revision: Arc<NodeRevision>) {
        // A product chooses one generation for each source revision. A store
        // may reacquire the same SourceVersion after retirement, but mixing
        // both generations would give one VersionedNodeKey two NodeIds.
        if !self
            .revisions
            .iter()
            .any(|existing| existing.version == revision.version)
        {
            self.revisions.push(revision);
        }
    }

    fn extend(&mut self, revisions: Self) {
        for revision in revisions.revisions {
            self.insert(revision);
        }
    }

    fn id_for_locator(&self, locator: &VersionedNodeKey) -> Option<NodeIndex> {
        self.revisions
            .iter()
            .filter(|revision| revision.version == locator.source_version())
            .find_map(|revision| revision.id_for_locator(locator))
    }

    fn locator(&self, index: NodeIndex) -> Option<VersionedNodeKey> {
        self.revisions
            .iter()
            .find_map(|revision| revision.locator(index))
    }
}

#[derive(Debug, Clone)]
/// A locator-keyed semantic table backed by compact session-local handles.
///
/// Equality and merge behavior are defined by [`VersionedNodeKey`], not by the
/// backing [`NodeId`]. This distinction matters after a revision is retired:
/// an already-published map retains the old generation while later products
/// may intern the same locator under a new generation.
pub struct NodeMap<V> {
    store: NodeStore,
    revisions: NodeRevisionSet,
    nodes: HashMap<NodeId, V>,
}

#[derive(Debug)]
pub struct NodeMapBuilder<V> {
    append: NodeStoreAppend,
    nodes: HashMap<NodeId, V>,
}

pub struct NodeMapIter<'a, V> {
    revisions: &'a NodeRevisionSet,
    entries: hash_map::Iter<'a, NodeId, V>,
}

impl<V> Default for NodeMap<V> {
    fn default() -> Self {
        Self::with_store(&NodeStore::new())
    }
}

impl<V: PartialEq> PartialEq for NodeMap<V> {
    fn eq(&self, other: &Self) -> bool {
        self.nodes.len() == other.nodes.len()
            && self.nodes.iter().all(|(node_id, value)| {
                self.revisions
                    .locator(node_id.index)
                    .is_some_and(|locator| {
                        other
                            .revisions
                            .id_for_locator(&locator)
                            .map(|index| NodeId {
                                store_id: other.store.id,
                                index,
                            })
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
            revisions: NodeRevisionSet::default(),
            nodes: HashMap::new(),
        }
    }

    pub fn builder(store: &NodeStore) -> NodeMapBuilder<V> {
        NodeMapBuilder {
            append: store.append(),
            nodes: HashMap::new(),
        }
    }

    pub fn get(&self, locator: &VersionedNodeKey) -> Option<&V> {
        self.node_id(locator)
            .and_then(|node_id| self.nodes.get(&node_id))
    }

    pub fn contains_key(&self, locator: &VersionedNodeKey) -> bool {
        self.node_id(locator).is_some()
    }

    pub fn get_by_id(&self, node_id: NodeId) -> Option<&V> {
        self.nodes.get(&node_id)
    }

    pub fn node_id(&self, locator: &VersionedNodeKey) -> Option<NodeId> {
        self.revisions
            .id_for_locator(locator)
            .map(|index| NodeId {
                store_id: self.store.id,
                index,
            })
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
            revisions: &self.revisions,
            entries: self.nodes.iter(),
        }
    }

    pub fn values(&self) -> hash_map::Values<'_, NodeId, V> {
        self.nodes.values()
    }

    pub fn keys(&self) -> impl Iterator<Item = VersionedNodeKey> + '_ {
        self.nodes.keys().map(|node_id| {
            self.revisions
                .locator(node_id.index)
                .expect("node map id belongs to its node store")
        })
    }

    pub fn into_entries(self) -> impl Iterator<Item = (VersionedNodeKey, V)> {
        let Self {
            revisions, nodes, ..
        } = self;
        nodes.into_iter().map(move |(node_id, value)| {
            (
                revisions
                    .locator(node_id.index)
                    .expect("node map id belongs to its node store"),
                value,
            )
        })
    }

    pub fn into_builder(self) -> NodeMapBuilder<V> {
        NodeMapBuilder {
            append: NodeStoreAppend {
                store: self.store,
                revisions: self.revisions,
            },
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

    pub fn remove(&mut self, locator: &VersionedNodeKey) -> Option<V> {
        self.append
            .id_for_locator(locator)
            .and_then(|node_id| self.nodes.remove(&node_id))
    }

    pub fn extend(&mut self, entries: impl IntoIterator<Item = (VersionedNodeKey, V)>) {
        for (locator, value) in entries {
            self.insert(locator, value);
        }
    }

    pub fn extend_map(&mut self, nodes: NodeMap<V>) {
        if self.append.store.id == nodes.store.id {
            let NodeMap {
                revisions, nodes, ..
            } = nodes;
            // Retained and reacquired generations can coexist in the store.
            // Select the target's generation before interning source entries,
            // so a logical locator occurs once and source values still win.
            self.append.revisions.extend(revisions.clone());
            for (node_id, value) in nodes {
                let locator = revisions
                    .locator(node_id.index)
                    .expect("node map id belongs to its node store");
                self.nodes.insert(self.append.intern(locator), value);
            }
        } else {
            self.extend(nodes.into_entries());
        }
    }

    pub fn finish(self) -> NodeMap<V> {
        let NodeStoreAppend { store, revisions } = self.append;
        NodeMap {
            store,
            revisions,
            nodes: self.nodes,
        }
    }
}

impl<'a, V> Iterator for NodeMapIter<'a, V> {
    type Item = (VersionedNodeKey, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        self.entries.next().map(|(node_id, value)| {
            (
                self.revisions
                    .locator(node_id.index)
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
/// The accepted AST origin mapping for one parsed source product.
pub struct NodeOriginTable {
    store: NodeStore,
    revisions: NodeRevisionSet,
    nodes: HashMap<(SyntaxKind, Span), NodeId>,
}

#[derive(Debug)]
/// Incrementally builds an origin table against a shared node store.
pub struct NodeOriginTableBuilder {
    append: NodeStoreAppend,
    nodes: HashMap<(SyntaxKind, Span), NodeId>,
    changes: Vec<OriginChange>,
}

#[derive(Debug)]
struct OriginChange {
    origin: (SyntaxKind, Span),
    previous: Option<NodeId>,
}

impl Default for NodeOriginTable {
    fn default() -> Self {
        Self::with_store(&NodeStore::new())
    }
}

impl PartialEq for NodeOriginTable {
    fn eq(&self, other: &Self) -> bool {
        self.nodes.len() == other.nodes.len()
            && self.nodes.iter().all(|(origin, node_id)| {
                other.nodes.get(origin).is_some_and(|other_id| {
                    self.revisions.locator(node_id.index) == other.revisions.locator(other_id.index)
                })
            })
    }
}

impl Eq for NodeOriginTable {}

impl NodeOriginTable {
    pub fn with_store(store: &NodeStore) -> Self {
        Self {
            store: store.clone(),
            revisions: NodeRevisionSet::default(),
            nodes: HashMap::new(),
        }
    }

    pub fn builder(store: &NodeStore) -> NodeOriginTableBuilder {
        NodeOriginTableBuilder {
            append: store.append(),
            nodes: HashMap::new(),
            changes: Vec::new(),
        }
    }

    pub fn node_id(&self, kind: SyntaxKind, span: Span) -> Option<NodeId> {
        self.nodes.get(&(kind, span)).copied()
    }

    pub fn locator(&self, kind: SyntaxKind, span: Span) -> Option<VersionedNodeKey> {
        self.node_id(kind, span)
            .and_then(|node_id| self.revisions.locator(node_id.index))
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
    /// Marks the current origin map so speculative parser work can be undone.
    ///
    /// Interned node locators remain reusable in the shared [`NodeStore`], but
    /// the published origin map is transactional: a rollback removes entries
    /// that were created after this mark and restores overwritten entries.
    pub fn checkpoint(&self) -> usize {
        self.changes.len()
    }

    /// Rolls the origin map back to a prior [`Self::checkpoint`] mark.
    pub fn rollback(&mut self, checkpoint: usize) {
        assert!(
            checkpoint <= self.changes.len(),
            "origin checkpoint cannot be ahead of the current builder state"
        );
        while self.changes.len() > checkpoint {
            let change = self
                .changes
                .pop()
                .expect("origin change exists while rolling back");
            if let Some(previous) = change.previous {
                self.nodes.insert(change.origin, previous);
            } else {
                self.nodes.remove(&change.origin);
            }
        }
    }

    pub fn insert(&mut self, kind: SyntaxKind, span: Span, locator: VersionedNodeKey) -> NodeId {
        let node_id = self.append.intern(locator);
        let origin = (kind, span);
        let previous = self.nodes.insert(origin, node_id);
        self.changes.push(OriginChange { origin, previous });
        node_id
    }

    pub fn finish(self) -> NodeOriginTable {
        let NodeStoreAppend { store, revisions } = self.append;
        NodeOriginTable {
            store,
            revisions,
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
    fn origin_builder_rollback_restores_overwritten_entries() {
        let version = SourceVersion {
            id: SourceId(4),
            revision: SourceRevision(2),
        };
        let span = Span::new(1, 3);
        let original = VersionedNodeKey::span(version, SyntaxKind::Expr, span);
        let replacement = VersionedNodeKey::child_path(
            version,
            SyntaxKind::Expr,
            NodeChildPath::from_steps([0, 1]),
        );
        let store = NodeStore::new();
        let mut origins = NodeOriginTable::builder(&store);
        origins.insert(SyntaxKind::Expr, span, original.clone());
        let checkpoint = origins.checkpoint();
        origins.insert(
            SyntaxKind::Type,
            Span::new(4, 6),
            VersionedNodeKey::span(version, SyntaxKind::Type, Span::new(4, 6)),
        );
        origins.insert(SyntaxKind::Expr, span, replacement);

        origins.rollback(checkpoint);
        let origins = origins.finish();

        assert_eq!(origins.locator(SyntaxKind::Expr, span), Some(original));
        assert!(origins.locator(SyntaxKind::Type, Span::new(4, 6)).is_none());
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
        let mut append = store.append();
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
        let mut append = store.append();
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
    fn node_store_retires_revision_owner_without_invalidating_owned_maps() {
        let store = NodeStore::new();
        let first_version = SourceVersion {
            id: SourceId(1),
            revision: SourceRevision::INITIAL,
        };
        let second_version = SourceVersion {
            id: SourceId(1),
            revision: SourceRevision(1),
        };
        let first_locator =
            VersionedNodeKey::span(first_version, SyntaxKind::Expr, Span::new(2, 5));
        let second_locator =
            VersionedNodeKey::span(second_version, SyntaxKind::Expr, Span::new(2, 5));
        let mut first = NodeMap::builder(&store);
        first.insert(first_locator.clone(), 11);
        let first = first.finish();
        let first_id = first.node_id(&first_locator).expect("first revision id");
        let mut second = NodeMap::builder(&store);
        second.insert(second_locator.clone(), 22);
        let second = second.finish();
        let second_id = second.node_id(&second_locator).expect("second revision id");

        assert_eq!(store.active_revision_count(), 2);
        assert_eq!(store.len(), 2);
        assert_eq!(store.retire_revision(first_version), 1);
        assert_eq!(store.active_revision_count(), 1);
        assert_eq!(store.len(), 1);
        assert_eq!(store.locator(first_id), None);
        assert_eq!(store.id_for_locator(&first_locator), None);
        assert_eq!(store.locator(second_id), Some(second_locator.clone()));
        assert_eq!(first.get(&first_locator), Some(&11));
        assert_eq!(
            first.keys().collect::<Vec<_>>(),
            vec![first_locator.clone()]
        );
        assert_eq!(second.get(&second_locator), Some(&22));

        let mut replacement = NodeMap::builder(&store);
        replacement.insert(first_locator.clone(), 33);
        let replacement = replacement.finish();
        let replacement_id = replacement
            .node_id(&first_locator)
            .expect("replacement revision id");
        assert_ne!(replacement_id, first_id);
        assert_eq!(store.locator(first_id), None);
        assert_eq!(store.locator(replacement_id), Some(first_locator));
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
        assert!(nodes.contains_key(&locator));
        assert_eq!(nodes.get_by_id(node_id), Some(&17));
        assert_eq!(
            nodes.iter().collect::<Vec<_>>(),
            vec![(locator.clone(), &17)]
        );

        let mut builder = nodes.into_builder();
        assert_eq!(builder.remove(&locator), Some(17));
        assert!(builder.finish().is_empty());
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
    fn retained_and_reacquired_generations_compare_by_locator() {
        let version = SourceVersion {
            id: SourceId(9),
            revision: SourceRevision(3),
        };
        let span = Span::new(1, 4);
        let locator = VersionedNodeKey::span(version, SyntaxKind::Expr, span);
        let store = NodeStore::new();

        let mut old_map = NodeMap::builder(&store);
        old_map.insert(locator.clone(), 7);
        let old_map = old_map.finish();
        let mut old_origins = NodeOriginTable::builder(&store);
        let old_id = old_origins.insert(SyntaxKind::Expr, span, locator.clone());
        let old_origins = old_origins.finish();

        assert_eq!(store.retire_revision(version), 1);

        let mut new_map = NodeMap::builder(&store);
        new_map.insert(locator.clone(), 7);
        let new_map = new_map.finish();
        let mut new_origins = NodeOriginTable::builder(&store);
        let new_id = new_origins.insert(SyntaxKind::Expr, span, locator);
        let new_origins = new_origins.finish();

        assert_ne!(old_id, new_id);
        assert_eq!(old_map, new_map);
        assert_eq!(old_origins, new_origins);
    }

    #[test]
    fn same_store_merge_coalesces_reacquired_locator_generations() {
        let version = SourceVersion {
            id: SourceId(10),
            revision: SourceRevision(4),
        };
        let locator = VersionedNodeKey::span(version, SyntaxKind::Type, Span::new(2, 6));
        let store = NodeStore::new();
        let mut old = NodeMap::builder(&store);
        old.insert(locator.clone(), 1);
        let old = old.finish();

        assert_eq!(store.retire_revision(version), 1);
        let mut reacquired = NodeMap::builder(&store);
        reacquired.insert(locator.clone(), 2);

        let mut merged = old.into_builder();
        merged.extend_map(reacquired.finish());
        let merged = merged.finish();

        assert_eq!(merged.len(), 1);
        assert_eq!(merged.get(&locator), Some(&2));
        assert_eq!(merged.keys().collect::<Vec<_>>(), vec![locator]);
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
