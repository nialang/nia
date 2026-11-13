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
use nia_ids::{GlobalDefId, LocalId};
use nia_item_tree::ItemTreeNodeKind;
use nia_node_id::VersionedNodeKey;

use crate::ConstModuleInput;

/// Const bindings declared by the active module, by identity.
pub(crate) struct ConstDeclarations<'a> {
    locals: HashMap<LocalId, &'a BindingStmt>,
    globals: HashMap<DefId, &'a BindingItem>,
    pub(crate) local_owners: HashMap<LocalId, GlobalDefId>,
    pub(crate) expression_owners: HashMap<VersionedNodeKey, GlobalDefId>,
}

impl<'a> ConstDeclarations<'a> {
    pub(crate) fn collect(input: &ConstModuleInput<'a>) -> Self {
        let mut collector = Collector {
            input,
            locals: HashMap::new(),
            owner: None,
            local_owners: HashMap::new(),
            expression_owners: HashMap::new(),
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
            local_owners: collector.local_owners,
            expression_owners: collector.expression_owners,
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
    owner: Option<GlobalDefId>,
    local_owners: HashMap<LocalId, GlobalDefId>,
    expression_owners: HashMap<VersionedNodeKey, GlobalDefId>,
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
    fn visit_function(&mut self, function: &'a nia_ast::FunctionItem) {
        let owner = self
            .input
            .defs
            .def_nodes
            .get(&function.node_key)
            .map(|def_id| GlobalDefId {
                module_id: self.input.defs.module_id,
                def_id,
            });
        let previous = std::mem::replace(&mut self.owner, owner);
        nia_ast_walk::walk_function(self, function);
        self.owner = previous;
    }

    fn visit_expr(&mut self, expr: &'a nia_ast::Expr) {
        if let Some(owner) = self.owner {
            self.expression_owners.insert(expr.node_key.clone(), owner);
        }
        nia_ast_walk::walk_expr(self, expr);
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let StmtKind::Binding(binding) = &stmt.kind
            && binding.kind == LocalBindingKind::Const
            && let Some(local_id) = self.bound_local(&binding.pattern)
        {
            self.locals.insert(local_id, binding);
            if let Some(owner) = self.owner {
                self.local_owners.insert(local_id, owner);
            }
        }
        nia_ast_walk::walk_stmt(self, stmt);
    }
}
