// SPDX-License-Identifier: GPL-3.0-or-later
use nia_symbol::{SymbolId, unresolved_symbol_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PromotedAllocationId {
    module_id: nia_ids::ModuleId,
    span: nia_span::Span,
}

impl PromotedAllocationId {
    pub const fn new(module_id: nia_ids::ModuleId, span: nia_span::Span) -> Self {
        Self { module_id, span }
    }

    pub const fn module_id(self) -> nia_ids::ModuleId {
        self.module_id
    }

    pub const fn span(self) -> nia_span::Span {
        self.span
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalName {
    SelfValue,
    Named(SymbolId),
    Generated(GeneratedLocalName),
    Temporary(u32),
    Anonymous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeneratedLocalName {
    ForIterable,
    ForIterator,
    ForNext,
}

impl LocalName {
    pub fn named(name: SymbolId) -> Self {
        Self::Named(name)
    }

    pub fn generated(name: GeneratedLocalName) -> Self {
        Self::Generated(name)
    }

    pub fn temporary(id: u32) -> Self {
        Self::Temporary(id)
    }

    pub fn is_self_value(self) -> bool {
        matches!(self, Self::SelfValue)
    }

    pub fn symbol(self) -> Option<SymbolId> {
        match self {
            Self::Named(name) => Some(name),
            Self::SelfValue | Self::Generated(_) | Self::Temporary(_) | Self::Anonymous => None,
        }
    }

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
