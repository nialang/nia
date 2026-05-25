// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
};

use crate::{LoadedModule, LoadedProgram, ProgramDiagnostic, module_diagnostics};
use nia_diagnostic::Diagnostic;
use nia_ids::ModuleId;
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
    loader
        .diagnostics
        .extend(detect_import_cycles(&loader.graph));
    let imports = collect_import_aliases(&loader.graph);
    LoadedProgram {
        graph: loader.graph,
        imports,
        modules: loader.modules,
        diagnostics: loader.diagnostics,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Done,
}

fn detect_import_cycles(graph: &ModuleGraph) -> Vec<ProgramDiagnostic> {
    let mut states = HashMap::new();
    let mut diagnostics = Vec::new();
    for module in graph.modules() {
        if !states.contains_key(&module.id) {
            detect_import_cycles_from(module.id, graph, &mut states, &mut diagnostics);
        }
    }
    diagnostics
}

fn detect_import_cycles_from(
    module_id: ModuleId,
    graph: &ModuleGraph,
    states: &mut HashMap<ModuleId, VisitState>,
    diagnostics: &mut Vec<ProgramDiagnostic>,
) {
    states.insert(module_id, VisitState::Visiting);
    let Some(module) = graph.get(module_id) else {
        states.insert(module_id, VisitState::Done);
        return;
    };
    for import in &module.imports {
        match states.get(&import.target).copied() {
            Some(VisitState::Visiting) => diagnostics.push(ProgramDiagnostic {
                path: module.path.clone(),
                diagnostic: Diagnostic::error(
                    import.span,
                    format!(
                        "import cycle detected: `{}` imports `{}`",
                        module.path.as_str(),
                        import.path.as_str()
                    ),
                ),
            }),
            Some(VisitState::Done) => {}
            None => detect_import_cycles_from(import.target, graph, states, diagnostics),
        }
    }
    states.insert(module_id, VisitState::Done);
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
