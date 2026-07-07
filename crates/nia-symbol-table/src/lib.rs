// SPDX-License-Identifier: GPL-3.0-or-later
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

#[derive(Debug, Default, Clone)]
pub struct SymbolTable {
    inner: Arc<RwLock<SymbolTableInner>>,
}

#[derive(Debug, Default)]
struct SymbolTableInner {
    by_id: HashMap<SymbolId, Arc<str>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        let table = Self::default();
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
}
