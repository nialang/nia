// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable names for promoted allocations and function-IR locals.

use nia_symbol::{SymbolId, unresolved_symbol_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Identity of a source allocation promoted into static storage.
pub struct PromotedAllocationId {
    module_id: nia_ids::ModuleId,
    span: nia_span::Span,
}

impl PromotedAllocationId {
    /// Creates an allocation identity from its owning module and source span.
    pub const fn new(module_id: nia_ids::ModuleId, span: nia_span::Span) -> Self {
        Self { module_id, span }
    }

    /// Returns the owning module.
    pub const fn module_id(self) -> nia_ids::ModuleId {
        self.module_id
    }

    /// Returns the source span that introduced the allocation.
    pub const fn span(self) -> nia_span::Span {
        self.span
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Classification of a function-IR local name.
pub enum LocalName {
    /// The receiver binding.
    SelfValue,
    /// A source-level symbol.
    Named(SymbolId),
    /// A compiler-generated loop helper.
    Generated(GeneratedLocalName),
    /// An unnamed temporary identified by ordinal.
    Temporary(u32),
    /// A deliberately anonymous binding.
    Anonymous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Names reserved for compiler-generated loop helpers.
pub enum GeneratedLocalName {
    /// Storage for the iterable expression.
    ForIterable,
    /// Storage for the iterator value.
    ForIterator,
    /// Storage for the next iterator result.
    ForNext,
}

impl LocalName {
    /// Constructs a source-level named local.
    pub fn named(name: SymbolId) -> Self {
        Self::Named(name)
    }

    /// Constructs a compiler-generated local.
    pub fn generated(name: GeneratedLocalName) -> Self {
        Self::Generated(name)
    }

    /// Constructs a numbered temporary local.
    pub fn temporary(id: u32) -> Self {
        Self::Temporary(id)
    }

    /// Reports whether this is the receiver binding.
    pub fn is_self_value(self) -> bool {
        matches!(self, Self::SelfValue)
    }

    /// Returns the source symbol when this local has one.
    pub fn symbol(self) -> Option<SymbolId> {
        match self {
            Self::Named(name) => Some(name),
            Self::SelfValue | Self::Generated(_) | Self::Temporary(_) | Self::Anonymous => None,
        }
    }

    /// Returns the stable internal storage name used by function IR.
    pub fn internal_storage_name(self) -> String {
        match self {
            Self::SelfValue => "self".to_string(),
            Self::Named(name) => unresolved_symbol_text(name),
            Self::Generated(GeneratedLocalName::ForIterable) => "__for_iterable".to_string(),
            Self::Generated(GeneratedLocalName::ForIterator) => "__for_iter".to_string(),
            Self::Generated(GeneratedLocalName::ForNext) => "__for_next".to_string(),
            Self::Temporary(id) => format!("fir.tmp.{id}"),
            Self::Anonymous => "_".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_symbol::SymbolId;

    #[test]
    fn local_name_categories_preserve_identity() {
        let symbol = SymbolId::from_stable_hash(7);
        assert_eq!(LocalName::named(symbol).symbol(), Some(symbol));
        assert!(LocalName::SelfValue.is_self_value());
        assert_eq!(LocalName::temporary(3).symbol(), None);
        assert_eq!(
            LocalName::generated(GeneratedLocalName::ForIterator).symbol(),
            None
        );
    }

    #[test]
    fn internal_storage_names_are_stable_and_distinct() {
        assert_eq!(LocalName::SelfValue.internal_storage_name(), "self");
        assert_eq!(
            LocalName::generated(GeneratedLocalName::ForIterable).internal_storage_name(),
            "__for_iterable"
        );
        assert_eq!(
            LocalName::generated(GeneratedLocalName::ForIterator).internal_storage_name(),
            "__for_iter"
        );
        assert_eq!(
            LocalName::temporary(12).internal_storage_name(),
            "fir.tmp.12"
        );
        assert_eq!(LocalName::Anonymous.internal_storage_name(), "_");
    }
}
