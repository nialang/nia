// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    thread,
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender, select, tick, unbounded};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    Diagnostic, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    PublishDiagnosticsParams, Uri,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    analysis::{self, Analysis, Documents, OpenDocument, Session},
    document::Document,
    highlight::{self, Resolved},
    rename::Rename,
    workspace::Cancellation,
};

type Error = Box<dyn std::error::Error + Send + Sync>;
const SEMANTIC_FULL: &str = "textDocument/semanticTokens/full";
const SEMANTIC_REFRESH: &str = "workspace/semanticTokens/refresh";
const COMPLETION: &str = "textDocument/completion";
const RENAME: &str = "textDocument/rename";
const PREPARE_RENAME: &str = "textDocument/prepareRename";
const REFERENCES: &str = "textDocument/references";

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
                "referencesProvider": true,
                "documentHighlightProvider": true,
                "renameProvider": { "prepareProvider": true },
                "workspace": { "workspaceFolders": { "supported": true, "changeNotifications": true } },
                "hoverProvider": true,
                "documentSymbolProvider": true,
                "foldingRangeProvider": true,
                "semanticTokensProvider": { "legend": highlight::legend(), "full": true },
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

struct Work {
    revision: u64,
    disk_revision: u64,
    documents: Documents,
    path: PathBuf,
    request: Option<Request>,
    folders: Vec<PathBuf>,
    versioned_edits: bool,
    resource_rename: bool,
    cancel: Cancellation,
}

