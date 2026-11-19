// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, select, tick, unbounded};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    Diagnostic, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    PublishDiagnosticsParams, SemanticTokens, SemanticTokensDelta, SemanticTokensDeltaParams,
    SemanticTokensFullOptions, SemanticTokensParams, SemanticTokensRangeParams, Uri,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    analysis::{self, Analysis, Documents, OpenDocument, Session},
    document::Document,
    highlight::{self, Resolved},
    rename::Rename,
    workspace::{CallHierarchyContext, Cancellation},
};

type Error = Box<dyn std::error::Error + Send + Sync>;
const SEMANTIC_FULL: &str = "textDocument/semanticTokens/full";
const SEMANTIC_DELTA: &str = "textDocument/semanticTokens/full/delta";
const SEMANTIC_RANGE: &str = "textDocument/semanticTokens/range";
const SEMANTIC_REFRESH: &str = "workspace/semanticTokens/refresh";
const COMPLETION: &str = "textDocument/completion";
const INLAY_HINT: &str = "textDocument/inlayHint";
const CALL_PREPARE: &str = "textDocument/prepareCallHierarchy";
const CALL_INCOMING: &str = "callHierarchy/incomingCalls";
const CALL_OUTGOING: &str = "callHierarchy/outgoingCalls";
const CODE_ACTION: &str = "textDocument/codeAction";
const FORMATTING: &str = "textDocument/formatting";
const RENAME: &str = "textDocument/rename";
const PREPARE_RENAME: &str = "textDocument/prepareRename";
const REFERENCES: &str = "textDocument/references";
const WORKSPACE_SYMBOL: &str = "workspace/symbol";
const IMPLEMENTATION: &str = "textDocument/implementation";
const WILL_RENAME_FILES: &str = "workspace/willRenameFiles";

pub(crate) fn run(toolchain: nia_toolchain::ToolchainLayout) -> Result<(), Error> {
    let (connection, io) = Connection::stdio();
    let (id, params) = connection.initialize_start()?;
    let params = decode::<lsp_types::InitializeParams>(params)?;
    #[allow(deprecated)]
    let folders: Vec<_> = params.workspace_folders.map_or_else(
        || {
            params
                .root_uri
                .into_iter()
                .filter_map(|uri| analysis::path(&uri))
                .collect()
        },
        |folders| {
            folders
                .into_iter()
                .filter_map(|folder| analysis::path(&folder.uri))
                .collect()
        },
    );
    let workspace = params.capabilities.workspace.unwrap_or_default();
    let file_operations = workspace.file_operations.unwrap_or_default();
    let mut file_rename = serde_json::Map::new();
    let rename_filters = json!({ "filters": [
        { "scheme": "file", "pattern": { "glob": "**/*.nia" } },
        { "scheme": "file", "pattern": { "glob": "**", "matches": "folder" } }
    ] });
    if file_operations.will_rename.unwrap_or_default() {
        file_rename.insert("willRename".into(), rename_filters.clone());
    }
    if file_operations.did_rename.unwrap_or_default() {
        file_rename.insert("didRename".into(), rename_filters);
    }
    let edit = workspace.workspace_edit.unwrap_or_default();
    let versioned_edits = edit.document_changes.unwrap_or_default();
    let resource_rename = versioned_edits
        && edit.resource_operations.is_some_and(|operations| {
            operations.contains(&lsp_types::ResourceOperationKind::Rename)
        });
    let watch_files = workspace
        .did_change_watched_files
        .and_then(|watched| watched.dynamic_registration)
        .unwrap_or_default();
    let semantic_refresh = workspace
        .semantic_tokens
        .and_then(|semantic| semantic.refresh_support)
        .unwrap_or_default();
    let client = params.capabilities.text_document.unwrap_or_default();
    let semantic_delta = client.semantic_tokens.is_some_and(|tokens| {
        matches!(
            tokens.requests.full,
            Some(SemanticTokensFullOptions::Delta { delta: Some(true) })
        )
    });
    let code_actions = client
        .code_action
        .and_then(|actions| actions.code_action_literal_support)
        .is_some();
    let hierarchical_symbols = client
        .document_symbol
        .and_then(|symbols| symbols.hierarchical_document_symbol_support)
        .unwrap_or_default();
    let folding = client.folding_range.unwrap_or_default();
    connection.initialize_finish(
        id,
        json!({
            "capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": { "openClose": true, "change": 2, "save": true },
                "definitionProvider": true,
                "typeDefinitionProvider": true,
                "implementationProvider": true,
                "referencesProvider": true,
                "documentHighlightProvider": true,
                "renameProvider": { "prepareProvider": true },
                "workspace": {
                    "workspaceFolders": { "supported": true, "changeNotifications": true },
                    "fileOperations": file_rename
                },
                "hoverProvider": true,
                "inlayHintProvider": true,
                "callHierarchyProvider": true,
                "documentSymbolProvider": true,
                "workspaceSymbolProvider": true,
                "foldingRangeProvider": true,
                "documentFormattingProvider": true,
                "codeActionProvider": if code_actions { json!({ "codeActionKinds": ["quickfix", "source.organizeImports"] }) } else { json!(false) },
                "semanticTokensProvider": { "legend": highlight::legend(), "full": { "delta": true }, "range": true },
                "completionProvider": { "triggerCharacters": [":", "."] },
                "signatureHelpProvider": { "triggerCharacters": ["(", ","], "retriggerCharacters": [","] }
            },
            "serverInfo": { "name": "nia-lsp", "version": nia_compat::COMPILER_VERSION },
        }),
    )?;
    if watch_files {
        connection.sender.send(
            Request::new(
                RequestId::from("nia-watch-files".to_owned()),
                "client/registerCapability".to_owned(),
                json!({ "registrations": [{
                    "id": "nia-watch-files",
                    "method": "workspace/didChangeWatchedFiles",
                    "registerOptions": { "watchers": [
                        { "globPattern": "**/*.nia" },
                        { "globPattern": "**/nia.toml" },
                        { "globPattern": "**/nia.lock" },
                        { "globPattern": "**/build-plan.bin" }
                    ]}
                }]}),
            )
            .into(),
        )?;
    }
    let (work_tx, work_rx) = unbounded();
    let (workspace_tx, workspace_rx) = unbounded();
    let (reply_tx, reply_rx) = unbounded();
    let toolchain = Arc::new(toolchain);
    let editor = toolchain.clone();
    let reply = reply_tx.clone();
    let worker = thread::spawn(move || analyze(editor, Lane::Editor, work_rx, reply));
    let workspace_worker =
        thread::spawn(move || analyze(toolchain, Lane::Workspace, workspace_rx, reply_tx));
    let mut server = Server {
        hierarchical_symbols,
        fold_limit: folding.range_limit.map(|limit| limit as usize),
        line_folding_only: folding.line_folding_only.unwrap_or_default(),
        semantic_refresh,
        semantic_delta,
        folders,
        versioned_edits,
        resource_rename,
        ..Server::default()
    };
    let result = server.serve(&connection, &work_tx, &workspace_tx, &reply_rx);
    server.cancel_active();
    drop(connection);
    drop(work_tx);
    drop(workspace_tx);
    worker.join().map_err(|_| "Nia analysis thread panicked")?;
    workspace_worker
        .join()
        .map_err(|_| "Nia workspace thread panicked")?;
    io.join()?;
    result
}

