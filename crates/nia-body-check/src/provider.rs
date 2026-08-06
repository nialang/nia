// SPDX-License-Identifier: GPL-3.0-or-later
use std::sync::atomic::{AtomicU64, Ordering};

use nia_source::SourcePath;
use nia_symbol::SymbolId;

static NEXT_PROVIDER_FACT_OWNER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderDemand {
    pub source_path: SourcePath,
    pub request: ProviderRequest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProviderFactRevision {
    owner: u64,
    index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFactRevisionTransition {
    Unchanged,
    Advanced,
    Replaced,
    Stale,
}

impl ProviderFactRevision {
    pub fn new_store() -> Self {
        let owner = NEXT_PROVIDER_FACT_OWNER
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |owner| {
                owner.checked_add(1)
            })
            .expect("provider fact owner space exhausted");
        Self { owner, index: 0 }
    }

    pub fn next(self) -> Self {
        Self {
            owner: self.owner,
            index: self
                .index
                .checked_add(1)
                .expect("provider fact revision overflow"),
        }
    }

    pub fn is_newer_than(self, previous: Self) -> bool {
        matches!(
            self.transition_from(previous),
            ProviderFactRevisionTransition::Advanced | ProviderFactRevisionTransition::Replaced
        )
    }

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
pub enum ProviderRequest {
    Method {
        target_type_name: Option<SymbolId>,
        method_name: SymbolId,
    },
    TraitImpl {
        target_type_name: Option<SymbolId>,
        trait_name: SymbolId,
    },
    ModuleSemantic {
        module_path: SourcePath,
    },
    ModuleBody {
        module_path: SourcePath,
    },
}

impl ProviderRequest {
    pub fn invalidates_resolved_body_facts(&self) -> bool {
        matches!(self, Self::Method { .. } | Self::TraitImpl { .. })
    }
}
