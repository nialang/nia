// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{HashSet, VecDeque},
    fs,
};

use crate::{LoadedModule, LoadedProgram, ProgramDiagnostic, module_diagnostics};
use nia_diagnostic::Diagnostic;
use nia_imports::{
    ModuleGraph, ModuleMap, SourcePath, collect_import_aliases, collect_module_imports,
};
use nia_span::Span;

pub fn load_program(root_path: impl Into<String>) -> LoadedProgram {
    load_program_with_map(root_path, ModuleMap::default())
}

pub fn load_program_with_map(root_path: impl Into<String>, module_map: ModuleMap) -> LoadedProgram {
    let root_path = SourcePath::new(root_path.into());
    let mut loader = ProgramLoader {
        graph: ModuleGraph::new(root_path.clone()),
        modules: Vec::new(),
        diagnostics: Vec::new(),
        loaded: HashSet::new(),
        queue: VecDeque::from([root_path]),
        module_map,
    };
    loader.load_all();
    let imports = collect_import_aliases(&loader.graph);
    LoadedProgram {
        graph: loader.graph,
        imports,
        modules: loader.modules,
        diagnostics: loader.diagnostics,
    }
}

struct ProgramLoader {
    graph: ModuleGraph,
    modules: Vec<LoadedModule>,
    diagnostics: Vec<ProgramDiagnostic>,
    loaded: HashSet<String>,
    queue: VecDeque<SourcePath>,
    module_map: ModuleMap,
}

impl ProgramLoader {
    fn load_all(&mut self) {
        while let Some(path) = self.queue.pop_front() {
            if !self.loaded.insert(path.as_str().to_string()) {
                continue;
            }
            self.load_one(path);
        }
    }

    fn load_one(&mut self, path: SourcePath) {
        let Some(module_id) = self.graph.module_id_for_path(path.as_str()) else {
            self.diagnostics.push(ProgramDiagnostic {
                path: path.clone(),
                diagnostic: Diagnostic::error(
                    Span::default(),
                    format!(
                        "internal loader error: module path `{}` is not interned",
                        path.as_str()
                    ),
                ),
            });
            return;
        };

        let source = match fs::read_to_string(path.as_str()) {
            Ok(source) => source,
            Err(err) => {
                self.diagnostics.push(ProgramDiagnostic {
                    path: path.clone(),
                    diagnostic: Diagnostic::error(
                        Span::default(),
                        format!("failed to read `{}`: {err}", path.as_str()),
                    ),
                });
                return;
            }
        };
        let (module, parse_errors) = nia_parser::parse_module(&source);
        let mut import_diagnostics = Vec::new();
        collect_module_imports(
            &mut self.graph,
            &mut import_diagnostics,
            module_id,
            &path,
            &module,
            &self.module_map,
        );
        self.diagnostics
            .extend(module_diagnostics(&path, &import_diagnostics));
        if let Some(node) = self.graph.get(module_id) {
            for import in &node.imports {
                if !self.loaded.contains(import.path.as_str()) {
                    self.queue.push_back(import.path.clone());
                }
            }
        }
        self.modules.push(LoadedModule {
            id: module_id,
            path,
            source,
            module,
            parse_errors,
        });
    }
}