enum WorkerMessage {
    Analyze(Work),
    ReleaseAnalysis,
}

struct Work {
    revision: u64,
    disk_changes: DiskChanges,
    documents: Documents,
    path: PathBuf,
    request: Option<Request>,
    folders: Vec<PathBuf>,
    versioned_edits: bool,
    resource_rename: bool,
    cancel: Cancellation,
}

#[derive(Clone, Default)]
enum DiskChanges {
    #[default]
    None,
    Sources(Arc<BTreeSet<PathBuf>>),
    Configuration,
}

impl DiskChanges {
    fn merge(&mut self, changes: &Self) {
        match (self, changes) {
            (_, Self::None) | (Self::Configuration, _) => {}
            (Self::Sources(pending), Self::Sources(paths)) => {
                Arc::make_mut(pending).extend(paths.iter().cloned());
            }
            (pending, changes) => *pending = changes.clone(),
        }
    }
}

struct Reply {
    lane: Lane,
    revision: u64,
    path: PathBuf,
    publication: Option<Publication>,
    response: Option<QueryResponse>,
    reference_coverage: Option<String>,
}

enum QueryResponse {
    Value(Response),
    Semantic(Request),
}

impl QueryResponse {
    fn into_id(self) -> RequestId {
        match self {
            Self::Value(response) => response.id,
            Self::Semantic(request) => request.id,
        }
    }
}

