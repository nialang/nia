// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::ops::Deref;

#[derive(Clone)]
pub(super) struct CompilerDatabase {
    compiler: super::super::CompilerDatabase,
    loader: TestLoaderFacts,
}

impl CompilerDatabase {
    pub(super) fn new(request: CompileRequest) -> Self {
        let provider_facts = request
            .loader_facts
            .provider_facts()
            .expect("test provider facts");
        let program = materialize_loader_facts(request.loader_facts.as_ref());
        let loader = TestLoaderFacts::new(program, provider_facts);
        let request = request.with_loader_facts(loader.clone());
        let compiler = super::super::CompilerDatabase::new(request);
        Self { compiler, loader }
    }

    pub(super) fn update(&self, request: CompileRequest) -> CompilerInvalidation {
        let mut program = materialize_loader_facts(request.loader_facts.as_ref());
        program.provider_fact_revision = crate::LoaderFactProvider::provider_facts(&self.loader)
            .expect("test provider facts")
            .revision();
        self.loader.replace_program(program);
        let request = request.with_loader_facts(self.loader.clone());
        self.compiler.update(request).expect("test compiler update")
    }

    pub(super) fn check_program(&self) -> CheckedProgram {
        self.compiler.check_program().expect("test compiler check")
    }

    pub(super) fn analyze_program(&self) -> CheckedProgramAnalysis {
        self.compiler
            .analyze_program()
            .expect("test compiler analysis")
    }

    pub(super) fn codegen_program(&self) -> CodegenProgram {
        self.compiler
            .codegen_program()
            .expect("test codegen program")
    }

    pub(super) fn codegen_preparation(&self) -> CodegenPreparation {
        self.compiler
            .codegen_preparation()
            .expect("test codegen preparation")
    }

    pub(super) fn replace_provider_facts(
        &self,
        provider_facts: crate::ProviderFactSnapshot,
    ) -> nia_query::QueryInvalidation {
        self.loader.replace_provider_facts(provider_facts)
    }
}

impl Deref for CompilerDatabase {
    type Target = super::super::CompilerDatabase;

    fn deref(&self) -> &Self::Target {
        &self.compiler
    }
}

fn materialize_loader_facts(facts: &dyn crate::LoaderFactProvider) -> LoadedProgram {
    let graph = facts.module_graph().expect("test module graph");
    let modules = facts
        .loaded_module_source_identities()
        .expect("test loaded module identities")
        .into_iter()
        .map(|identity| {
            let module_id = graph
                .modules()
                .find_map(|module| {
                    facts
                        .module_path(module.id)
                        .expect("test module path query")
                        .is_some_and(|path| path.identity() == identity)
                        .then_some(module.id)
                })
                .expect("test loaded module identity must resolve in graph");
            LoadedModule {
                id: module_id,
                path: facts
                    .module_path(module_id)
                    .expect("test module path query")
                    .expect("test module path"),
                source_identity: facts
                    .module_path(module_id)
                    .expect("test module path query")
                    .expect("test module path")
                    .identity(),
                source_version: facts
                    .module_source_version(module_id)
                    .expect("test module source version query")
                    .expect("test module source version"),
                source_text: facts
                    .module_source_text(module_id)
                    .expect("test module source text query")
                    .unwrap_or_else(|| Arc::from("")),
                item_tree: facts
                    .module_item_tree(module_id)
                    .expect("test module item tree query")
                    .expect("test module item tree"),
                active_item_tree: facts
                    .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)
                    .expect("test active module item tree query")
                    .expect("test active module item tree"),
                provider_summary: facts
                    .module_provider_summary(module_id)
                    .expect("test module provider summary query")
                    .expect("test module provider summary"),
                origins: facts
                    .module_origins(module_id)
                    .expect("test module origins query")
                    .expect("test module origins"),
                parse_errors: facts
                    .module_parse_errors(module_id)
                    .expect("test module parse errors query")
                    .expect("test module parse errors"),
            }
        })
        .collect();
    LoadedProgram {
        graph,
        provider_fact_revision: facts
            .provider_facts()
            .expect("test provider facts")
            .revision(),
        symbols: facts.symbols(),
        target: facts.target(),
        runtime: facts.runtime(),
        toolchain_identity: facts.toolchain_identity(),
        modules,
        diagnostics: facts
            .load_diagnostics()
            .expect("test load diagnostics")
            .to_diagnostics(),
    }
}
