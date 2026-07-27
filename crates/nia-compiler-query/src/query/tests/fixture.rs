// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn intern_child(
    graph: &mut ModuleGraph,
    parent: ModuleId,
    child_name: &str,
    visibility: nia_ids::Visibility,
) -> ModuleId {
    let child = sym(child_name);
    graph
        .intern_declared_child(parent, &child, visibility, Span::default())
        .expect("intern child module")
}

fn intern_shallow_child(
    graph: &mut ModuleGraph,
    parent: ModuleId,
    child_name: &str,
    visibility: nia_ids::Visibility,
) -> ModuleId {
    let child = sym(child_name);
    graph
        .intern_declared_child_with_processing(
            parent,
            &child,
            visibility,
            Span::default(),
            false,
            false,
        )
        .expect("intern shallow child module")
}

pub(super) struct LoadedProgramFixture {
    pub(super) graph: ModuleGraph,
    pub(super) modules: Vec<LoadedModule>,
}

impl LoadedProgramFixture {
    pub(super) fn new(entry_path: &str, source: &str) -> Self {
        let graph =
            ModuleGraph::with_symbol_text(SourcePath::new(entry_path), Arc::new(test_symbols()));
        let entry_id = graph.entry();
        Self {
            graph,
            modules: vec![loaded_module(entry_id, entry_path, source)],
        }
    }

    pub(super) fn entry_id(&self) -> ModuleId {
        self.graph.entry()
    }

    pub(super) fn add_child(
        &mut self,
        parent: ModuleId,
        child_name: &str,
        path: &str,
        source: &str,
    ) -> ModuleId {
        self.add_child_with_visibility(
            parent,
            child_name,
            nia_ids::Visibility::Public,
            path,
            source,
        )
    }

    pub(super) fn add_child_with_visibility(
        &mut self,
        parent: ModuleId,
        child_name: &str,
        visibility: nia_ids::Visibility,
        path: &str,
        source: &str,
    ) -> ModuleId {
        let module_id = intern_child(&mut self.graph, parent, child_name, visibility);
        self.modules.push(loaded_module(module_id, path, source));
        module_id
    }

    pub(super) fn add_shallow_child(
        &mut self,
        parent: ModuleId,
        child_name: &str,
        path: &str,
        source: &str,
    ) -> ModuleId {
        let module_id = intern_shallow_child(
            &mut self.graph,
            parent,
            child_name,
            nia_ids::Visibility::Public,
        );
        self.modules.push(loaded_module(module_id, path, source));
        module_id
    }

    pub(super) fn update_module_source(
        &mut self,
        module_id: ModuleId,
        source: &str,
        revision: SourceRevision,
    ) {
        let module = self
            .modules
            .iter_mut()
            .find(|module| module.id == module_id)
            .expect("fixture module");
        *module = loaded_module_with_revision(module_id, module.path.as_str(), source, revision);
    }

    pub(super) fn update_module_path(&mut self, module_id: ModuleId, path: &str) {
        let module = self
            .modules
            .iter_mut()
            .find(|module| module.id == module_id)
            .expect("fixture module");
        module.path = SourcePath::new(path);
        module.source_identity = module.path.identity();
    }

    pub(super) fn program(&self) -> LoadedProgram {
        LoadedProgram {
            graph: self.graph.clone().into(),
            provider_fact_revision: crate::ProviderFactRevision::default(),
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::Bare,
            modules: self.modules.clone(),
            diagnostics: Vec::new(),
        }
    }

    pub(super) fn database(&self) -> CompilerDatabase {
        CompilerDatabase::new(CompileRequest::new(self.program()))
    }
}

pub(super) fn loaded_module(id: ModuleId, path: &str, source: &str) -> LoadedModule {
    loaded_module_with_revision(id, path, source, SourceRevision::INITIAL)
}

fn loaded_module_with_revision(
    id: ModuleId,
    path: &str,
    source: &str,
    revision: SourceRevision,
) -> LoadedModule {
    loaded_module_with_source_version(
        id,
        path,
        source,
        SourceVersion {
            id: SourceId(id.local_index()),
            revision,
        },
    )
}

pub(super) fn loaded_module_with_source_version(
    id: ModuleId,
    path: &str,
    source: &str,
    source_version: SourceVersion,
) -> LoadedModule {
    let syntax = nia_syntax::parse_source(source, Some(source_version));
    let (module, parse_errors, origins) =
        nia_parser::parse_module_syntax_with_origins_and_symbols(&syntax, test_symbols());
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    let item_tree = ModuleItemTree::from_module(&module);
    let active_item_tree = ActiveModuleItemTree::new(item_tree.items.clone(), Default::default());
    let provider_summary =
        nia_provider_summary::ProviderSummary::from_active_item_tree(&active_item_tree);
    LoadedModule {
        id,
        path: SourcePath::new(path),
        source_identity: SourcePath::new(path).identity(),
        source_version,
        item_tree: item_tree.clone(),
        active_item_tree,
        provider_summary,
        parse_errors,
        origins,
    }
}