struct Publication {
    diagnostics: Result<Vec<Diagnostic>, String>,
    highlights: Arc<Resolved>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lane {
    Editor,
    Workspace,
}

impl Lane {
    fn for_request(request: &Request) -> Self {
        if matches!(
            request.method.as_str(),
            REFERENCES
                | RENAME
                | WORKSPACE_SYMBOL
                | IMPLEMENTATION
                | WILL_RENAME_FILES
                | CALL_INCOMING
                | CALL_OUTGOING
        ) {
            Self::Workspace
        } else {
            Self::Editor
        }
    }
}

fn analyze(
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
    lane: Lane,
    queue: Receiver<WorkerMessage>,
    output: Sender<Reply>,
) {
    let mut session = Session::new(toolchain.clone());
    let mut completion_probe = None;
    let mut cached = None;
    for message in queue {
        let WorkerMessage::Analyze(mut work) = message else {
            cached = None;
            completion_probe = None;
            session.reload();
            nia_alloc::collect_unused();
            continue;
        };
        let changes = std::mem::take(&mut work.disk_changes);
        if !matches!(changes, DiskChanges::None) {
            cached = None;
            completion_probe = None;
            if let DiskChanges::Sources(paths) = changes {
                session.reload_changed_sources(&paths);
            } else {
                session.reload();
            }
        }
        if work.cancel.check().is_err() {
            let response = work.request.map(|request| {
                Response::new_err(
                    request.id,
                    ErrorCode::RequestCanceled as i32,
                    "Request canceled".into(),
                )
            });
            if output
                .send(Reply {
                    lane,
                    revision: work.revision,
                    path: work.path,
                    publication: None,
                    response: response.map(QueryResponse::Value),
                    reference_coverage: None,
                })
                .is_err()
            {
                break;
            }
            continue;
        }
        if let Some(request) = &work.request
            && matches!(
                request.method.as_str(),
                WORKSPACE_SYMBOL | WILL_RENAME_FILES | CALL_INCOMING | CALL_OUTGOING
            )
        {
            let request = work.request.take().expect("workspace request");
            let response = workspace_query(&mut session, &work, request);
            if output
                .send(Reply {
                    lane,
                    revision: work.revision,
                    path: work.path,
                    publication: None,
                    response: Some(QueryResponse::Value(response)),
                    reference_coverage: None,
                })
                .is_err()
            {
                break;
            }
            continue;
        }
        if !cached
            .as_ref()
            .is_some_and(|(revision, path, _)| (*revision, path) == (work.revision, &work.path))
            && let Some(response) = complete_declarations(&mut session, &mut work)
        {
            if output
                .send(Reply {
                    lane,
                    revision: work.revision,
                    path: work.path,
                    publication: None,
                    response: Some(QueryResponse::Value(response)),
                    reference_coverage: None,
                })
                .is_err()
            {
                break;
            }
            continue;
        }
        let key = (work.revision, &work.path);
        let current = if let Some((revision, path, current)) = &cached
            && (*revision, path) == key
        {
            Arc::clone(current)
        } else {
            let analysis =
                session.analyze_with_cancel(&work.documents, &work.path, Some(&work.cancel));
            let highlights = Arc::new(analysis.as_ref().map_or_else(
                |_| Resolved::new(),
                |analysis| {
                    if lane == Lane::Editor && work.cancel.check().is_ok() {
                        analysis.semantic_highlights(&work.path)
                    } else {
                        Resolved::new()
                    }
                },
            ));
            let current = Arc::new((analysis, highlights));
            if work.cancel.check().is_ok() {
                cached = Some((work.revision, work.path.clone(), current.clone()));
            }
            current
        };
        let (analysis, highlights) = current.as_ref();
        let document = &work.documents[&work.path].document;
        let publication = (lane == Lane::Editor).then(|| Publication {
            diagnostics: analysis
                .as_ref()
                .map(|analysis| analysis.diagnostics(&work.path, document))
                .map_err(Clone::clone),
            highlights: highlights.clone(),
        });
        let mut reference_coverage = None;
        let semantic = work.request.take_if(|request| {
            matches!(
                request.method.as_str(),
                SEMANTIC_FULL | SEMANTIC_DELTA | SEMANTIC_RANGE
            )
        });
        let response = work
            .request
            .take()
            .map(|request| {
                if work.cancel.check().is_err() {
                    return Response::new_err(
                        request.id,
                        ErrorCode::RequestCanceled as i32,
                        "Request canceled".into(),
                    );
                }
                match analysis {
                    Ok(analysis) if request.method == RENAME => {
                        let params = match decode::<lsp_types::RenameParams>(request.params) {
                            Ok(params) => params,
                            Err(error) => {
                                return Response::new_err(
                                    request.id,
                                    ErrorCode::InvalidParams as i32,
                                    error.to_string(),
                                );
                            }
                        };
                        let result = session.rename(
                            analysis,
                            &work.documents,
                            Rename {
                                path: &work.path,
                                position: params.text_document_position.position,
                                new_name: &params.new_name,
                                folders: &work.folders,
                                versioned: work.versioned_edits,
                                resource_rename: work.resource_rename,
                            },
                            &work.cancel,
                        );
                        match result {
                            Ok(edit) => Response::new_ok(request.id, edit),
                            Err(error) => Response::new_err(
                                request.id,
                                ErrorCode::RequestFailed as i32,
                                error,
                            ),
                        }
                    }
                    Ok(analysis) if request.method == IMPLEMENTATION => {
                        let params =
                            match decode::<lsp_types::TextDocumentPositionParams>(request.params) {
                                Ok(params) => params,
                                Err(error) => {
                                    return Response::new_err(
                                        request.id,
                                        ErrorCode::InvalidParams as i32,
                                        error.to_string(),
                                    );
                                }
                            };
                        let result = session
                            .workspace_index(
                                &work.path,
                                &work.documents,
                                &work.folders,
                                &work.cancel,
                            )
                            .and_then(|index| {
                                index.implementations(
                                    analysis,
                                    &work.path,
                                    params.position,
                                    &work.cancel,
                                )
                            });
                        match result {
                            Ok(locations) => Response::new_ok(request.id, locations),
                            Err(error) => Response::new_err(
                                request.id,
                                ErrorCode::RequestFailed as i32,
                                error,
                            ),
                        }
                    }
                    Ok(analysis) if request.method == REFERENCES => {
                        match decode::<lsp_types::ReferenceParams>(request.params) {
                            Ok(params) => {
                                let position = params.text_document_position.position;
                                let include_declaration = params.context.include_declaration;
                                if analysis
                                    .reference_symbol(&work.path, position)
                                    .is_none_or(|symbol| !analysis.rename_workspace_symbol(symbol))
                                {
                                    return Response::new_ok(
                                        request.id,
                                        analysis.references(
                                            &work.path,
                                            position,
                                            include_declaration,
                                        ),
                                    );
                                }
                                let result = session
                                    .workspace_index(
                                        &work.path,
                                        &work.documents,
                                        &work.folders,
                                        &work.cancel,
                                    )
                                    .and_then(|index| {
                                        let references = index.references(
                                            analysis,
                                            &work.path,
                                            position,
                                            include_declaration,
                                            &work.cancel,
                                        )?;
                                        reference_coverage = index.reference_coverage_warning();
                                        Ok(references)
                                    });
                                match result {
                                    Ok(references) => Response::new_ok(request.id, references),
                                    Err(error) => Response::new_err(
                                        request.id,
                                        ErrorCode::RequestFailed as i32,
                                        error,
                                    ),
                                }
                            }
                            Err(error) => Response::new_err(
                                request.id,
                                ErrorCode::InvalidParams as i32,
                                error.to_string(),
                            ),
                        }
                    }
                    Ok(analysis) => {
                        let probe = matches!(request.method.as_str(), COMPLETION | CODE_ACTION)
                            .then(|| {
                                completion_probe
                                    .get_or_insert_with(|| Session::new(toolchain.clone()))
                            });
                        query(analysis, &work, request, &mut session, probe)
                    }
                    Err(error) => Response::new_err(
                        request.id,
                        ErrorCode::RequestFailed as i32,
                        error.clone(),
                    ),
                }
            })
            .map(QueryResponse::Value)
            .or_else(|| semantic.map(QueryResponse::Semantic));
        if output
            .send(Reply {
                lane,
                revision: work.revision,
                path: work.path,
                publication,
                response,
                reference_coverage,
            })
            .is_err()
        {
            break;
        }
    }
}

fn complete_declarations(session: &mut Session, work: &mut Work) -> Option<Response> {
    let request = work
        .request
        .as_ref()
        .filter(|request| request.method == COMPLETION)?;
    let params = decode::<lsp_types::CompletionParams>(request.params.clone()).ok()?;
    let position = params.text_document_position.position;
    let doc = &work.documents.get(&work.path)?.document;
    let offset = doc.lines.offset(&doc.text, position)?;
    if crate::editing::MemberCompletion::at(&doc.text, offset).is_some() {
        return None;
    }
    let result = session.complete_names(&work.documents, &work.path, position, &work.cancel);
    let items = result.ok().flatten()?;
    let request = work.request.take().expect("completion request");
    Some(Response::new_ok(request.id, items))
}

fn workspace_query(session: &mut Session, work: &Work, request: Request) -> Response {
    if matches!(request.method.as_str(), CALL_INCOMING | CALL_OUTGOING) {
        return expand_calls(session, work, request);
    }
    let result = if request.method == WILL_RENAME_FILES {
        decode::<lsp_types::RenameFilesParams>(request.params).map(|params| {
            session
                .will_rename_files(
                    params.files,
                    &work.documents,
                    &work.folders,
                    work.versioned_edits,
                    &work.cancel,
                )
                .map(|edit| json!(edit))
        })
    } else {
        decode::<lsp_types::WorkspaceSymbolParams>(request.params).map(|params| {
            session
                .index_workspace(None, &work.documents, &work.folders, &work.cancel)
                .and_then(|index| index.symbols(&params.query, &work.cancel))
                .map(|symbols| json!(symbols))
        })
    };
    match result {
        Ok(Ok(result)) => Response::new_ok(request.id, result),
        Ok(Err(error)) => Response::new_err(request.id, ErrorCode::RequestFailed as i32, error),
        Err(error) => Response::new_err(
            request.id,
            ErrorCode::InvalidParams as i32,
            error.to_string(),
        ),
    }
}

fn expand_calls(session: &mut Session, work: &Work, request: Request) -> Response {
    let incoming = request.method == CALL_INCOMING;
    let params = if incoming {
        decode::<CallHierarchyIncomingCallsParams>(request.params).map(|params| params.item)
    } else {
        decode::<CallHierarchyOutgoingCallsParams>(request.params).map(|params| params.item)
    };
    let (item, context) = match params.and_then(|mut item: CallHierarchyItem| {
        let context = decode::<CallHierarchyContext>(item.data.take().unwrap_or_default())?;
        Ok((item, context))
    }) {
        Ok(value) => value,
        Err(error) => {
            return Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            );
        }
    };
    if context.revision != work.revision {
        return Response::new_err(
            request.id,
            ErrorCode::ContentModified as i32,
            "Document changed since the call hierarchy was prepared".into(),
        );
    }
    let Some(anchor) = analysis::path(&context.anchor) else {
        return Response::new_err(
            request.id,
            ErrorCode::InvalidParams as i32,
            "Invalid call hierarchy anchor".into(),
        );
    };
    let calls = session
        .workspace_index(&anchor, &work.documents, &work.folders, &work.cancel)
        .and_then(|index| index.call_relations(&item, &context, incoming, &work.cancel));
    match calls {
        Ok(calls) if incoming => Response::new_ok(
            request.id,
            calls
                .into_iter()
                .map(|(from, from_ranges)| CallHierarchyIncomingCall { from, from_ranges })
                .collect::<Vec<_>>(),
        ),
        Ok(calls) => Response::new_ok(
            request.id,
            calls
                .into_iter()
                .map(|(to, from_ranges)| CallHierarchyOutgoingCall { to, from_ranges })
                .collect::<Vec<_>>(),
        ),
        Err(error) => Response::new_err(request.id, ErrorCode::RequestFailed as i32, error),
    }
}

