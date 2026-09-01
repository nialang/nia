// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fmt,
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

use crate::Diagnostic;

static NEXT_STORE_ID: AtomicU32 = AtomicU32::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Store-qualified identity of a diagnostic bundle.
pub struct DiagnosticBundleId {
    store: NonZeroU32,
    index: u32,
}

#[derive(Clone)]
/// Immutable, shareable collection of diagnostics owned by one store.
pub struct DiagnosticBundle(Arc<DiagnosticBundleData>);

impl DiagnosticBundle {
    /// Returns the store-qualified bundle identity.
    pub fn id(&self) -> DiagnosticBundleId {
        self.0.id
    }
}

impl fmt::Debug for DiagnosticBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticBundle")
            .field("id", &self.id())
            .field("len", &self.0.diagnostics.as_slice().len())
            .finish()
    }
}

impl PartialEq for DiagnosticBundle {
    fn eq(&self, other: &Self) -> bool {
        self.0.diagnostics.as_slice() == other.0.diagnostics.as_slice()
    }
}

impl DiagnosticBundle {
    /// Reports whether this bundle contains no diagnostics.
    pub fn is_empty(&self) -> bool {
        self.0.diagnostics.as_slice().is_empty()
    }
}

#[derive(Debug)]
struct DiagnosticBundleData {
    id: DiagnosticBundleId,
    diagnostics: DiagnosticStorage,
}

#[derive(Debug)]
enum DiagnosticStorage {
    Owned(Box<[Diagnostic]>),
    Shared(Arc<Vec<Diagnostic>>),
}

impl DiagnosticStorage {
    fn as_slice(&self) -> &[Diagnostic] {
        match self {
            Self::Owned(diagnostics) => diagnostics,
            Self::Shared(diagnostics) => diagnostics,
        }
    }
}

#[derive(Debug)]
/// Session-owned allocator and accessor for diagnostic bundles.
pub struct DiagnosticStore {
    id: NonZeroU32,
    next_bundle_index: AtomicU32,
    empty: DiagnosticBundle,
}

impl DiagnosticStore {
    /// Creates an empty store with a fresh owner identity.
    pub fn new() -> Self {
        let id = NEXT_STORE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .ok()
            .and_then(NonZeroU32::new)
            .unwrap_or_else(|| panic!("Nia ICE: exhausted diagnostic store identities"));
        Self {
            id,
            next_bundle_index: AtomicU32::new(1),
            empty: DiagnosticBundle(Arc::new(DiagnosticBundleData {
                id: DiagnosticBundleId {
                    store: id,
                    index: 0,
                },
                diagnostics: DiagnosticStorage::Owned(Box::new([])),
            })),
        }
    }

    /// Publishes an owned diagnostic vector as an immutable bundle.
    pub fn bundle(&self, diagnostics: Vec<Diagnostic>) -> DiagnosticBundle {
        if diagnostics.is_empty() {
            return self.empty.clone();
        }
        let index = self
            .next_bundle_index
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .unwrap_or_else(|_| panic!("Nia ICE: exhausted diagnostic bundle identities"));
        DiagnosticBundle(Arc::new(DiagnosticBundleData {
            id: DiagnosticBundleId {
                store: self.id,
                index,
            },
            diagnostics: DiagnosticStorage::Owned(diagnostics.into_boxed_slice()),
        }))
    }

    /// Publishes a shared diagnostic vector without copying its payload.
    pub fn bundle_shared(&self, diagnostics: Arc<Vec<Diagnostic>>) -> DiagnosticBundle {
        if diagnostics.is_empty() {
            return self.empty.clone();
        }
        let index = self
            .next_bundle_index
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .unwrap_or_else(|_| panic!("Nia ICE: exhausted diagnostic bundle identities"));
        DiagnosticBundle(Arc::new(DiagnosticBundleData {
            id: DiagnosticBundleId {
                store: self.id,
                index,
            },
            diagnostics: DiagnosticStorage::Shared(diagnostics),
        }))
    }

    /// Borrows a bundle's diagnostics when it belongs to this store.
    pub fn diagnostics<'bundle>(
        &self,
        bundle: &'bundle DiagnosticBundle,
    ) -> Option<&'bundle [Diagnostic]> {
        (bundle.id().store == self.id).then(|| bundle.0.diagnostics.as_slice())
    }
}

impl Default for DiagnosticStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use nia_span::Span;

    use super::*;
    use crate::{Diagnostic, codes};

    #[test]
    fn bundle_handles_are_compact_and_owner_scoped() {
        assert_eq!(std::mem::size_of::<DiagnosticBundleId>(), 8);
        assert_eq!(
            std::mem::size_of::<DiagnosticBundle>(),
            std::mem::size_of::<usize>(),
        );

        let first = DiagnosticStore::new();
        let second = DiagnosticStore::new();
        let diagnostics = vec![Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            Span::new(2, 4),
            "bad type",
        )];
        let bundle = first.bundle(diagnostics.clone());

        assert_eq!(first.diagnostics(&bundle), Some(diagnostics.as_slice()));
        assert_eq!(second.diagnostics(&bundle), None);
    }

    #[test]
    fn empty_diagnostics_share_the_session_bundle() {
        let store = DiagnosticStore::new();
        let first = store.bundle(Vec::new());
        let second = store.bundle(Vec::new());

        assert_eq!(first.id(), second.id());
        assert_eq!(store.diagnostics(&first), Some([].as_slice()));
    }

    #[test]
    fn non_empty_payload_is_reclaimed_with_its_last_handle() {
        let store = DiagnosticStore::new();
        let bundle = store.bundle(vec![Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            Span::new(0, 1),
            "bad type",
        )]);
        let payload = Arc::downgrade(&bundle.0);

        drop(bundle);

        assert!(payload.upgrade().is_none());
    }

    #[test]
    fn shared_payload_keeps_its_existing_allocation() {
        let store = DiagnosticStore::new();
        let diagnostics = Arc::new(vec![Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            Span::new(0, 1),
            "bad type",
        )]);
        let bundle = store.bundle_shared(Arc::clone(&diagnostics));

        assert!(matches!(
            &bundle.0.diagnostics,
            DiagnosticStorage::Shared(shared) if Arc::ptr_eq(shared, &diagnostics)
        ));
    }
}
