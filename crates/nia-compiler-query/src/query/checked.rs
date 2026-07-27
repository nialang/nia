// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(in crate::query) struct ExecutableValueRefItemInput {
    pub(in crate::query) item_index: usize,
    pub(in crate::query) owner_node_key: nia_node_id::VersionedNodeKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CheckedModuleQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for CheckedModuleQuery {
    type Value = CheckedModule;

    fn name() -> &'static str {
        "checked_module"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.checked_module)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CheckedModuleIdsQuery;

impl QueryKey<CompilerContext> for CheckedModuleIdsQuery {
    type Value = Vec<ModuleId>;

    fn name() -> &'static str {
        "checked_module_ids"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.checked_module_ids)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableCheckedModuleFactsQuery;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExecutableCheckedModuleFacts {
    pub(super) modules: Vec<Arc<CheckedModule>>,
    pub(super) const_modules: HashMap<ModuleId, Arc<nia_const_ir::ResolvedConstModule>>,
    pub(super) runtime_functions: Vec<GlobalDefId>,
    pub(super) runtime_globals: Vec<GlobalDefId>,
    pub(super) reachable_body_modules: HashSet<ModuleId>,
}

impl QueryKey<CompilerContext> for ExecutableCheckedModuleFactsQuery {
    type Value = ExecutableCheckedModuleFacts;

    fn name() -> &'static str {
        "executable_checked_module_facts"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        provide_executable_checked_module_facts(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableCheckedModulesQuery;

impl QueryKey<CompilerContext> for ExecutableCheckedModulesQuery {
    type Value = Vec<Arc<CheckedModule>>;

    fn name() -> &'static str {
        "executable_checked_modules"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        materialize_executable_checked_modules(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableValueRefItemIndexQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ExecutableValueRefItemIndexQuery {
    type Value = HashMap<nia_ids::DefId, ExecutableValueRefItemInput>;

    fn name() -> &'static str {
        "executable_value_ref_item_index"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        let active_item_tree = db.get(FullActiveModuleItemTreeQuery(self.0))?;
        let defs = module_defs_semantic(db, self.0)?;
        let mut index = HashMap::new();
        for (item_index, item) in active_item_tree.items.iter().enumerate() {
            index_executable_value_ref_item(item, item_index, &defs, &mut index);
        }
        Ok(index)
    }
}

fn index_executable_value_ref_item(
    item: &nia_item_tree::ItemTreeNode,
    item_index: usize,
    defs: &DefCollection,
    index: &mut HashMap<nia_ids::DefId, ExecutableValueRefItemInput>,
) {
    let mut insert = |node_key: &nia_node_id::VersionedNodeKey| {
        let Some(def_id) = defs.def_nodes.get(node_key) else {
            return;
        };
        index.insert(
            def_id,
            ExecutableValueRefItemInput {
                item_index,
                owner_node_key: node_key.clone(),
            },
        );
    };
    match &item.kind {
        nia_item_tree::ItemTreeNodeKind::Function(function)
            if !function.is_const && function.body.is_some() =>
        {
            insert(&function.node_key);
        }
        nia_item_tree::ItemTreeNodeKind::Binding(binding)
            if !binding.is_const() && binding.value.is_some() =>
        {
            insert(&binding.node_key);
        }
        nia_item_tree::ItemTreeNodeKind::Trait(item_trait) => {
            for method in &item_trait.methods {
                if !method.function.is_const && method.function.body.is_some() {
                    insert(&method.function.node_key);
                }
            }
        }
        nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
            for method in &extend.methods {
                if !method.function.is_const && method.function.body.is_some() {
                    insert(&method.function.node_key);
                }
            }
            for value in &extend.associated_values {
                if !value.binding.is_const() && value.binding.value.is_some() {
                    insert(&value.binding.node_key);
                }
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableValueRefItemQuery(pub(super) GlobalDefId);

impl QueryKey<CompilerContext> for ExecutableValueRefItemQuery {
    type Value = Option<ExecutableValueRefItemInput>;

    fn name() -> &'static str {
        "executable_value_ref_item"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get(ExecutableValueRefItemIndexQuery(self.0.module_id))?
            .get(&self.0.def_id)
            .cloned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableValueRefEdgesQuery(pub(super) GlobalDefId);

impl QueryKey<CompilerContext> for ExecutableValueRefEdgesQuery {
    type Value = ExecutableValueRefEdges;

    fn name() -> &'static str {
        "executable_value_ref_edges"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        provide_executable_value_ref_edges(db, self.0)
    }
}