struct Reply {
    lane: Lane,
    revision: u64,
    path: PathBuf,
    diagnostics: Result<Vec<Diagnostic>, String>,
    highlights: Arc<Resolved>,
    response: Option<Response>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lane {
    Editor,
    Workspace,
}

impl Lane {
    fn for_request(request: &Request) -> Self {
        if matches!(request.method.as_str(), REFERENCES | RENAME) {
            Self::Workspace
        } else {
            Self::Editor
        }
    }
}

fn analyze(
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
    lane: Lane,
    queue: Receiver<Work>,
    output: Sender<Reply>,
) {
    let mut session = Session::new(toolchain.clone());
    let mut completion_probe = None;
    let mut cached = None;
    let mut disk_revision = 0;
    let mut revision = None;
    for work in queue {
        if revision != Some(work.revision) {
            session.workspace_index = None;
            revision = Some(work.revision);
        }
        if disk_revision != work.disk_revision {
            session.reload();
            completion_probe = None;
            disk_revision = work.disk_revision;
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
                    diagnostics: Ok(Vec::new()),
                    highlights: Arc::new(Resolved::new()),
                    response,
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
        let diagnostics = analysis
            .as_ref()
            .map(|analysis| {
                if lane == Lane::Editor {
                    analysis.diagnostics(&work.path, document)
                } else {
                    Vec::new()
                }
            })
            .map_err(Clone::clone);
        let response = work.request.map(|request| {
            if work.cancel.check().is_err() {
                return Response::new_err(
                    request.id,
                    ErrorCode::RequestCanceled as i32,
                    "Request canceled".into(),
                );
            }
            if request.method == SEMANTIC_FULL {
                return semantic_response(request, document, Some(highlights));
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
                        Err(error) => {
                            Response::new_err(request.id, ErrorCode::RequestFailed as i32, error)
                        }
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
                                    analysis.references(&work.path, position, include_declaration),
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
                                    index.references(
                                        analysis,
                                        &work.path,
                                        position,
                                        include_declaration,
                                        &work.cancel,
                                    )
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
                Ok(analysis) => query(
                    analysis,
                    &work.path,
                    request,
                    work.resource_rename,
                    |position| {
                        let probe =
                            completion_probe.get_or_insert_with(|| Session::new(toolchain.clone()));
                        probe.complete(analysis, &work.documents, &work.path, position)
                    },
                ),
                Err(error) => {
                    Response::new_err(request.id, ErrorCode::RequestFailed as i32, error.clone())
                }
            }
        });
        if output
            .send(Reply {
                lane,
                revision: work.revision,
                path: work.path,
                diagnostics,
                highlights: highlights.clone(),
                response,
            })
            .is_err()
        {
            break;
        }
    }
}

fn semantic_response(
    request: Request,
    document: &Document,
    resolved: Option<&Resolved>,
) -> Response {
    match decode::<lsp_types::SemanticTokensParams>(request.params) {
        Ok(_) => Response::new_ok(request.id, document.semantic_tokens(resolved)),
        Err(error) => Response::new_err(
            request.id,
            ErrorCode::InvalidParams as i32,
            error.to_string(),
        ),
    }
}

fn query(
    analysis: &Analysis,
    path: &std::path::Path,
    request: Request,
    resource_rename: bool,
    complete: impl FnOnce(lsp_types::Position) -> Option<Vec<lsp_types::CompletionItem>>,
) -> Response {
    let Request { id, method, params } = request;
    let result = match method.as_str() {
        "textDocument/definition" | "textDocument/hover" => {
            decode::<lsp_types::TextDocumentPositionParams>(params).map(|params| {
                if method == "textDocument/hover" {
                    json!(analysis.hover(path, params.position))
                } else {
                    json!(analysis.definition(path, params.position))
                }
            })
        }
        "textDocument/documentHighlight" => decode::<lsp_types::TextDocumentPositionParams>(params)
            .map(|params| json!(analysis.document_highlights(path, params.position))),
        PREPARE_RENAME => decode::<lsp_types::TextDocumentPositionParams>(params)
            .map(|params| json!(analysis.prepare_rename(path, params.position, resource_rename))),
        COMPLETION => decode::<lsp_types::CompletionParams>(params)
            .map(|params| json!(complete(params.text_document_position.position))),
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
    disk_revision: u64,
}

struct Active {
    request: Option<RequestId>,
    cancel: Cancellation,
}

impl Server {
    fn serve(
        &mut self,
        connection: &Connection,
        work: &Sender<Work>,
        workspace: &Sender<Work>,
        replies: &Receiver<Reply>,
    ) -> Result<(), Error> {
        let idle = tick(Duration::from_millis(90));
        loop {
            select! {
                recv(connection.receiver) -> message => match message? {
                    Message::Request(request) => {
                        if request.method == "shutdown" {
                            self.cancel_active();
                        }
                        if connection.handle_shutdown(&request)? {
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
        if !matches!(
            request.method.as_str(),
            "textDocument/definition"
                | REFERENCES
                | "textDocument/documentHighlight"
                | RENAME
                | PREPARE_RENAME
                | "textDocument/hover"
                | COMPLETION
                | "textDocument/signatureHelp"
                | "textDocument/documentSymbol"
                | "textDocument/foldingRange"
                | SEMANTIC_FULL
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
        if request.method == SEMANTIC_FULL {
            let resolved = self.highlights.get(&path);
            if resolved.is_some() || self.semantic_refresh {
                let response =
                    semantic_response(request, &open.document, resolved.map(Arc::as_ref));
                if resolved.is_none() && response.response_result.is_ok() {
                    self.syntax_sent.insert(path);
                }
                connection.sender.send(response.into())?;
            } else {
                self.pending.push_back((path, self.revision, request));
            }
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
                            path,
                            OpenDocument {
                                uri: document.uri,
                                document: Document::new(document.version, document.text),
                            },
                        );
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
            "textDocument/didSave" => decode::<DidSaveTextDocumentParams>(notification.params)
                .map(|_| self.disk_changed()),
            "workspace/didChangeWatchedFiles" => {
                decode::<DidChangeWatchedFilesParams>(notification.params).map(|params| {
                    if params
                        .changes
                        .iter()
                        .filter_map(|event| analysis::path(&event.uri))
                        .any(|path| {
                            path.extension().is_some_and(|extension| extension == "nia")
                                || path.file_name().is_some_and(|name| {
                                    name == "nia.toml"
                                        || name == "nia.lock"
                                        || name == "build-plan.bin"
                                })
                        })
                    {
                        self.disk_changed();
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
                            publish(connection, uri, None, Vec::new())?;
                            self.invalidate();
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

    fn disk_changed(&mut self) {
        self.disk_revision += 1;
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
        editor: &Sender<Work>,
        workspace: &Sender<Work>,
        diagnostics: bool,
    ) -> Result<(), Error> {
        for (lane, active, worker) in [
            (Lane::Editor, &mut self.editor, editor),
            (Lane::Workspace, &mut self.workspace, workspace),
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
            worker.send(Work {
                revision: self.revision,
                disk_revision: self.disk_revision,
                documents: self.documents.clone(),
                path,
                request,
                folders: self.folders.clone(),
                versioned_edits: self.versioned_edits,
                resource_rename: self.resource_rename,
                cancel,
            })?;
        }
        Ok(())
    }

    fn reply(&mut self, connection: &Connection, reply: Reply) -> Result<(), Error> {
        let active = match reply.lane {
            Lane::Editor => self.editor.take(),
            Lane::Workspace => self.workspace.take(),
        };
        let canceled = active.is_some_and(|active| active.cancel.check().is_err());
        if reply.lane == Lane::Editor
            && !canceled
            && reply.revision == self.revision
            && let Some(open) = self.documents.get(&reply.path)
        {
            self.dirty.remove(&reply.path);
            if self.syntax_sent.remove(&reply.path)
                && open.document.has_refinements(&reply.highlights)
            {
                self.refresh_needed = true;
            }
            self.highlights.insert(reply.path, reply.highlights);
            let diagnostics = reply.diagnostics.unwrap_or_else(|error| {
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
        if let Some(mut response) = reply.response {
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
            if let Some((code, message)) = code {
                response = Response::new_err(response.id, code as i32, message.into());
            }
            connection.sender.send(response.into())?;
        }
        Ok(())
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
        assert_eq!(server.disk_revision, 1);
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
        let scanning = workspace_work.try_recv().unwrap();

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
        assert!(editor_work.try_recv().unwrap().cancel.check().is_ok());

        server
            .reply(
                &server_connection,
                Reply {
                    lane: Lane::Workspace,
                    revision: 0,
                    path,
                    diagnostics: Ok(Vec::new()),
                    highlights: Arc::default(),
                    response: Some(Response::new_ok(1.into(), ())),
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
