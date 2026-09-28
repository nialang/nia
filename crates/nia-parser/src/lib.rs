// SPDX-License-Identifier: GPL-3.0-or-later
//! AST lowering from finished syntax trees.
//!
//! Lowering is a pure function of a [`nia_syntax::Parse`]: every grammar
//! decision, recovery, and syntax diagnostic already happened in
//! `nia-syntax`. This crate interns names, builds AST values with their
//! stable node identities, and records a [`NodeOriginTable`] for every AST
//! node it creates. It never reads tokens outside the tree and never makes a
//! parse decision.

mod lower;

use nia_ast::Module;
use nia_node_id::{NodeOriginTable, NodeStore};
use nia_source::{SourceId, SourceRevision, SourceVersion};
use nia_symbol_table::SymbolTable;
use nia_syntax::{Parse, ParseError};

/// AST, diagnostics, and syntax origins lowered from one parse.
#[derive(Debug)]
pub struct LoweredModule {
    /// Lowered module AST.
    pub module: Module,
    /// Grammar diagnostics followed by lowering diagnostics.
    pub errors: Vec<ParseError>,
    /// Accepted AST-to-syntax origins.
    pub origins: NodeOriginTable,
}

/// Lowers a parsed source file into AST using a shared node identity store.
pub fn lower_module(parse: &Parse, node_store: &NodeStore, symbols: SymbolTable) -> LoweredModule {
    lower::lower_module(parse, node_store, symbols)
}

/// Parses and lowers an in-memory source string with fresh stores.
pub fn parse_module(source: &str) -> (Module, Vec<ParseError>) {
    parse_module_with_symbols(source, SymbolTable::new())
}

/// Parses and lowers an in-memory source string with caller symbols.
pub fn parse_module_with_symbols(source: &str, symbols: SymbolTable) -> (Module, Vec<ParseError>) {
    let parse = parse_isolated(source);
    let lowered = lower_module(&parse, &NodeStore::new(), symbols);
    (lowered.module, lowered.errors)
}

/// Parses a source string under a fresh isolated source identity.
pub fn parse_isolated(source: &str) -> Parse {
    let version = SourceVersion {
        id: SourceId::isolated(),
        revision: SourceRevision::INITIAL,
    };
    parse_or_ice(source, Some(version))
}

/// Parses and lowers one source revision with fresh stores.
pub fn parse_versioned(
    source: &str,
    version: Option<SourceVersion>,
    symbols: SymbolTable,
) -> LoweredModule {
    lower_module(&parse_or_ice(source, version), &NodeStore::new(), symbols)
}

fn parse_or_ice(source: &str, version: Option<SourceVersion>) -> Parse {
    nia_syntax::parse(source, version)
        .unwrap_or_else(|error| panic!("Nia ICE: grammar produced an invalid tree: {error:?}"))
}

#[cfg(test)]
mod tests;
