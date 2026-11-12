// SPDX-License-Identifier: GPL-3.0-or-later
//! Source declarations of the module's const bindings.
//!
//! Const lowering needs a binding's declaration in two places: to lower each
//! local const initializer, and to recover the type of a const that a hint
//! refers to. Both read this table, found in one structural walk of the
//! active items, so a binding nested anywhere a block is accepted (`if` and
//! `match` arms, block tails, closures) is found the same way the local
//! resolver allocates it.

use std::collections::HashMap;

use nia_ast::{BindingItem, BindingStmt, LocalBindingKind, Pattern, PatternKind, Stmt, StmtKind};
use nia_ast_walk::Visitor;
use nia_defs::{DefId, DefKind};
use nia_ids::LocalId;
use nia_item_tree::ItemTreeNodeKind;

use crate::ConstModuleInput;

/// Const bindings declared by the active module, by identity.
pub(crate) struct ConstDeclarations<'a> {
    locals: HashMap<LocalId, &'a BindingStmt>,
    globals: HashMap<DefId, &'a BindingItem>,
}

impl<'a> ConstDeclarations<'a> {
    pub(crate) fn collect(input: &ConstModuleInput<'a>) -> Self {
        let mut collector = Collector {
            input,
            locals: HashMap::new(),
        };
        let mut globals = HashMap::new();
        for item in input.active_item_tree.items.iter() {
            collector.visit_item(item);
            match &item.kind {
                ItemTreeNodeKind::Binding(binding) => {
                    collector.insert_global(&mut globals, binding);
                }
                // Values of a generic extension are typed in terms of its
                // parameters, so they say nothing about a use's type.
                ItemTreeNodeKind::Extend(extend) if extend.generics.is_empty() => {
                    for associated_value in &extend.associated_values {
                        collector.insert_global(&mut globals, &associated_value.binding);
                    }
                }
                _ => {}
            }
        }
        Self {
            locals: collector.locals,
            globals,
        }
    }

    /// The declaration of a local const binding.
    pub(crate) fn local(&self, id: LocalId) -> Option<&'a BindingStmt> {
        self.locals.get(&id).copied()
    }

    /// The declaration of a const owned by the active module.
    pub(crate) fn global(&self, id: DefId) -> Option<&'a BindingItem> {
        self.globals.get(&id).copied()
    }
}

struct Collector<'i, 'a> {
    input: &'i ConstModuleInput<'a>,
    locals: HashMap<LocalId, &'a BindingStmt>,
}

impl<'a> Collector<'_, 'a> {
    fn insert_global(
        &self,
        globals: &mut HashMap<DefId, &'a BindingItem>,
        binding: &'a BindingItem,
    ) {
        if !binding.is_const() {
            return;
        }
        let Some(def_id) = self.input.defs.def_nodes.get(&binding.node_key) else {
            return;
        };
        if self
            .input
            .defs
            .defs
            .get(def_id)
            .is_some_and(|def| def.kind == DefKind::Const)
        {
            globals.insert(def_id, binding);
        }
    }
}

impl Collector<'_, '_> {
    fn bound_local(&self, pattern: &Pattern) -> Option<LocalId> {
        match &pattern.kind {
            PatternKind::Bind { node_key, .. } => self.input.semantic_uses.node_local_def(node_key),
            PatternKind::Pointer(pattern) | PatternKind::MutPointer(pattern) => {
                self.bound_local(pattern)
            }
            _ => None,
        }
    }
}

impl<'a> Visitor<'a> for Collector<'_, 'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let StmtKind::Binding(binding) = &stmt.kind
            && binding.kind == LocalBindingKind::Const
            && let Some(local_id) = self.bound_local(&binding.pattern)
        {
            self.locals.insert(local_id, binding);
        }
        nia_ast_walk::walk_stmt(self, stmt);
    }
}