fn query(
    analysis: &Analysis,
    work: &Work,
    request: Request,
    session: &mut Session,
    probe: Option<&mut Session>,
) -> Response {
    let path = &work.path;
    let Request { id, method, params } = request;
    let result = match method.as_str() {
        "textDocument/definition" => {
            decode::<lsp_types::TextDocumentPositionParams>(params).map(|params| {
                json!(analysis.definition(path, params.position).map(|location| {
                    session.generated_definition(analysis, &work.documents, location)
                }))
            })
        }
        "textDocument/typeDefinition" => decode::<lsp_types::TextDocumentPositionParams>(params)
            .map(|params| {
                json!(
                    analysis
                        .type_definition(path, params.position)
                        .map(|location| {
                            session.generated_definition(analysis, &work.documents, location)
                        })
                )
            }),
        "textDocument/hover" => decode::<lsp_types::TextDocumentPositionParams>(params)
            .map(|params| json!(analysis.hover(path, params.position))),
        "textDocument/documentHighlight" => decode::<lsp_types::TextDocumentPositionParams>(params)
            .map(|params| json!(analysis.document_highlights(path, params.position))),
        PREPARE_RENAME => decode::<lsp_types::TextDocumentPositionParams>(params).map(|params| {
            json!(analysis.prepare_rename(path, params.position, work.resource_rename))
        }),
        COMPLETION => decode::<lsp_types::CompletionParams>(params).map(|params| {
            json!(probe.expect("completion probe").complete(
                analysis,
                &work.documents,
                path,
                params.text_document_position.position,
                &work.cancel,
            ))
        }),
        INLAY_HINT => decode::<lsp_types::InlayHintParams>(params)
            .map(|params| json!(analysis.inlay_hints(path, params.range, &work.cancel))),
        CALL_PREPARE => decode::<CallHierarchyPrepareParams>(params).map(|params| {
            let item = analysis.prepare_call_hierarchy(
                path,
                params.text_document_position_params.position,
                work.revision,
            );
            json!(item.into_iter().collect::<Vec<_>>())
        }),
        CODE_ACTION => decode::<lsp_types::CodeActionParams>(params).map(|params| {
            json!(probe.expect("code action probe").code_actions(
                analysis,
                &work.documents,
                path,
                &params,
                work.versioned_edits,
                &work.cancel,
            ))
        }),
        "textDocument/signatureHelp" => {
            decode::<lsp_types::SignatureHelpParams>(params).map(|params| {
                json!(analysis.signature_help(path, params.text_document_position_params.position))
            })
        }
        _ => return Response::new_err(id, ErrorCode::MethodNotFound as i32, method),
    };
    match result {
        Ok(value) => Response::new_ok(id, value),
        Err(error) => Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string()),
    }
}

fn decode<T: DeserializeOwned>(value: Value) -> serde_json::Result<T> {
    serde_json::from_value(value)
}

#[derive(Default)]
struct Server {
    folders: Vec<PathBuf>,
    versioned_edits: bool,
    resource_rename: bool,
    hierarchical_symbols: bool,
    fold_limit: Option<usize>,
    line_folding_only: bool,
    semantic_refresh: bool,
    semantic_delta: bool,
    token_results: HashMap<PathBuf, SemanticTokens>,
    token_sequence: u64,
    highlights: HashMap<PathBuf, Arc<Resolved>>,
    syntax_sent: BTreeSet<PathBuf>,
    refresh_needed: bool,
    refresh_request: Option<RequestId>,
    refresh_sequence: u64,
    documents: Documents,
    dirty: BTreeSet<PathBuf>,
    pending: VecDeque<(PathBuf, u64, Request)>,
    editor: Option<Active>,
    workspace: Option<Active>,
    revision: u64,
    editor_disk_changes: DiskChanges,
    workspace_disk_changes: DiskChanges,
    release_analysis: bool,
}

struct Active {
    request: Option<RequestId>,
    cancel: Cancellation,
}

