// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

use lsp_types::{Diagnostic, DiagnosticSeverity, Location, NumberOrString, Position, Uri};
use nia_compiler_query::{CheckedProgramAnalysis, CompilerDatabase};
use nia_diagnostic::LabelStyle;
use nia_driver::{CheckRequest, Driver};
use nia_ids::{GlobalDefId, LocalId, ModuleId};
use nia_imports::ModuleMap;
use nia_lexer::{LosslessTokenKind, TokenKind, tokenize_lossless};
use nia_package_metadata::{ArtifactKind, PackageManifest};
use nia_sema_ir::{ResolvedCall, SemanticValueUse};
use nia_source::SourcePath;
use nia_span::Span;
use nia_symbol::{SymbolId, stable_hash};
use nia_target_config::CompilationMode;
use nia_toolchain::ToolchainLayout;
use url::Url;

use crate::document::{Document, Lines};

pub(crate) type Documents = BTreeMap<PathBuf, OpenDocument>;

#[derive(Clone)]
pub(crate) struct OpenDocument {
    pub(crate) uri: Uri,
    pub(crate) document: Document,
}

pub(crate) fn path(uri: &Uri) -> Option<PathBuf> {
    Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

fn uri(path: &Path) -> Option<Uri> {
    Url::from_file_path(path).ok()?.as_str().parse().ok()
}

fn normalized(path: &Path) -> String {
    nia_source::normalize_path(&path.to_string_lossy())
}

pub(crate) struct Session {
    toolchain: Arc<ToolchainLayout>,
    driver: Driver,
    overlays: BTreeMap<PathBuf, Arc<str>>,
}

impl Session {
    pub(crate) fn new(toolchain: Arc<ToolchainLayout>) -> Self {
        Self {
            driver: Driver::new(toolchain.clone()),
            toolchain,
            overlays: BTreeMap::new(),
        }
    }

    pub(crate) fn analyze(
        &mut self,
        documents: &Documents,
        entry: &Path,
    ) -> Result<Analysis, String> {
        // A fresh source database lets closed files return to disk ownership.
        if self
            .overlays
            .keys()
            .any(|path| !documents.contains_key(path))
        {
            self.driver = Driver::new(self.toolchain.clone());
            self.overlays.clear();
        }
        for (path, open) in documents {
            let text = &open.document.text;
            if self.overlays.get(path).is_some_and(|old| old == text) {
                continue;
            }
            self.driver
                .set_source(normalized(path), text.clone())
                .map_err(|error| match error {
                    nia_driver::DriverError::InternalDiagnostic(diagnostic) => diagnostic.summary,
                    other => format!("{other:?}"),
                })?;
            self.overlays.insert(path.clone(), text.clone());
        }
        let (request, fallback) = request_for(entry)?;
        let analyze = |request| {
            self.driver
                .analyze_for_editor(request, |db, program| {
                    Analysis::from_program(&self.driver, db, program)
                })
                .map_err(|error| error.to_string())
        };
        let analysis = analyze(request)?;
        if let Some(fallback) = fallback
            && !analysis.files.contains_key(&normalized(entry))
        {
            return analyze(fallback);
        }
        Ok(analysis)
    }
}

fn request_for(entry: &Path) -> Result<(CheckRequest, Option<CheckRequest>), String> {
    let Some(directory) = entry
        .ancestors()
        .skip(1)
        .find(|directory| directory.join("nia.toml").is_file())
    else {
        return Ok((CheckRequest::new(normalized(entry)), None));
    };
    let manifest = PackageManifest::read(directory).map_err(|error| error.to_string())?;
    let library = manifest
        .library_path
        .as_deref()
        .map(|path| directory.join(path));
    let artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| directory.join(&artifact.path) == entry);
    let mode = if artifact.is_some_and(|artifact| artifact.kind == ArtifactKind::Test) {
        CompilationMode::Test
    } else {
        CompilationMode::Normal
    };
    let mut modules = ModuleMap::new();
    if let Some(library) = &library
        && library != entry
    {
        modules.insert(
            manifest.package.name.clone(),
            SourcePath::new(normalized(library)),
        )?;
    }
    let request = |path| {
        CheckRequest::new(normalized(path))
            .with_compilation_mode(mode)
            .with_module_map(modules.clone())
    };
    let primary = library.as_deref().filter(|library| {
        artifact.is_none() && *library != entry && entry != directory.join("build.nia")
    });
    match primary {
        Some(library) => {
            let fallback = request(entry);
            Ok((
                request(library).with_module_map(ModuleMap::new()),
                Some(fallback),
            ))
        }
        None => Ok((request(entry), None)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Symbol {
    Global(GlobalDefId),
    Local(ModuleId, LocalId),
}

struct Occurrence {
    span: Span,
    symbol: Symbol,
    declaration: bool,
}

struct File {
    uri: Uri,
    text: Arc<str>,
    lines: Lines,
    occurrences: Vec<Occurrence>,
}

pub(crate) struct Analysis {
    files: BTreeMap<String, File>,
    definitions: HashMap<Symbol, (String, Span)>,
    diagnostics: Vec<nia_compiler_query::ProgramDiagnostic>,
}

impl Analysis {
    fn from_program(
        driver: &Driver,
        db: &CompilerDatabase,
        program: CheckedProgramAnalysis,
    ) -> nia_query::QueryResult<Self> {
        let mut result = Self {
            files: BTreeMap::new(),
            definitions: HashMap::new(),
            diagnostics: program.diagnostics,
        };
        let modules: HashMap<_, _> = program
            .modules
            .iter()
            .map(|module| (module.id, module))
            .collect();
        for module in &program.modules {
            let path = module.path.as_str();
            let Some(file) = program
                .graph
                .get(module.id)
                .and_then(|module| driver.sources().source_for_id(module.source_id))
            else {
                continue;
            };
            let Some(file_uri) = uri(Path::new(path)) else {
                continue;
            };
            let names: Vec<_> = tokenize_lossless(&file.text)
                .into_iter()
                .filter(|token| {
                    matches!(
                        token.kind,
                        LosslessTokenKind::Token(TokenKind::Ident | TokenKind::SelfValue)
                    )
                })
                .map(|token| token.span)
                .collect();
            let tokens_in = |span: Span| {
                let start = names.partition_point(|name| name.start < span.start);
                let end = names
                    .partition_point(|name| name.end <= span.end)
                    .max(start);
                &names[start..end]
            };
            let name_matches = |span: &Span, name| {
                SymbolId::from_stable_hash(stable_hash(&file.text[span.start..span.end])) == name
            };
            let mut occurrences = Vec::new();
            let mut declare = |symbol, span| {
                result.definitions.insert(symbol, (path.to_owned(), span));
                occurrences.push(Occurrence {
                    span,
                    symbol,
                    declaration: true,
                });
            };
            for (id, def) in module.defs.defs.iter() {
                if let Some(&span) = tokens_in(def.span)
                    .iter()
                    .find(|span| name_matches(span, def.name))
                {
                    declare(
                        Symbol::Global(GlobalDefId {
                            module_id: module.id,
                            def_id: id,
                        }),
                        span,
                    );
                }
            }
            for (id, local) in module.local_resolution.locals.iter() {
                if let Some(&span) = tokens_in(local.span).first() {
                    declare(Symbol::Local(module.id, id), span);
                }
            }
            let call_targets: HashMap<_, _> = module
                .semantic_facts
                .iter_node_resolved_calls()
                .filter_map(|(key, call)| match call {
                    ResolvedCall::Method { def_id, .. } => Some((key, *def_id)),
                    _ => None,
                })
                .collect();
            for (span, key) in db.source_origins(module.id)?.entries() {
                let names = tokens_in(span);
                let target = match module.semantic_uses.node_value_use(&key) {
                    Some(SemanticValueUse::Local(id)) => Some(Symbol::Local(module.id, id)),
                    Some(SemanticValueUse::Global(id)) => Some(Symbol::Global(id)),
                    None => module
                        .semantic_uses
                        .node_type_prefix(&key)
                        .map(Symbol::Global),
                };
                if let Some(symbol) = target
                    && let Some(&span) = names.last()
                {
                    occurrences.push(Occurrence {
                        span,
                        symbol,
                        declaration: false,
                    });
                }
                if let Some(id) = call_targets.get(&key)
                    && let Some(def) = modules
                        .get(&id.module_id)
                        .and_then(|module| module.defs.defs.get(id.def_id))
                    && let Some(&span) = names.iter().find(|span| name_matches(span, def.name))
                {
                    occurrences.push(Occurrence {
                        span,
                        symbol: Symbol::Global(*id),
                        declaration: false,
                    });
                }
            }
            occurrences.sort_by_key(|occurrence| (occurrence.span, !occurrence.declaration));
            result.files.insert(
                path.to_owned(),
                File {
                    uri: file_uri,
                    lines: Lines::new(&file.text),
                    text: file.text,
                    occurrences,
                },
            );
        }
        Ok(result)
    }

    fn symbol_at(&self, path: &Path, position: Position) -> Option<Symbol> {
        let file = self.files.get(&normalized(path))?;
        let offset = file.lines.offset(&file.text, position)?;
        file.occurrences
            .iter()
            .find(|item| item.span.start <= offset && offset < item.span.end)
            .map(|item| item.symbol)
    }

    pub(crate) fn definition(&self, path: &Path, position: Position) -> Option<Location> {
        let (source, span) = self.definitions.get(&self.symbol_at(path, position)?)?;
        let file = self.files.get(source)?;
        Some(Location::new(
            file.uri.clone(),
            file.lines.range(&file.text, *span),
        ))
    }

    pub(crate) fn hover(&self, path: &Path, position: Position) -> Option<lsp_types::Hover> {
        let (source, span) = self.definitions.get(&self.symbol_at(path, position)?)?;
        let file = self.files.get(source)?;
        let start = file.text[..span.start]
            .rfind(['\r', '\n'])
            .map_or(0, |index| index + 1);
        let line = file.text[start..].lines().next()?.trim();
        let header = line.split_once(" {").map_or(line, |(header, _)| header);
        Some(lsp_types::Hover {
            contents: lsp_types::HoverContents::Scalar(lsp_types::MarkedString::LanguageString(
                lsp_types::LanguageString {
                    language: "nia".into(),
                    value: header.into(),
                },
            )),
            range: None,
        })
    }

    pub(crate) fn references(
        &self,
        path: &Path,
        position: Position,
        declaration: bool,
    ) -> Vec<Location> {
        let Some(symbol) = self.symbol_at(path, position) else {
            return Vec::new();
        };
        let mut locations = Vec::new();
        for file in self.files.values() {
            let mut previous = None;
            for item in &file.occurrences {
                if item.symbol == symbol
                    && (declaration || !item.declaration)
                    && previous != Some(item.span)
                {
                    locations.push(Location::new(
                        file.uri.clone(),
                        file.lines.range(&file.text, item.span),
                    ));
                    previous = Some(item.span);
                }
            }
        }
        locations
    }

    pub(crate) fn diagnostics(&self, path: &Path, document: &Document) -> Vec<Diagnostic> {
        let path = normalized(path);
        self.diagnostics
            .iter()
            .filter(|item| item.path.as_str() == path)
            .map(|item| {
                let diagnostic = &item.diagnostic;
                let primary = diagnostic
                    .labels
                    .iter()
                    .find(|label| label.style == LabelStyle::Primary);
                let mut parts = vec![diagnostic.summary.as_str()];
                if let Some(message) = primary.and_then(|label| label.message.as_deref())
                    && message != diagnostic.summary
                {
                    parts.push(message);
                }
                parts.extend(diagnostic.notes.iter().map(String::as_str));
                parts.extend(diagnostic.help.iter().map(String::as_str));
                Diagnostic {
                    range: document.lines.range(
                        &document.text,
                        primary.map_or(Span::default(), |label| label.span),
                    ),
                    severity: Some(if item.is_error() {
                        DiagnosticSeverity::ERROR
                    } else {
                        DiagnosticSeverity::WARNING
                    }),
                    code: Some(NumberOrString::String(diagnostic.code.as_str().to_owned())),
                    source: Some("nia".to_owned()),
                    message: parts.join("\n"),
                    ..Diagnostic::default()
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_test_support::{TestWorkload, acquire_test_resources, test_dir, workspace_root};
    use nia_toolchain::ToolchainLayoutRequest;

    #[test]
    fn compiler_queries_follow_unsaved_cross_module_changes() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-cross-module");
        let main = root.join("main.nia");
        let child = root.join("math.nia");
        let main_text =
            "module math;\nfun f() i32 {\n    let local = math::amount();\n    local\n}\n";
        let child_text = "pub fun amount() i32 { 42 }\n";
        std::fs::write(&main, main_text).unwrap();
        std::fs::write(&child, child_text).unwrap();
        let mut documents: Documents = [(&main, main_text), (&child, child_text)]
            .into_iter()
            .map(|(path, text)| {
                (
                    path.clone(),
                    OpenDocument {
                        uri: uri(path).unwrap(),
                        document: Document::new(1, text.to_owned()),
                    },
                )
            })
            .collect();
        let toolchain = ToolchainLayout::resolve(ToolchainLayoutRequest::explicit(
            std::env::current_exe().unwrap(),
            workspace_root().join("lib"),
        ))
        .unwrap();
        let mut session = Session::new(Arc::new(toolchain));
        let first = session.analyze(&documents, &main).unwrap();
        assert!(
            first
                .diagnostics(&main, &documents[&main].document)
                .is_empty()
        );
        let imported = Position::new(2, 25);
        let target = first.definition(&main, imported).unwrap();
        assert_eq!(target.uri, documents[&child].uri);
        assert_eq!(target.range.start, Position::new(0, 8));
        assert_eq!(first.references(&main, imported, true).len(), 2);
        assert!(first.hover(&main, imported).is_some());
        let local = first.definition(&main, Position::new(3, 4)).unwrap();
        assert_eq!(local.range.start, Position::new(2, 8));

        documents.get_mut(&child).unwrap().document =
            Document::new(2, "\npub fun amount() bool { true }\n".into());
        let edited = session.analyze(&documents, &main).unwrap();
        assert!(
            !edited
                .diagnostics(&main, &documents[&main].document)
                .is_empty()
        );

        documents.remove(&child);
        let restored = session.analyze(&documents, &main).unwrap();
        assert!(
            restored
                .diagnostics(&main, &documents[&main].document)
                .is_empty()
        );
        assert_eq!(restored.definition(&main, imported), Some(target));
    }
}
