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
    Diagnostic, DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, PublishDiagnosticsParams, Uri,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    analysis::{self, Analysis, Documents, OpenDocument, Session},
    document::Document,
    highlight::{self, Resolved},
};

type Error = Box<dyn std::error::Error + Send + Sync>;
const SEMANTIC_FULL: &str = "textDocument/semanticTokens/full";
const SEMANTIC_REFRESH: &str = "workspace/semanticTokens/refresh";

pub(crate) fn run(toolchain: nia_toolchain::ToolchainLayout) -> Result<(), Error> {
    let (connection, io) = Connection::stdio();
    let (id, params) = connection.initialize_start()?;
    let params = decode::<lsp_types::InitializeParams>(params)?;
    let semantic_refresh = params
        .capabilities
        .workspace
        .and_then(|workspace| workspace.semantic_tokens)
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
                "hoverProvider": true,
                "documentSymbolProvider": true,
                "foldingRangeProvider": true,
                "semanticTokensProvider": { "legend": highlight::legend(), "full": true },
                "completionProvider": { "triggerCharacters": [":"] },
                "signatureHelpProvider": { "triggerCharacters": ["(", ","], "retriggerCharacters": [","] }
            },
            "serverInfo": { "name": "nia-lsp", "version": nia_compat::COMPILER_VERSION },
        }),
    )?;
    let (work_tx, work_rx) = unbounded();
    let (reply_tx, reply_rx) = unbounded();
    let worker = thread::spawn(move || analyze(Arc::new(toolchain), work_rx, reply_tx));
    let result = Server {
        hierarchical_symbols,
        fold_limit: folding.range_limit.map(|limit| limit as usize),
        line_folding_only: folding.line_folding_only.unwrap_or_default(),
        semantic_refresh,
        ..Server::default()
    }
    .serve(&connection, &work_tx, &reply_rx);
    drop(connection);
    drop(work_tx);
    worker.join().map_err(|_| "Nia analysis thread panicked")?;
    io.join()?;
    result
}

struct Work {
    revision: u64,
    documents: Documents,
    path: PathBuf,
    request: Option<Request>,
}

struct Reply {
    revision: u64,
    path: PathBuf,
    diagnostics: Result<Vec<Diagnostic>, String>,
    highlights: Arc<Resolved>,
    response: Option<Response>,
}

fn analyze(
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
    queue: Receiver<Work>,
    output: Sender<Reply>,
) {
    let mut session = Session::new(toolchain);
    let mut cached = None;
    for work in queue {
        let key = (work.revision, &work.path);
        if !cached
            .as_ref()
            .is_some_and(|(revision, path, _, _)| (*revision, path) == key)
        {
            let analysis = session.analyze(&work.documents, &work.path);
            let highlights = Arc::new(analysis.as_ref().map_or_else(
                |_| Resolved::new(),
                |analysis| analysis.semantic_highlights(&work.path),
            ));
            cached = Some((work.revision, work.path.clone(), analysis, highlights));
        }
        let (_, _, analysis, highlights) = cached.as_ref().expect("analysis cache filled");
        let document = &work.documents[&work.path].document;
        let diagnostics = analysis
            .as_ref()
            .map(|analysis| analysis.diagnostics(&work.path, document))
            .map_err(Clone::clone);
        let response = work.request.map(|request| {
            if request.method == SEMANTIC_FULL {
                return semantic_response(request, document, Some(highlights));
            }
            match analysis {
                Ok(analysis) => query(analysis, &work.path, request),
                Err(error) => {
                    Response::new_err(request.id, ErrorCode::RequestFailed as i32, error.clone())
                }
            }
        });
        if output
            .send(Reply {
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

fn query(analysis: &Analysis, path: &std::path::Path, request: Request) -> Response {
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
        "textDocument/references" => decode::<lsp_types::ReferenceParams>(params).map(|params| {
            json!(analysis.references(
                path,
                params.text_document_position.position,
                params.context.include_declaration,
            ))
        }),
        "textDocument/completion" => decode::<lsp_types::CompletionParams>(params)
            .map(|params| json!(analysis.complete(path, params.text_document_position.position))),
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
    active: bool,
    active_request: Option<RequestId>,
    canceled: bool,
    revision: u64,
}

impl Server {
    fn serve(
        &mut self,
        connection: &Connection,
        work: &Sender<Work>,
        replies: &Receiver<Reply>,
    ) -> Result<(), Error> {
        let idle = tick(Duration::from_millis(90));
        loop {
            select! {
                recv(connection.receiver) -> message => match message? {
                    Message::Request(request) => {
                        if connection.handle_shutdown(&request)? {
                            break;
                        }
                        self.request(connection, request)?;
                        self.schedule(connection, work, false)?;
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
                    self.schedule(connection, work, false)?;
                },
                recv(idle) -> _ => {
                    self.schedule(connection, work, true)?;
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
                | "textDocument/references"
                | "textDocument/hover"
                | "textDocument/completion"
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
            "textDocument/didSave" => {
                decode::<DidSaveTextDocumentParams>(notification.params).map(|_| self.invalidate())
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
            "$/cancelRequest" => match decode::<Cancel>(notification.params) {
                Ok(params) => {
                    if self.active_request.as_ref() == Some(&params.id) {
                        self.canceled = true;
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
        self.revision += 1;
        self.dirty = self.documents.keys().cloned().collect();
        self.highlights.clear();
        self.syntax_sent.clear();
        self.refresh_needed = false;
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
        worker: &Sender<Work>,
        diagnostics: bool,
    ) -> Result<(), Error> {
        if self.active {
            return Ok(());
        }
        let (path, request) = loop {
            if let Some((path, revision, request)) = self.pending.pop_front() {
                if revision == self.revision {
                    break (path, Some(request));
                }
                connection.sender.send(
                    Response::new_err(
                        request.id,
                        ErrorCode::ContentModified as i32,
                        "Document changed before analysis".into(),
                    )
                    .into(),
                )?;
            } else if diagnostics && let Some(path) = self.dirty.pop_first() {
                break (path, None);
            } else {
                return Ok(());
            }
        };
        self.active = true;
        self.active_request = request.as_ref().map(|request| request.id.clone());
        worker.send(Work {
            revision: self.revision,
            documents: self.documents.clone(),
            path,
            request,
        })?;
        Ok(())
    }

    fn reply(&mut self, connection: &Connection, reply: Reply) -> Result<(), Error> {
        self.active = false;
        self.active_request = None;
        let canceled = std::mem::take(&mut self.canceled);
        if reply.revision == self.revision
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
            let code = if canceled {
                Some((ErrorCode::RequestCanceled, "Request canceled"))
            } else if reply.revision != self.revision {
                Some((
                    ErrorCode::ContentModified,
                    "Document changed during analysis",
                ))
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