impl Server {
    fn semantic_tokens(&mut self, path: &std::path::Path, request: Request) -> Response {
        let params = match request.method.as_str() {
            SEMANTIC_RANGE => decode::<SemanticTokensRangeParams>(request.params)
                .map(|params| (Some(params.range), None)),
            SEMANTIC_DELTA => decode::<SemanticTokensDeltaParams>(request.params)
                .map(|params| (None, Some(params.previous_result_id))),
            _ => decode::<SemanticTokensParams>(request.params).map(|_| (None, None)),
        };
        let (range, previous) = match params {
            Ok(params) => params,
            Err(error) => {
                return Response::new_err(
                    request.id,
                    ErrorCode::InvalidParams as i32,
                    error.to_string(),
                );
            }
        };
        let doc = &self.documents[path].document;
        let span = if let Some(range) = range {
            let start = doc.lines.offset(&doc.text, range.start);
            let end = doc.lines.offset(&doc.text, range.end);
            match (start, end) {
                (Some(start), Some(end)) if start <= end => Some(nia_span::Span::new(start, end)),
                _ => {
                    return Response::new_err(
                        request.id,
                        ErrorCode::InvalidParams as i32,
                        "Invalid semantic tokens range".into(),
                    );
                }
            }
        } else {
            None
        };
        let resolved = self.highlights.get(path);
        if resolved.is_none() && self.semantic_refresh {
            self.syntax_sent.insert(path.to_owned());
        }
        let mut tokens = doc.semantic_tokens(resolved.map(Arc::as_ref), span);
        if span.is_some() || !self.semantic_delta {
            return Response::new_ok(request.id, tokens);
        }
        self.token_sequence += 1;
        let result_id = self.token_sequence.to_string();
        let edits = previous.and_then(|previous| {
            let base = self.token_results.get(path)?;
            if base.result_id.as_deref()? != previous {
                return None;
            }
            highlight::token_edits(&base.data, &tokens.data)
        });
        tokens.result_id = Some(result_id.clone());
        let response = if let Some(edits) = edits {
            Response::new_ok(
                request.id,
                SemanticTokensDelta {
                    result_id: Some(result_id),
                    edits,
                },
            )
        } else {
            Response::new_ok(request.id, &tokens)
        };
        self.token_results.insert(path.to_owned(), tokens);
        response
    }

    fn serve(
        &mut self,
        connection: &Connection,
        work: &Sender<WorkerMessage>,
        workspace: &Sender<WorkerMessage>,
        replies: &Receiver<Reply>,
    ) -> Result<(), Error> {
        let idle = tick(Duration::from_millis(90));
        loop {
            select! {
                recv(connection.receiver) -> message => match message? {
                    Message::Request(request) => {
                        if request.method == "shutdown" {
                            self.cancel_active();
                            finish_shutdown(connection, request)?;
                            break;
                        }
                        self.request(connection, request)?;
                        self.schedule(connection, work, workspace, false)?;
                    }
                    Message::Notification(notification) => {
                        if notification.method == "exit" {
                            break;
                        }
                        self.notification(connection, notification)?;
                    }
                    Message::Response(response) => {
                        if self.refresh_request.as_ref() == Some(&response.id) {
                            self.refresh_request = None;
                        }
                    }
                },
                recv(replies) -> reply => {
                    self.reply(connection, reply?)?;
                    self.schedule(connection, work, workspace, false)?;
                },
                recv(idle) -> _ => {
                    self.schedule(connection, work, workspace, true)?;
                    self.refresh_highlights(connection)?;
                },
            }
        }
        Ok(())
    }

    fn request(&mut self, connection: &Connection, request: Request) -> Result<(), Error> {
        if matches!(
            request.method.as_str(),
            WORKSPACE_SYMBOL | WILL_RENAME_FILES | CALL_INCOMING | CALL_OUTGOING
        ) {
            self.pending
                .push_back((PathBuf::new(), self.revision, request));
            return Ok(());
        }
        if !matches!(
            request.method.as_str(),
            "textDocument/definition"
                | "textDocument/typeDefinition"
                | IMPLEMENTATION
                | REFERENCES
                | "textDocument/documentHighlight"
                | RENAME
                | PREPARE_RENAME
                | "textDocument/hover"
                | COMPLETION
                | INLAY_HINT
                | CALL_PREPARE
                | CODE_ACTION
                | FORMATTING
                | "textDocument/signatureHelp"
                | "textDocument/documentSymbol"
                | "textDocument/foldingRange"
                | SEMANTIC_FULL
                | SEMANTIC_DELTA
                | SEMANTIC_RANGE
        ) {
            connection.sender.send(
                Response::new_err(request.id, ErrorCode::MethodNotFound as i32, request.method)
                    .into(),
            )?;
            return Ok(());
        }
        let path = request
            .params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .and_then(|text| text.parse::<Uri>().ok())
            .and_then(|uri| analysis::path(&uri));
        let Some((path, open)) =
            path.and_then(|path| self.documents.get(&path).map(|open| (path, open)))
        else {
            connection.sender.send(
                Response::new_err(
                    request.id,
                    ErrorCode::InvalidParams as i32,
                    "Nia source document is not open".into(),
                )
                .into(),
            )?;
            return Ok(());
        };
        if request.method == COMPLETION
            && let Some(active) = &self.editor
            && active.request.is_none()
        {
            active.cancel.cancel();
        }
        if matches!(
            request.method.as_str(),
            SEMANTIC_FULL | SEMANTIC_DELTA | SEMANTIC_RANGE
        ) {
            if self.highlights.contains_key(&path) || self.semantic_refresh {
                connection
                    .sender
                    .send(self.semantic_tokens(&path, request).into())?;
            } else {
                self.pending.push_back((path, self.revision, request));
            }
            return Ok(());
        }
        if request.method == FORMATTING {
            let response = match decode::<lsp_types::DocumentFormattingParams>(request.params) {
                Ok(params) => match open.document.format(params.options) {
                    Ok(edits) => Response::new_ok(request.id, edits),
                    Err(error) => {
                        Response::new_err(request.id, ErrorCode::RequestFailed as i32, error)
                    }
                },
                Err(error) => Response::new_err(
                    request.id,
                    ErrorCode::InvalidParams as i32,
                    error.to_string(),
                ),
            };
            connection.sender.send(response.into())?;
            return Ok(());
        }
        let value = match request.method.as_str() {
            "textDocument/documentSymbol" => {
                json!(open.document.symbols(&open.uri, self.hierarchical_symbols))
            }
            "textDocument/foldingRange" => {
                json!(
                    open.document
                        .folding_ranges(self.line_folding_only, self.fold_limit)
                )
            }
            _ => {
                self.pending.push_back((path, self.revision, request));
                return Ok(());
            }
        };
        connection
            .sender
            .send(Response::new_ok(request.id, value).into())?;
        Ok(())
    }

