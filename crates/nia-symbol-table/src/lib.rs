// SPDX-License-Identifier: GPL-3.0-or-later
//! Concurrent text storage for Nia's stable content-addressed symbols.
//!
//! [`SymbolId`] is derived from the stable hash of its text, so independently
//! created tables agree on identity. A table retains the reverse mapping used
//! by diagnostics and persistence decoding, and rejects the otherwise-unsound
//! case where distinct strings produce the same ID. Both [`SymbolTable::new`]
//! and [`Default`] install every [`known`] symbol.

use std::{
    collections::{HashMap, hash_map::Entry},
    fmt,
    sync::{Arc, RwLock},
};

pub use nia_symbol::{
    SymbolId, SymbolMap, SymbolSet, SymbolText, known, stable_hash, unresolved_symbol_text,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolCollision {
    pub symbol: SymbolId,
    pub existing: Arc<str>,
    pub incoming: Arc<str>,
}

impl fmt::Display for SymbolCollision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "symbol collision for {:#018x}: `{}` and `{}`",
            self.symbol.raw(),
            self.existing,
            self.incoming
        )
    }
}

impl std::error::Error for SymbolCollision {}

#[derive(Debug, Clone)]
/// A shareable symbol text registry for one compiler session.
///
/// Clones share the same registry. Equality intentionally tests that shared
/// ownership, rather than comparing accumulated text, because symbol tables
/// are mutable session services rather than immutable semantic products.
pub struct SymbolTable {
    inner: Arc<RwLock<SymbolTableInner>>,
}

#[derive(Debug, Default)]
struct SymbolTableInner {
    by_id: HashMap<SymbolId, Arc<str>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        let table = Self {
            inner: Arc::new(RwLock::new(SymbolTableInner::default())),
        };
        table.install_known_symbols();
        table
    }

    pub fn intern(&self, text: &str) -> Result<SymbolId, SymbolCollision> {
        let symbol = SymbolId::from_stable_hash(stable_hash(text));
        let incoming = Arc::<str>::from(text);
        let mut inner = self.inner.write().expect("symbol table lock poisoned");
        match inner.by_id.entry(symbol) {
            Entry::Occupied(entry) if entry.get().as_ref() != text => Err(SymbolCollision {
                symbol,
                existing: entry.get().clone(),
                incoming,
            }),
            Entry::Occupied(_) => Ok(symbol),
            Entry::Vacant(entry) => {
                entry.insert(incoming);
                Ok(symbol)
            }
        }
    }

    pub fn resolve(&self, symbol: SymbolId) -> Option<Arc<str>> {
        self.inner
            .read()
            .expect("symbol table lock poisoned")
            .by_id
            .get(&symbol)
            .cloned()
    }

    pub fn resolver(&self) -> SymbolResolver {
        SymbolResolver {
            table: self.clone(),
        }
    }

    fn install_known_symbols(&self) {
        let mut inner = self.inner.write().expect("symbol table lock poisoned");
        for (symbol, text) in known::WELL_KNOWN {
            inner
                .by_id
                .entry(*symbol)
                .or_insert_with(|| Arc::<str>::from(*text));
        }
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for SymbolTable {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for SymbolTable {}

impl SymbolText for SymbolTable {
    fn symbol_text(&self, symbol: SymbolId) -> Option<Arc<str>> {
        self.resolve(symbol)
    }
}

#[derive(Debug, Clone)]
pub struct SymbolResolver {
    table: SymbolTable,
}

impl SymbolResolver {
    pub fn resolve(&self, symbol: SymbolId) -> Option<Arc<str>> {
        self.table.resolve(symbol)
    }

    pub fn display(&self, symbol: SymbolId) -> ResolvedSymbolDisplay<'_> {
        ResolvedSymbolDisplay {
            resolver: self,
            symbol,
        }
    }
}

pub struct ResolvedSymbolDisplay<'a> {
    resolver: &'a SymbolResolver,
    symbol: SymbolId,
}

impl fmt::Display for ResolvedSymbolDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.resolver.resolve(self.symbol) {
            Some(text) => f.write_str(&text),
            None => f.write_str(&unresolved_symbol_text(self.symbol)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_returns_stable_content_id() {
        let table = SymbolTable::new();
        let first = table.intern("value").unwrap();
        let second = table.intern("value").unwrap();

        assert_eq!(first, second);
        assert_eq!(first, SymbolId::from_stable_hash(stable_hash("value")));
    }

    #[test]
    fn known_symbols_are_resolvable_without_interning() {
        let table = SymbolTable::new();

        assert_eq!(table.resolve(known::std()).as_deref(), Some("std"));
    }

    #[test]
    fn default_installs_the_same_well_known_symbol_registry() {
        let table = SymbolTable::default();
        let mut by_id = HashMap::new();

        for &(symbol, text) in known::WELL_KNOWN {
            assert_eq!(symbol, SymbolId::from_stable_hash(stable_hash(text)));
            assert_eq!(table.resolve(symbol).as_deref(), Some(text));
            assert_eq!(
                by_id.insert(symbol, text),
                None,
                "well-known symbols must not contain duplicate IDs"
            );
        }
    }
}
