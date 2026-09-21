// SPDX-License-Identifier: GPL-3.0-or-later
use std::sync::Arc;

use nia_diagnostic::{Diagnostic, Severity};
use nia_source::SourcePath;

/// Diagnostic paired with its stable source path.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramDiagnostic {
    pub path: SourcePath,
    pub diagnostic: Diagnostic,
}

impl ProgramDiagnostic {
    pub fn is_error(&self) -> bool {
        self.diagnostic.severity == Severity::Error
    }
    pub fn is_warning(&self) -> bool {
        self.diagnostic.severity == Severity::Warning
    }
}

/// Store-owned diagnostic bundles grouped by source without flattening eagerly.
#[derive(Clone)]
pub struct ProgramDiagnosticBundles {
    store: Arc<nia_diagnostic::DiagnosticStore>,
    bundles: Arc<[SourceDiagnosticBundle]>,
}

#[derive(Debug, Clone, PartialEq)]
struct SourceDiagnosticBundle {
    path: SourcePath,
    diagnostics: nia_diagnostic::DiagnosticBundle,
}

impl ProgramDiagnosticBundles {
    pub fn from_diagnostics(diagnostics: Vec<ProgramDiagnostic>) -> nia_ice::IceResult<Self> {
        let store = Arc::new(nia_diagnostic::DiagnosticStore::new()?);
        Self::from_diagnostics_in(store, diagnostics)
    }

    pub fn from_diagnostics_in(
        store: Arc<nia_diagnostic::DiagnosticStore>,
        diagnostics: Vec<ProgramDiagnostic>,
    ) -> nia_ice::IceResult<Self> {
        let mut diagnostics = diagnostics.into_iter().peekable();
        let mut bundles = Vec::new();
        while let Some(ProgramDiagnostic { path, diagnostic }) = diagnostics.next() {
            let mut source_diagnostics = vec![diagnostic];
            while let Some(next) = diagnostics.next_if(|next| next.path == path) {
                source_diagnostics.push(next.diagnostic);
            }
            bundles.push(SourceDiagnosticBundle {
                path,
                diagnostics: store.bundle(source_diagnostics)?,
            });
        }
        Ok(Self {
            store,
            bundles: bundles.into(),
        })
    }

    pub fn from_source_bundle(
        store: Arc<nia_diagnostic::DiagnosticStore>,
        path: SourcePath,
        diagnostics: nia_diagnostic::DiagnosticBundle,
    ) -> nia_ice::IceResult<Self> {
        if store.diagnostics(&diagnostics).is_none() {
            return Err(nia_ice::Ice::new(
                "program diagnostic bundle has a foreign store owner",
            ));
        }
        let bundles = if diagnostics.is_empty() {
            Vec::new()
        } else {
            vec![SourceDiagnosticBundle { path, diagnostics }]
        };
        Ok(Self {
            store,
            bundles: bundles.into(),
        })
    }

    pub fn append(&self, other: &Self) -> nia_ice::IceResult<Self> {
        if !Arc::ptr_eq(&self.store, &other.store) {
            return Err(nia_ice::Ice::new(
                "cannot append program diagnostics from different stores",
            ));
        }
        Ok(Self {
            store: self.store.clone(),
            bundles: self
                .bundles
                .iter()
                .chain(other.bundles.iter())
                .cloned()
                .collect::<Vec<_>>()
                .into(),
        })
    }

    pub fn to_diagnostics(&self) -> Vec<ProgramDiagnostic> {
        self.bundles
            .iter()
            .flat_map(|bundle| {
                bundle
                    .diagnostics
                    .diagnostics()
                    .iter()
                    .cloned()
                    .map(|diagnostic| ProgramDiagnostic {
                        path: bundle.path.clone(),
                        diagnostic,
                    })
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.bundles.is_empty()
    }

    /// Returns the store-owned bundle handles in publication order.
    #[doc(hidden)]
    pub fn bundle_ids(&self) -> impl Iterator<Item = nia_diagnostic::DiagnosticBundleId> + '_ {
        self.bundles.iter().map(|bundle| bundle.diagnostics.id())
    }
}

impl std::fmt::Debug for ProgramDiagnosticBundles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ProgramDiagnosticBundles")
            .field(&self.to_diagnostics())
            .finish()
    }
}

impl PartialEq for ProgramDiagnosticBundles {
    fn eq(&self, other: &Self) -> bool {
        self.to_diagnostics() == other.to_diagnostics()
    }
}