    fn notification(
        &mut self,
        connection: &Connection,
        notification: Notification,
    ) -> Result<(), Error> {
        let result = match notification.method.as_str() {
            "textDocument/didOpen" => {
                decode::<DidOpenTextDocumentParams>(notification.params).map(|params| {
                    let document = params.text_document;
                    if let Some(path) = analysis::path(&document.uri) {
                        self.documents.insert(
                            path.clone(),
                            OpenDocument {
                                uri: document.uri,
                                document: Document::new(document.version, document.text),
                            },
                        );
                        self.token_results.remove(&path);
                        self.invalidate();
                    }
                })
            }
            "textDocument/didChange" => decode::<DidChangeTextDocumentParams>(notification.params)
                .map(|params| {
                    if let Some(path) = analysis::path(&params.text_document.uri)
                        && let Some(open) = self.documents.get_mut(&path)
                        && let Some(changed) = open
                            .document
                            .changed(params.text_document.version, params.content_changes)
                    {
                        open.document = changed;
                        self.invalidate();
                    }
                }),
            "textDocument/didSave" => {
                decode::<DidSaveTextDocumentParams>(notification.params).map(|params| {
                    if let Some(path) = analysis::path(&params.text_document.uri) {
                        self.disk_changed(DiskChanges::Sources(Arc::new(BTreeSet::from([path]))));
                    }
                })
            }
            "workspace/didRenameFiles" => {
                decode::<lsp_types::RenameFilesParams>(notification.params)
                    .map(|_| self.disk_changed(DiskChanges::Configuration))
            }
            "workspace/didChangeWatchedFiles" => {
                decode::<DidChangeWatchedFilesParams>(notification.params).map(|params| {
                    let mut sources = BTreeSet::new();
                    for path in params
                        .changes
                        .iter()
                        .filter_map(|event| analysis::path(&event.uri))
                    {
                        if path.file_name().is_some_and(|name| {
                            name == "nia.toml" || name == "nia.lock" || name == "build-plan.bin"
                        }) {
                            self.disk_changed(DiskChanges::Configuration);
                            return;
                        }
                        if path.extension().is_some_and(|extension| extension == "nia") {
                            sources.insert(path);
                        }
                    }
                    if !sources.is_empty() {
                        self.disk_changed(DiskChanges::Sources(Arc::new(sources)));
                    }
                })
            }
            "textDocument/didClose" => {
                match decode::<DidCloseTextDocumentParams>(notification.params) {
                    Ok(params) => {
                        let uri = params.text_document.uri;
                        if let Some(path) = analysis::path(&uri)
                            && self.documents.remove(&path).is_some()
                        {
                            self.token_results.remove(&path);
                            publish(connection, uri, None, Vec::new())?;
                            self.invalidate();
                            self.release_analysis |= self.documents.is_empty();
                        }
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
            "workspace/didChangeWorkspaceFolders" => decode::<
                lsp_types::DidChangeWorkspaceFoldersParams,
            >(notification.params)
            .map(|params| {
                for folder in params.event.removed {
                    if let Some(path) = analysis::path(&folder.uri) {
                        self.folders.retain(|folder| *folder != path);
                    }
                }
                self.folders.extend(
                    params
                        .event
                        .added
                        .into_iter()
                        .filter_map(|folder| analysis::path(&folder.uri)),
                );
                self.invalidate();
                self.token_results.clear();
                self.release_analysis = true;
            }),
            "$/cancelRequest" => match decode::<Cancel>(notification.params) {
                Ok(params) => {
                    for active in [&mut self.editor, &mut self.workspace]
                        .into_iter()
                        .flatten()
                    {
                        if active.request.as_ref() == Some(&params.id) {
                            active.cancel.cancel();
                        }
                    }
                    if let Some(index) = self
                        .pending
                        .iter()
                        .position(|(_, _, request)| request.id == params.id)
                    {
                        self.pending.remove(index);
                        connection.sender.send(
                            Response::new_err(
                                params.id,
                                ErrorCode::RequestCanceled as i32,
                                "Request canceled".into(),
                            )
                            .into(),
                        )?;
                    }
                    Ok(())
                }
                Err(error) => Err(error),
            },
            _ => Ok(()),
        };
        if let Err(error) = result {
            eprintln!("{}: {error}", notification.method);
        }
        Ok(())
    }

    fn invalidate(&mut self) {
        self.cancel_active();
        self.revision += 1;
        self.dirty = self.documents.keys().cloned().collect();
        self.highlights.clear();
        self.syntax_sent.clear();
        self.refresh_needed = false;
    }

    fn cancel_active(&self) {
        for active in [&self.editor, &self.workspace].into_iter().flatten() {
            active.cancel.cancel();
        }
    }

    fn disk_changed(&mut self, changes: DiskChanges) {
        self.editor_disk_changes.merge(&changes);
        self.workspace_disk_changes.merge(&changes);
        self.invalidate();
        self.refresh_needed = self.semantic_refresh && !self.documents.is_empty();
    }

    fn refresh_highlights(&mut self, connection: &Connection) -> Result<(), Error> {
        if self.refresh_needed && self.refresh_request.is_none() {
            self.refresh_sequence += 1;
            let id = RequestId::from(format!("nia-semantic-refresh-{}", self.refresh_sequence));
            connection
                .sender
                .send(Request::new(id.clone(), SEMANTIC_REFRESH.to_owned(), ()).into())?;
            self.refresh_request = Some(id);
            self.refresh_needed = false;
        }
        Ok(())
    }

    fn schedule(
        &mut self,
        connection: &Connection,
        editor: &Sender<WorkerMessage>,
        workspace: &Sender<WorkerMessage>,
        diagnostics: bool,
    ) -> Result<(), Error> {
        if std::mem::take(&mut self.release_analysis) {
            self.editor_disk_changes = DiskChanges::None;
            self.workspace_disk_changes = DiskChanges::None;
            editor.send(WorkerMessage::ReleaseAnalysis)?;
            workspace.send(WorkerMessage::ReleaseAnalysis)?;
        }
        for (lane, active, worker, disk_changes) in [
            (
                Lane::Editor,
                &mut self.editor,
                editor,
                &mut self.editor_disk_changes,
            ),
            (
                Lane::Workspace,
                &mut self.workspace,
                workspace,
                &mut self.workspace_disk_changes,
            ),
        ] {
            if active.is_some() {
                continue;
            }
            let work = loop {
                if let Some(index) = self
                    .pending
                    .iter()
                    .position(|(_, _, request)| Lane::for_request(request) == lane)
                {
                    let (path, revision, request) =
                        self.pending.remove(index).expect("pending request");
                    if revision == self.revision {
                        break Some((path, Some(request)));
                    }
                    connection.sender.send(
                        Response::new_err(
                            request.id,
                            ErrorCode::ContentModified as i32,
                            "Document changed before analysis".into(),
                        )
                        .into(),
                    )?;
                } else if diagnostics
                    && lane == Lane::Editor
                    && let Some(path) = self.dirty.pop_first()
                {
                    break Some((path, None));
                } else {
                    break None;
                }
            };
            let Some((path, request)) = work else {
                continue;
            };
            let cancel = Cancellation::default();
            *active = Some(Active {
                request: request.as_ref().map(|request| request.id.clone()),
                cancel: cancel.clone(),
            });
            worker.send(WorkerMessage::Analyze(Work {
                revision: self.revision,
                disk_changes: std::mem::take(disk_changes),
                documents: self.documents.clone(),
                path,
                request,
                folders: self.folders.clone(),
                versioned_edits: self.versioned_edits,
                resource_rename: self.resource_rename,
                cancel,
            }))?;
        }
        Ok(())
    }

    fn reply(&mut self, connection: &Connection, reply: Reply) -> Result<(), Error> {
        let active = match reply.lane {
            Lane::Editor => self.editor.take(),
            Lane::Workspace => self.workspace.take(),
        };
        let canceled = active
            .as_ref()
            .is_some_and(|active| active.cancel.check().is_err());
        if reply.lane == Lane::Editor
            && canceled
            && reply.revision == self.revision
            && active.is_some_and(|active| active.request.is_none())
            && self.documents.contains_key(&reply.path)
        {
            self.dirty.insert(reply.path.clone());
        }
        if reply.lane == Lane::Editor
            && !canceled
            && reply.revision == self.revision
            && let Some(open) = self.documents.get(&reply.path)
            && let Some(publication) = reply.publication
        {
            self.dirty.remove(&reply.path);
            if self.syntax_sent.remove(&reply.path)
                && open.document.has_refinements(&publication.highlights)
            {
                self.refresh_needed = true;
            }
            self.highlights
                .insert(reply.path.clone(), publication.highlights);
            let diagnostics = publication.diagnostics.unwrap_or_else(|error| {
                vec![Diagnostic {
                    source: Some("nia-lsp".into()),
                    severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                    message: error,
                    ..Diagnostic::default()
                }]
            });
            publish(
                connection,
                open.uri.clone(),
                Some(open.document.version),
                diagnostics,
            )?;
        }
        if let Some(response) = reply.response {
            let code = if reply.revision != self.revision {
                Some((
                    ErrorCode::ContentModified,
                    "Document changed during analysis",
                ))
            } else if canceled {
                Some((ErrorCode::RequestCanceled, "Request canceled"))
            } else {
                None
            };
            let response = if let Some((code, message)) = code {
                Response::new_err(response.into_id(), code as i32, message.into())
            } else {
                if let Some(message) = reply.reference_coverage {
                    connection.sender.send(
                        Notification::new(
                            "window/logMessage".into(),
                            lsp_types::LogMessageParams {
                                typ: lsp_types::MessageType::WARNING,
                                message,
                            },
                        )
                        .into(),
                    )?;
                }
                match response {
                    QueryResponse::Value(response) => response,
                    QueryResponse::Semantic(request) => self.semantic_tokens(&reply.path, request),
                }
            };
            connection.sender.send(response.into())?;
        }
        Ok(())
    }
}

fn finish_shutdown(connection: &Connection, request: Request) -> Result<(), Error> {
    connection
        .sender
        .send(Response::new_ok(request.id, ()).into())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match connection.receiver.recv_deadline(deadline)? {
            Message::Notification(notification) if notification.method == "exit" => return Ok(()),
            Message::Request(request) => connection.sender.send(
                Response::new_err(
                    request.id,
                    ErrorCode::InvalidRequest as i32,
                    "Nia language server is shutting down".into(),
                )
                .into(),
            )?,
            Message::Notification(_) | Message::Response(_) => {}
        }
    }
}

#[derive(serde::Deserialize)]
struct Cancel {
    id: RequestId,
}

fn publish(
    connection: &Connection,
    uri: Uri,
    version: Option<i32>,
    diagnostics: Vec<Diagnostic>,
) -> Result<(), Error> {
    connection.sender.send(
        Notification::new(
            "textDocument/publishDiagnostics".into(),
            PublishDiagnosticsParams::new(uri, diagnostics, version),
        )
        .into(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_waits_for_exit_after_an_outstanding_refresh_response() {
        let (server, client) = Connection::memory();
        client
            .sender
            .send(Response::new_ok("nia-semantic-refresh-1".to_owned().into(), ()).into())
            .unwrap();
        client
            .sender
            .send(Notification::new("exit".into(), ()).into())
            .unwrap();
        finish_shutdown(&server, Request::new(1.into(), "shutdown".into(), ())).unwrap();
        let Message::Response(response) = client.receiver.try_recv().unwrap() else {
            panic!("expected a shutdown response");
        };
        assert_eq!(response.id, 1.into());
        assert!(response.response_result.is_ok());
    }

    #[test]
    fn published_build_plan_invalidates_disk_analysis() {
        let (connection, _client) = Connection::memory();
        let mut server = Server::default();
        let path = std::env::current_dir()
            .unwrap()
            .join("build/out/build-plan.bin");
        let uri = analysis::uri(&path).unwrap();
        server
            .notification(
                &connection,
                Notification::new(
                    "workspace/didChangeWatchedFiles".into(),
                    json!({"changes": [{"uri": uri, "type": 2}]}),
                ),
            )
            .unwrap();
        assert!(matches!(
            server.editor_disk_changes,
            DiskChanges::Configuration
        ));
        assert!(matches!(
            server.workspace_disk_changes,
            DiskChanges::Configuration
        ));
    }

    #[test]
    fn closing_the_last_document_releases_analysis_and_discards_inflight_results() {
        let (connection, client) = Connection::memory();
        let (editor, editor_work) = unbounded();
        let (workspace, workspace_work) = unbounded();
        let mut server = Server::default();
        let path = std::env::current_dir().unwrap().join("root.nia");
        let uri = analysis::uri(&path).unwrap();
        server
            .notification(
                &connection,
                Notification::new(
                    "textDocument/didOpen".into(),
                    json!({
                        "textDocument": {"uri": uri, "version": 1, "languageId": "nia", "text": ""}
                    }),
                ),
            )
            .unwrap();
        let path = analysis::path(&uri).unwrap();
        server.pending.push_back((
            path.clone(),
            server.revision,
            Request::new(1.into(), COMPLETION.into(), ()),
        ));
        server
            .schedule(&connection, &editor, &workspace, false)
            .unwrap();
        let WorkerMessage::Analyze(active) = editor_work.try_recv().unwrap() else {
            panic!("expected editor analysis");
        };
        server
            .notification(
                &connection,
                Notification::new(
                    "textDocument/didClose".into(),
                    json!({
                        "textDocument": {"uri": uri}
                    }),
                ),
            )
            .unwrap();
        assert!(active.cancel.check().is_err());
        assert!(matches!(
            client.receiver.try_recv().unwrap(),
            Message::Notification(_)
        ));
        server
            .schedule(&connection, &editor, &workspace, false)
            .unwrap();
        for work in [&editor_work, &workspace_work] {
            assert!(matches!(
                work.try_recv().unwrap(),
                WorkerMessage::ReleaseAnalysis
            ));
        }
        server
            .reply(
                &connection,
                Reply {
                    lane: Lane::Editor,
                    revision: active.revision,
                    path,
                    publication: Some(Publication {
                        diagnostics: Ok(Vec::new()),
                        highlights: Arc::default(),
                    }),
                    response: Some(QueryResponse::Value(Response::new_ok(1.into(), ()))),
                    reference_coverage: None,
                },
            )
            .unwrap();
        let Message::Response(response) = client.receiver.try_recv().unwrap() else {
            panic!("expected failure for the closed document");
        };
        assert_eq!(response.id, 1.into());
        assert_eq!(
            response.response_result.unwrap_err().code,
            ErrorCode::ContentModified as i32
        );
        assert!(server.highlights.is_empty());
    }

    #[test]
    fn completion_reschedules_interrupted_diagnostics_for_the_same_revision() {
        let (connection, client) = Connection::memory();
        let (editor, editor_work) = unbounded();
        let (workspace, _) = unbounded();
        let mut server = Server::default();
        let uri = analysis::uri(&std::env::current_dir().unwrap().join("root.nia")).unwrap();
        server
            .notification(
                &connection,
                Notification::new(
                    "textDocument/didOpen".into(),
                    json!({
                        "textDocument": {"uri": uri, "version": 1, "languageId": "nia", "text": ""}
                    }),
                ),
            )
            .unwrap();
        server
            .schedule(&connection, &editor, &workspace, true)
            .unwrap();
        let WorkerMessage::Analyze(diagnosing) = editor_work.try_recv().unwrap() else {
            panic!("expected background diagnostics");
        };
        assert!(diagnosing.request.is_none());

        server
            .request(
                &connection,
                Request::new(
                    1.into(),
                    COMPLETION.into(),
                    json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 0}}),
                ),
            )
            .unwrap();
        assert!(diagnosing.cancel.check().is_err());
        server
            .reply(
                &connection,
                Reply {
                    lane: Lane::Editor,
                    revision: diagnosing.revision,
                    path: diagnosing.path,
                    publication: None,
                    response: None,
                    reference_coverage: None,
                },
            )
            .unwrap();
        server
            .schedule(&connection, &editor, &workspace, true)
            .unwrap();
        let WorkerMessage::Analyze(completing) = editor_work.try_recv().unwrap() else {
            panic!("expected completion work");
        };
        assert_eq!(completing.request.unwrap().id, 1.into());
        server
            .reply(
                &connection,
                Reply {
                    lane: Lane::Editor,
                    revision: completing.revision,
                    path: completing.path.clone(),
                    publication: None,
                    response: Some(QueryResponse::Value(Response::new_ok(
                        1.into(),
                        Vec::<Value>::new(),
                    ))),
                    reference_coverage: None,
                },
            )
            .unwrap();
        assert!(matches!(
            client.receiver.try_recv().unwrap(),
            Message::Response(_)
        ));
        assert!(client.receiver.is_empty());
        assert!(server.highlights.is_empty());
        server
            .schedule(&connection, &editor, &workspace, true)
            .unwrap();
        let WorkerMessage::Analyze(retry) = editor_work.try_recv().unwrap() else {
            panic!("expected diagnostics to be rescheduled");
        };
        assert!(retry.request.is_none());
        assert_eq!(retry.path, completing.path);
        assert_eq!(retry.revision, completing.revision);
        assert!(retry.cancel.check().is_ok());
    }

    #[test]
    fn workspace_cancellation_leaves_the_editor_lane_available() {
        let (server_connection, client) = Connection::memory();
        let (editor, editor_work) = unbounded();
        let (workspace, workspace_work) = unbounded();
        let mut server = Server::default();
        let path = PathBuf::from("root.nia");
        let references = Request::new(1.into(), REFERENCES.into(), ());
        server.pending.push_back((path.clone(), 0, references));
        server
            .schedule(&server_connection, &editor, &workspace, false)
            .unwrap();
        let WorkerMessage::Analyze(scanning) = workspace_work.try_recv().unwrap() else {
            panic!("expected workspace analysis");
        };

        server
            .notification(
                &server_connection,
                Notification::new("$/cancelRequest".into(), json!({"id": 1})),
            )
            .unwrap();
        assert!(scanning.cancel.check().is_err());
        let completion = Request::new(2.into(), COMPLETION.into(), ());
        server.pending.push_back((path.clone(), 0, completion));
        server
            .schedule(&server_connection, &editor, &workspace, false)
            .unwrap();
        let WorkerMessage::Analyze(completing) = editor_work.try_recv().unwrap() else {
            panic!("expected completion analysis");
        };
        assert!(completing.cancel.check().is_ok());

        server
            .reply(
                &server_connection,
                Reply {
                    lane: Lane::Workspace,
                    revision: 0,
                    path,
                    publication: None,
                    response: Some(QueryResponse::Value(Response::new_ok(1.into(), ()))),
                    reference_coverage: None,
                },
            )
            .unwrap();
        let Message::Response(response) = client.receiver.try_recv().unwrap() else {
            panic!("expected a canceled response");
        };
        assert_eq!(
            response.response_result.unwrap_err().code,
            ErrorCode::RequestCanceled as i32
        );
        assert!(server.editor.is_some());
    }
}
