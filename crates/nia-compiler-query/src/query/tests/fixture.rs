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
    pub(super) sources: nia_source::SourceDatabase,
}

impl LoadedProgramFixture {
    pub(super) fn new(entry_path: &str, source: &str) -> Self {
        let sources = nia_source::SourceDatabase::new();
        let graph = ModuleGraph::with_source_table(
            SourcePath::new(entry_path),
            Arc::new(test_symbols()),
            sources.source_table(),
        )
        .expect("create module graph");
        let entry_id = graph.entry();
        Self {
            graph,
            modules: vec![loaded_module_in(
                &sources,
                entry_id,
                SourcePath::new(entry_path),
                source,
                SourceRevision::INITIAL,
            )],
            sources,
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
        self.modules.push(loaded_module_in(
            &self.sources,
            module_id,
            SourcePath::new(path),
            source,
            SourceRevision::INITIAL,
        ));
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
        self.modules.push(loaded_module_in(
            &self.sources,
            module_id,
            SourcePath::new(path),
            source,
            SourceRevision::INITIAL,
        ));
        module_id
    }

    fn add_child_with_source_path(
        &mut self,
        parent: ModuleId,
        child_name: &str,
        visibility: nia_ids::Visibility,
        path: SourcePath,
        source: &str,
    ) -> ModuleId {
        let module_id = self
            .graph
            .intern_declared_child_with_source_path(
                parent,
                &sym(child_name),
                visibility,
                Span::default(),
                path.clone(),
            )
            .expect("intern child source path");
        self.modules.push(loaded_module_in(
            &self.sources,
            module_id,
            path,
            source,
            SourceRevision::INITIAL,
        ));
        module_id
    }

    pub(super) fn add_freestanding_runtime(&mut self, source: &str) -> ModuleId {
        let runtime_root_path =
            SourcePath::with_identity("runtime/pkg.nia", "toolchain:/runtime/pkg.nia");
        let runtime_root = self
            .graph
            .intern_runtime_package_root(runtime_root_path.clone())
            .expect("intern runtime package root");
        self.modules.push(loaded_module_in(
            &self.sources,
            runtime_root,
            runtime_root_path,
            "",
            SourceRevision::INITIAL,
        ));
        let start_path =
            SourcePath::with_identity("runtime/start.nia", "toolchain:/runtime/start.nia");
        let start = self
            .graph
            .intern_declared_child_with_source_path(
                runtime_root,
                &sym("start"),
                nia_ids::Visibility::PublicPkg,
                Span::default(),
                start_path.clone(),
            )
            .expect("intern runtime start");
        self.modules.push(loaded_module_in(
            &self.sources,
            start,
            start_path,
            "pub(pkg) module freestanding;",
            SourceRevision::INITIAL,
        ));
        let freestanding = self.add_child_with_source_path(
            start,
            "freestanding",
            nia_ids::Visibility::PublicPkg,
            SourcePath::with_identity(
                "runtime/start/freestanding.nia",
                "toolchain:/runtime/start/freestanding.nia",
            ),
            "pub(pkg) module linux;",
        );
        let linux = self.add_child_with_source_path(
            freestanding,
            "linux",
            nia_ids::Visibility::PublicPkg,
            SourcePath::with_identity(
                "runtime/start/freestanding/linux.nia",
                "toolchain:/runtime/start/freestanding/linux.nia",
            ),
            "pub(pkg) module x86_64;",
        );
        let implementation = self.add_child_with_source_path(
            linux,
            "x86_64",
            nia_ids::Visibility::PublicPkg,
            SourcePath::with_identity(
                "runtime/start/freestanding/linux/x86_64.nia",
                "toolchain:/runtime/start/freestanding/linux/x86_64.nia",
            ),
            source,
        );
        self.graph.mark_executable_root_subtree(start);
        implementation
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
        *module = loaded_module_in(
            &self.sources,
            module_id,
            module.path.clone(),
            source,
            revision,
        );
    }

    pub(super) fn update_module_path(&mut self, module_id: ModuleId, path: &str) {
        let module = self
            .modules
            .iter_mut()
            .find(|module| module.id == module_id)
            .expect("fixture module");
        *module = loaded_module_in(
            &self.sources,
            module_id,
            SourcePath::new(path),
            &module.source_text,
            module.source_version.revision,
        );
    }

    pub(super) fn program(&self) -> LoadedProgram {
        LoadedProgram {
            graph: self.graph.clone().into(),
            provider_fact_revision: crate::ProviderFactRevision::default(),
            symbols: test_symbols(),
            target: TargetConfig::host(),
            profile: nia_target_config::BuildProfile::Debug,
            compilation_mode: nia_target_config::CompilationMode::Normal,
            runtime: RuntimeSpec::Bare,
            toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint::current(),
            modules: self.modules.clone(),
            diagnostics: Vec::new(),
        }
    }

    pub(super) fn freestanding_program(&self) -> LoadedProgram {
        self.freestanding_program_with_runtime(
            "using entry; pub extern fn _start() () { _ = &entry::main; }",
        )
    }

    pub(super) fn freestanding_program_with_runtime(&self, source: &str) -> LoadedProgram {
        let mut fixture = Self {
            graph: self.graph.clone(),
            modules: self.modules.clone(),
            sources: self.sources.clone(),
        };
        fixture.add_freestanding_runtime(source);
        let mut program = fixture.program();
        program.runtime = test_freestanding_runtime();
        program
    }

    pub(super) fn database(&self) -> CompilerDatabase {
        CompilerDatabase::new(CompileRequest::new(self.program()))
    }
}

pub(super) fn loaded_module(id: ModuleId, path: &str, source: &str) -> LoadedModule {
    let sources = nia_source::SourceDatabase::new();
    loaded_module_in(
        &sources,
        id,
        SourcePath::new(path),
        source,
        SourceRevision::INITIAL,
    )
}

pub(super) fn test_freestanding_runtime() -> RuntimeSpec {
    RuntimeSpec::freestanding_from_package_root("runtime/pkg.nia", &TargetConfig::host())
        .expect("host test runtime")
}

fn loaded_module_in(
    sources: &nia_source::SourceDatabase,
    id: ModuleId,
    path: SourcePath,
    source: &str,
    revision: SourceRevision,
) -> LoadedModule {
    let source_id = sources
        .id_for_path(&path)
        .expect("allocate fixture source id");
    let mut module = loaded_module_with_source_version(
        id,
        path.as_str(),
        source,
        SourceVersion {
            id: source_id,
            revision,
        },
    );
    module.path = path.clone();
    module.source_identity = path.identity();
    module
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
    let active_item_tree = item_tree.all_items_active();
    let provider_summary =
        nia_provider_summary::ProviderSummary::from_active_item_tree(&active_item_tree);
    LoadedModule {
        id,
        path: SourcePath::new(path),
        source_identity: SourcePath::new(path).identity(),
        source_version,
        source_text: Arc::from(source),
        item_tree: item_tree.clone(),
        active_item_tree,
        provider_summary,
        parse_errors,
        unused_imports: Vec::new(),
        origins,
    }
}
