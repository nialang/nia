// SPDX-License-Identifier: GPL-3.0-or-later
use std::sync::atomic::{AtomicU64, Ordering};

use nia_source::SourcePath;
use nia_symbol::SymbolId;

static NEXT_PROVIDER_FACT_OWNER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// One cross-module fact requested while checking a body.
pub struct ProviderDemand {
    /// Module/source owner of the request.
    pub source_path: SourcePath,
    /// Specific semantic or body fact requested.
    pub request: ProviderRequest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Monotonic revision token for provider facts.
pub struct ProviderFactRevision {
    owner: u64,
    index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Relationship between two provider fact revisions.
pub enum ProviderFactRevisionTransition {
    /// Tokens identify the same fact revision.
    Unchanged,
    /// A later revision in the same owner store.
    Advanced,
    /// A token from a different owner store.
    Replaced,
    /// An older revision in the same owner store.
    Stale,
}

impl ProviderFactRevision {
    /// Allocates the initial token for a new provider fact store.
    pub fn new_store() -> Self {
        let owner = NEXT_PROVIDER_FACT_OWNER
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |owner| {
                owner.checked_add(1)
            })
            .expect("provider fact owner space exhausted");
        Self { owner, index: 0 }
    }

    /// Advances this token within its owning store.
    pub fn next(self) -> Self {
        Self {
            owner: self.owner,
            index: self
                .index
                .checked_add(1)
                .expect("provider fact revision overflow"),
        }
    }

    /// Returns whether this token supersedes `previous`.
    pub fn is_newer_than(self, previous: Self) -> bool {
        matches!(
            self.transition_from(previous),
            ProviderFactRevisionTransition::Advanced | ProviderFactRevisionTransition::Replaced
        )
    }

    /// Classifies replacement, advancement, equality, or staleness.
    pub fn transition_from(self, previous: Self) -> ProviderFactRevisionTransition {
        if self.owner != previous.owner {
            return ProviderFactRevisionTransition::Replaced;
        }
        match self.index.cmp(&previous.index) {
            std::cmp::Ordering::Less => ProviderFactRevisionTransition::Stale,
            std::cmp::Ordering::Equal => ProviderFactRevisionTransition::Unchanged,
            std::cmp::Ordering::Greater => ProviderFactRevisionTransition::Advanced,
        }
    }

    #[doc(hidden)]
    pub const fn fingerprint_parts(self) -> [u64; 2] {
        [self.owner, self.index]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Kind of cross-module provider fact requested by body checking.
pub enum ProviderRequest {
    /// Method lookup/semantic facts.
    Method {
        /// Optional target type display identity.
        target_type_name: Option<SymbolId>,
        /// Method display identity.
        method_name: SymbolId,
    },
    /// Trait implementation lookup facts.
    TraitImpl {
        /// Optional target type display identity.
        target_type_name: Option<SymbolId>,
        /// Trait display identity.
        trait_name: SymbolId,
    },
    /// Module-level semantic facts.
    ModuleSemantic {
        /// Requested module path.
        module_path: SourcePath,
    },
    /// Module-level body facts.
    ModuleBody {
        /// Requested module path.
        module_path: SourcePath,
    },
}

impl ProviderRequest {
    /// Returns whether a request invalidates resolved body facts.
    pub fn invalidates_resolved_body_facts(&self) -> bool {
        matches!(self, Self::Method { .. } | Self::TraitImpl { .. })
    }
}
