// SPDX-License-Identifier: GPL-3.0-or-later
//! Exercises the language server over stdio, end to end: editing and
//! navigation, module and package moves, build-plan entries, source-mapped
//! generated code, and long expressions.
//!
//! CI points `NIA_LSP_SMOKE_SERVER` at the `bin/nia-lsp` of each unpacked
//! release archive; see `support` for the default.

mod support;

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

use nia_test_support::{CommandExt, TestWorkload, acquire_test_resources};
use serde_json::{Value, json};
use support::{
    Client, Workspace, array, create_dir_all, integer, is_empty, point, position, read_text,
    shifted, string, uri, write_text,
};

const TIMEOUT: Duration = Duration::from_secs(60);

/// The `nia` compiler beside the server under test.
///
/// The workspace's `nia` is a binary of another package, so Cargo does not
/// build it for this one. Build it in the same profile as the test, as
/// `cargo build --profile test -p nia-cli`: a `dev` compiler is unoptimized
/// and runs out of stack on the long-expression fixture.
fn nia_command() -> Command {
    let name = format!("nia{}", std::env::consts::EXE_SUFFIX);
    match support::packaged_server() {
        Some(server) => Command::new(server.with_file_name(name)),
        None => {
            let compiler = Path::new(env!("CARGO_BIN_EXE_nia-lsp")).with_file_name(name);
            assert!(
                compiler.is_file(),
                "{} is missing; run `cargo build --profile test -p nia-cli` first",
                compiler.display(),
            );
            nia_test_support::nia_command(compiler)
        }
    }
}

/// Writes `text` with `\n` line endings on every platform.
fn write_lf(path: &Path, text: &str) {
    write_bytes(path, text.as_bytes());
}

fn write_bytes(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn rename(from: &Path, to: &Path) {
    fs::rename(from, to)
        .unwrap_or_else(|error| panic!("rename {} to {}: {error}", from.display(), to.display()));
}

/// `point` moved `delta` lines.
fn shifted_lines(point: &Value, delta: i64) -> Value {
    let mut moved = point.clone();
    moved["line"] = json!(integer(&point["line"]) + delta);
    moved
}

fn range(start: &Value, end: &Value) -> Value {
    json!({ "start": start, "end": end })
}

/// The only element of `value`, which must be an array of one.
fn single(value: &Value) -> &Value {
    match array(value) {
        [item] => item,
        _ => panic!("expected exactly one element, got {value}"),
    }
}

/// The byte offset of LSP position `point` in `text`. A position past the
/// last line names the end of the text.
fn byte_offset(text: &str, point: &Value) -> usize {
    let line = usize::try_from(integer(&point["line"])).expect("line is not negative");
    let character =
        usize::try_from(integer(&point["character"])).expect("character is not negative");
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(index) => start += index + 1,
            None => return text.len(),
        }
    }
    let mut units = 0;
    for (index, ch) in text[start..].char_indices() {
        if units >= character || ch == '\n' {
            return start + index;
        }
        units += ch.len_utf16();
    }
    text.len()
}

/// `text` after the text edits `edits`, applied from the last to the first
/// as the protocol requires of non-overlapping edits.
fn apply_edits(text: &str, edits: &[Value]) -> String {
    let mut text = text.to_owned();
    for edit in edits.iter().rev() {
        let start = byte_offset(&text, &edit["range"]["start"]);
        let end = byte_offset(&text, &edit["range"]["end"]);
        text.replace_range(start..end, string(&edit["newText"]));
    }
    text
}

/// `PATH` with `directory` searched first.
fn path_with(directory: &Path) -> OsString {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(
        std::iter::once(directory.to_path_buf()).chain(std::env::split_paths(&inherited)),
    )
    .expect("PATH entries contain no separator")
}

/// The diagnostics published for `uri` at `version` that `accept` accepts.
fn diagnostics_of(
    client: &mut Client,
    uri: &str,
    version: Option<i64>,
    accept: impl Fn(&[Value]) -> bool,
) -> Vec<Value> {
    client.wait_diagnostics(uri, version, None, accept).1
}

/// Waits until the server has sent a request with `method`.
fn wait_server_request(client: &mut Client, method: &str) {
    let deadline = Instant::now() + TIMEOUT;
    while !client.server_requests.contains_key(method) {
        client.receive(deadline, &format!("a {method} request from the server"));
    }
}

/// The URIs of the symbols a `workspace/symbol` request returned.
fn symbol_uris(symbols: &Value) -> Vec<&str> {
    array(symbols)
        .iter()
        .map(|symbol| string(&symbol["location"]["uri"]))
        .collect()
}

/// The keys of a JSON object.
fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("expected an object, got {value}"))
        .keys()
        .map(String::as_str)
        .collect()
}

/// Content changes that apply text edits `edits` in the order given.
fn content_changes<'a>(edits: impl IntoIterator<Item = &'a Value>) -> Value {
    edits
        .into_iter()
        .map(|edit| json!({ "range": edit["range"], "text": edit["newText"] }))
        .collect()
}

/// Whether any diagnostic is an error whose message contains `text`.
fn has_error(items: &[Value], text: &str) -> bool {
    items
        .iter()
        .any(|item| item["severity"] == 1 && string(&item["message"]).contains(text))
}

/// Whether any diagnostic message contains `text`.
fn mentions(items: &[Value], text: &str) -> bool {
    items
        .iter()
        .any(|item| string(&item["message"]).contains(text))
}

fn open(client: &mut Client, path: &Path, version: i64, text: &str) {
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": {
            "uri": uri(path), "languageId": "nia", "version": version, "text": text,
        }}),
    );
}

fn replace_text(client: &mut Client, document: &Value, version: i64, text: &str) {
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": versioned(document, version),
            "contentChanges": [{ "text": text }],
        }),
    );
}

fn edit_text(client: &mut Client, document: &Value, version: i64, changes: Value) {
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": versioned(document, version), "contentChanges": changes }),
    );
}

fn versioned(document: &Value, version: i64) -> Value {
    json!({ "uri": document["uri"], "version": version })
}

fn definition(client: &mut Client, path: &Path, text: &str, needle: &str) -> Value {
    client.call(
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri(path) }, "position": position(text, needle) }),
    )
}

/// Editing, navigation, formatting, code actions, semantic tokens, file
/// operations, and workspace folders in one package.
#[test]
fn editing_and_navigation() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let workspace = Workspace::new("nia-lsp-smoke");
    let root = workspace.root.as_path();
    let manifest =
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n";
    let source = concat!(
        "module shapes;\n",
        "using std::process;\n",
        "fun analyze(init: process::Init, shape: &shapes::Shape) i32 {\n",
        "    _ = init;\n",
        "    _ = \"😀\"; shape.width\n",
        "}\n",
        "fun measure(low: i32) i32 { _=\"\u{1f34e}\"; shapes::distance(low, 10) }\n",
    );
    write_text(&root.join("nia.toml"), manifest);
    let main = root.join("root.nia");
    write_text(&main, source);
    let shapes = root.join("shapes.nia");
    let shapes_source = concat!(
        "pub struct Shape { width: i32 }\n",
        "/// Returns the weight.\n",
        "pub fun weight() i32 { 1 }\n",
        "pub fun distance(low: i32, high: i32) i32 { high - low }\n",
    );
    write_text(&shapes, shapes_source);
    write_text(
        &root.join("extensions.nia"),
        "extend root::shapes::Shape { pub fun measureHeight(&self) i32 { 42 } }\n",
    );
    write_text(
        &root.join("other_extensions.nia"),
        "struct Other {}\nextend Other { pub fun measureHeight(&self) i32 { 0 } }\n",
    );
    let mut client = Client::start(root, TIMEOUT);
    let initialized = client.call(
        "initialize",
        json!({
            "processId": null,
            "rootUri": uri(root),
            "capabilities": {
                "workspace": {
                    "semanticTokens": { "refreshSupport": true },
                    "didChangeWatchedFiles": { "dynamicRegistration": true },
                    "fileOperations": { "willRename": true, "didRename": true },
                    "workspaceFolders": true,
                    "workspaceEdit": { "documentChanges": true },
                },
                "textDocument": {
                    "codeAction": {
                        "codeActionLiteralSupport": {
                            "codeActionKind": { "valueSet": ["quickfix"] },
                        },
                    },
                    "semanticTokens": {
                        "requests": { "full": { "delta": true }, "range": true },
                        "tokenTypes": [], "tokenModifiers": [], "formats": ["relative"],
                    },
                },
            },
        }),
    );
    let capabilities = &initialized["capabilities"];
    assert_eq!(capabilities["positionEncoding"], "utf-16");
    for provider in [
        "documentFormattingProvider",
        "documentRangeFormattingProvider",
        "selectionRangeProvider",
        "inlayHintProvider",
        "callHierarchyProvider",
        "workspaceSymbolProvider",
        "typeDefinitionProvider",
    ] {
        assert!(
            !is_empty(&capabilities[provider]),
            "{provider}: {capabilities}"
        );
    }
    assert!(!is_empty(
        &capabilities["semanticTokensProvider"]["full"]["delta"]
    ));
    assert!(!is_empty(&capabilities["semanticTokensProvider"]["range"]));
    assert_eq!(
        keys(&capabilities["workspace"]["fileOperations"]),
        BTreeSet::from(["willRename", "didRename"]),
    );
    let action_kinds = array(&capabilities["codeActionProvider"]["codeActionKinds"])
        .iter()
        .map(string)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        action_kinds,
        BTreeSet::from(["quickfix", "source.organizeImports"])
    );
    client.notify("initialized", json!({}));

    // A canceled request either finishes or reports the cancellation.
    let canceled = client.request(
        "workspace/symbol",
        json!({ "query": "fixture::shapes::Shape" }),
    );
    client.notify("$/cancelRequest", json!({ "id": canceled }));
    let canceled_response = client.wait(canceled);
    if let Some(error) = canceled_response.get("error") {
        assert_eq!(error["code"], -32800, "{canceled_response}");
    } else {
        let names = array(&canceled_response["result"])
            .iter()
            .map(|symbol| string(&symbol["name"]))
            .collect::<Vec<_>>();
        assert_eq!(names, ["Shape"], "{canceled_response}");
    }

    let symbols = client.call(
        "workspace/symbol",
        json!({ "query": "fixture::shapes::Shape" }),
    );
    assert_eq!(symbol_uris(&symbols), [uri(&shapes)], "{symbols}");

    let document = json!({ "uri": uri(&main) });
    let params = json!({ "textDocument": document });
    open(&mut client, &main, 1, source);
    assert!(!is_empty(
        &client.call("textDocument/semanticTokens/full", params.clone())["data"]
    ));
    assert!(!is_empty(
        &client.call("textDocument/foldingRange", params.clone())
    ));
    let field_name = position(source, "width");
    let field_access = position(source, "shape.width");
    let string_literal = position(source, "\"\u{1f34e}\"");
    let selections = client.call(
        "textDocument/selectionRange",
        json!({
            "textDocument": document,
            "positions": [field_name, shifted(&string_literal, 1)],
        }),
    );
    let [field_selection, literal_selection] = array(&selections) else {
        panic!("expected two selection ranges, got {selections}");
    };
    let field_end = shifted(&field_name, "width".len() as i64);
    assert_eq!(
        field_selection["range"],
        range(&field_name, &field_end),
        "{selections}"
    );
    assert_eq!(
        field_selection["parent"]["range"],
        range(&field_access, &field_end),
        "{selections}",
    );
    assert_eq!(
        literal_selection["range"],
        range(&string_literal, &shifted(&string_literal, 4)),
        "{selections}",
    );

    let options = json!({ "tabSize": 2, "insertSpaces": true });
    let before_eq = position(source, "=\"");
    let after_eq = shifted(&before_eq, 1);
    let indent_edits = [3, 4]
        .map(|line| json!({ "range": range(&point(line, 0), &point(line, 4)), "newText": "  " }));
    let operator_edits =
        [&before_eq, &after_eq].map(|at| json!({ "range": range(at, at), "newText": " " }));
    let formatted = client.call(
        "textDocument/formatting",
        json!({ "textDocument": document, "options": options }),
    );
    assert_eq!(
        array(&formatted),
        [indent_edits.as_slice(), &operator_edits].concat()
    );
    let formatted = client.call(
        "textDocument/rangeFormatting",
        json!({
            "textDocument": document,
            "options": options,
            "range": range(&position(source, "shape.width"), &point(5, 0)),
        }),
    );
    assert_eq!(array(&formatted), &indent_edits[1..]);
    let formatted = client.call(
        "textDocument/rangeFormatting",
        json!({
            "textDocument": document,
            "options": options,
            "range": range(&position(source, "shapes::distance"), &point(7, 0)),
        }),
    );
    assert_eq!(array(&formatted), operator_edits);

    let target = client.call(
        "textDocument/typeDefinition",
        json!({ "textDocument": document, "position": position(source, "shape.width") }),
    );
    assert_eq!(target["uri"], uri(&shapes), "{target}");
    let field = json!({ "textDocument": document, "position": field_name });
    let target = client.call("textDocument/definition", field.clone());
    assert_eq!(target["uri"], uri(&shapes), "{target}");
    assert_eq!(target["range"]["start"], point(0, 19), "{target}");
    let hint_position = position(source, "10)");
    let hints = client.call(
        "textDocument/inlayHint",
        json!({ "textDocument": document, "range": range(&point(6, 0), &point(7, 0)) }),
    );
    assert_eq!(
        hints,
        json!([{
            "label": "high:", "position": hint_position, "kind": 2, "paddingRight": true,
        }]),
    );
    let callees = client.call(
        "textDocument/prepareCallHierarchy",
        json!({ "textDocument": document, "position": position(source, "distance") }),
    );
    let callee = single(&callees).clone();
    assert!(
        callee["name"] == "distance" && callee["uri"] == uri(&shapes),
        "{callee}",
    );
    let incoming = client.call("callHierarchy/incomingCalls", json!({ "item": callee }));
    let incoming = single(&incoming).clone();
    assert_eq!(incoming["from"]["name"], "measure", "{incoming}");
    let outgoing = client.call(
        "callHierarchy/outgoingCalls",
        json!({ "item": incoming["from"] }),
    );
    let outgoing = single(&outgoing);
    assert_eq!(outgoing["to"], callee, "{outgoing}");
    let call_ranges = json!([range(
        &position(source, "shapes::distance"),
        &shifted(&hint_position, 3),
    )]);
    assert!(
        outgoing["fromRanges"] == incoming["fromRanges"] && incoming["fromRanges"] == call_ranges,
        "{outgoing}",
    );

    // Renaming the field edits both files and refreshes highlighting.
    let mut rename_params = field.clone();
    rename_params["newName"] = json!("height");
    let edit = client.call("textDocument/rename", rename_params);
    let edited = array(&edit["documentChanges"])
        .iter()
        .map(|item| string(&item["textDocument"]["uri"]))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        edited,
        BTreeSet::from([uri(&main).as_str(), uri(&shapes).as_str()]),
        "{edit}",
    );
    assert!(
        client
            .server_requests
            .contains_key("client/registerCapability")
    );
    wait_server_request(&mut client, "workspace/semanticTokens/refresh");

    write_text(&shapes, &shapes_source.replace("width", "height"));
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": uri(&shapes), "type": 2 }] }),
    );
    diagnostics_of(&mut client, &uri(&main), Some(1), |items| {
        has_error(items, "width")
    });
    let start = field["position"].clone();
    let end = shifted(&start, "width".len() as i64);
    edit_text(
        &mut client,
        &document,
        2,
        json!([{ "range": range(&start, &end), "text": "height" }]),
    );
    diagnostics_of(&mut client, &uri(&main), Some(2), <[Value]>::is_empty);
    write_text(&main, &source.replace("width", "height"));
    client.notify("textDocument/didSave", params.clone());
    let target = client.call("textDocument/definition", field.clone());
    assert_eq!(target["uri"], uri(&shapes), "{target}");
    let stale = client.request("callHierarchy/incomingCalls", json!({ "item": callee }));
    let stale = client.wait(stale);
    assert_eq!(stale["error"]["code"], -32801, "{stale}");

    // A misspelled name: selection, semantic tokens, completion, and a fix.
    let mut updated = source.replace("width", "height");
    let typo = updated.replace("shape.height", "shapes::weigth()");
    replace_text(&mut client, &document, 3, &typo);
    let typo_name = position(&typo, "weigth");
    let selection = client.call(
        "textDocument/selectionRange",
        json!({ "textDocument": document, "positions": [typo_name] }),
    );
    let selection = single(&selection);
    assert_eq!(
        selection["range"],
        range(&typo_name, &shifted(&typo_name, "weigth".len() as i64)),
        "{selection}",
    );
    let tokens = client.call("textDocument/semanticTokens/full", params.clone());
    let visible_tokens = client.call(
        "textDocument/semanticTokens/range",
        json!({ "textDocument": document, "range": range(&point(4, 0), &point(5, 0)) }),
    );
    assert!(visible_tokens.get("resultId").is_none(), "{visible_tokens}");
    let token_data = array(&tokens["data"]);
    let visible_data = array(&visible_tokens["data"]);
    assert!(
        !visible_data.is_empty() && visible_data.len() < token_data.len(),
        "{visible_tokens}",
    );
    // The range request encodes line 4 as the full response does, except
    // that its first token is relative to the start of the document.
    let mut line = 0;
    let mut column = 0;
    let mut full_line: Vec<Value> = Vec::new();
    for token in token_data.chunks(5) {
        line += integer(&token[0]);
        column = if integer(&token[0]) != 0 {
            integer(&token[1])
        } else {
            column + integer(&token[1])
        };
        if line == 4 {
            if full_line.is_empty() {
                full_line.extend([json!(line), json!(column)]);
                full_line.extend_from_slice(&token[2..]);
            } else {
                full_line.extend_from_slice(token);
            }
        }
    }
    assert_eq!(visible_data, full_line, "{visible_tokens}");
    let start = position(&typo, "weigth");
    let end = shifted(&start, "weigth".len() as i64);
    let completions = client.call(
        "textDocument/completion",
        json!({ "textDocument": document, "position": shifted(&start, "weig".len() as i64) }),
    );
    let completion = single(&completions);
    assert_eq!(completion["label"], "weight", "{completion}");
    assert_eq!(completion["detail"], "fun weight() i32", "{completion}");
    assert_eq!(
        completion["documentation"], "Returns the weight.",
        "{completion}"
    );
    assert_eq!(
        completion["textEdit"],
        json!({ "range": range(&start, &end), "newText": "weight" }),
        "{completion}",
    );
    let diagnostics = diagnostics_of(&mut client, &uri(&main), Some(3), |items| {
        mentions(items, "unknown value `weigth`")
    });
    let delta = client.call(
        "textDocument/semanticTokens/full/delta",
        json!({ "textDocument": document, "previousResultId": tokens["resultId"] }),
    );
    assert!(!is_empty(&delta["edits"]), "{delta}");
    let mut applied = token_data.to_vec();
    for edit in array(&delta["edits"]).iter().rev() {
        let start = usize::try_from(integer(&edit["start"])).expect("edit start");
        let delete = usize::try_from(integer(&edit["deleteCount"])).expect("delete count");
        let data = edit.get("data").map_or(&[][..], |data| array(data));
        applied.splice(start..start + delete, data.iter().cloned());
    }
    let unchanged = client.call(
        "textDocument/semanticTokens/full/delta",
        json!({ "textDocument": document, "previousResultId": delta["resultId"] }),
    );
    assert_eq!(unchanged["edits"], json!([]), "{unchanged}");
    // A superseded result id falls back to the full tokens, which the delta
    // produced from it.
    let fallback = client.call(
        "textDocument/semanticTokens/full/delta",
        json!({ "textDocument": document, "previousResultId": tokens["resultId"] }),
    );
    assert!(
        array(&fallback["data"]) == applied && applied != token_data,
        "{fallback}",
    );
    let code_action_range = range(&start, &end);
    let actions = client.call(
        "textDocument/codeAction",
        json!({
            "textDocument": document,
            "range": code_action_range,
            "context": { "diagnostics": diagnostics, "only": ["quickfix"] },
        }),
    );
    let action = single(&actions);
    assert!(
        action["kind"] == "quickfix" && string(&action["title"]).contains("weight"),
        "{action}",
    );
    assert!(is_empty(&action["isPreferred"]), "{action}");
    let change = single(&action["edit"]["documentChanges"]);
    assert_eq!(change["textDocument"], versioned(&document, 3), "{change}");
    assert_eq!(
        change["edits"],
        json!([{ "range": code_action_range, "newText": "weight" }]),
        "{change}",
    );
    assert!(is_empty(&client.call(
        "textDocument/codeAction",
        json!({
            "textDocument": document,
            "range": code_action_range,
            "context": { "diagnostics": diagnostics, "only": ["source.organizeImports"] },
        }),
    )));
    let replacement = &change["edits"][0];
    edit_text(&mut client, &document, 4, content_changes([replacement]));
    diagnostics_of(&mut client, &uri(&main), Some(4), <[Value]>::is_empty);
    let hover = client.call(
        "textDocument/hover",
        json!({ "textDocument": document, "position": start }),
    );
    let [signature, docs] = array(&hover["contents"]) else {
        panic!("expected a signature and documentation, got {hover}");
    };
    assert_eq!(
        *signature,
        json!({ "language": "nia", "value": "fun weight() i32" })
    );
    assert_eq!(*docs, "Returns the weight.");
    assert!(is_empty(&client.call(
        "textDocument/codeAction",
        json!({
            "textDocument": document,
            "range": code_action_range,
            "context": { "diagnostics": diagnostics },
        }),
    )));

    // Completing a name that needs an import adds the import.
    let partial = updated.replace("shape.height", "weig()");
    replace_text(&mut client, &document, 5, &partial);
    let start = position(&partial, "weig");
    let cursor = shifted(&start, "weig".len() as i64);
    let completions = client.call(
        "textDocument/completion",
        json!({ "textDocument": document, "position": cursor }),
    );
    let completion = single(&completions);
    assert_eq!(completion["label"], "weight", "{completion}");
    assert_eq!(
        completion["textEdit"],
        json!({ "range": range(&start, &cursor), "newText": "weight" }),
        "{completion}",
    );
    let auto_import = single(&completion["additionalTextEdits"]);
    assert_eq!(
        auto_import["newText"], "using root::shapes::weight;\n\n",
        "{completion}"
    );
    edit_text(
        &mut client,
        &document,
        6,
        content_changes([&completion["textEdit"], auto_import]),
    );
    diagnostics_of(&mut client, &uri(&main), Some(6), <[Value]>::is_empty);
    let target = client.call(
        "textDocument/definition",
        json!({ "textDocument": document, "position": shifted_lines(&start, 2) }),
    );
    assert_eq!(target["uri"], uri(&shapes), "{target}");

    // An unknown name offers its import only when the user asks for fixes.
    let missing = updated.replace("shape.height", "weight()");
    replace_text(&mut client, &document, 7, &missing);
    let diagnostics = diagnostics_of(&mut client, &uri(&main), Some(7), |items| {
        mentions(items, "unknown value `weight`")
    });
    let start = position(&missing, "weight");
    let end = shifted(&start, "weight".len() as i64);
    let quick_fix_range = range(&start, &end);
    let quick_fixes = |diagnostics: &[Value], trigger: i64| {
        json!({
            "textDocument": document,
            "range": quick_fix_range,
            "context": { "diagnostics": diagnostics, "only": ["quickfix"], "triggerKind": trigger },
        })
    };
    assert!(is_empty(
        &client.call("textDocument/codeAction", quick_fixes(&diagnostics, 2))
    ));
    let actions = client.call("textDocument/codeAction", quick_fixes(&diagnostics, 1));
    let action = single(&actions);
    assert_eq!(
        action["title"], "Import `root::shapes::weight`",
        "{actions}"
    );
    let change = single(&action["edit"]["documentChanges"]);
    assert_eq!(change["textDocument"], versioned(&document, 7), "{change}");
    let insertion = single(&change["edits"]);
    assert_eq!(
        insertion["newText"], "using root::shapes::weight;\n\n",
        "{insertion}"
    );
    edit_text(&mut client, &document, 8, content_changes([insertion]));
    diagnostics_of(&mut client, &uri(&main), Some(8), <[Value]>::is_empty);

    // Two candidates are offered in order, an incompatible one included.
    for (name, declaration) in [
        ("alternate", "pub fun weight() i32 { 2 }\n"),
        ("incompatible", "pub fun weight(limit: i32) i32 { limit }\n"),
    ] {
        write_text(&root.join(format!("{name}.nia")), declaration);
    }
    let ambiguous = format!("{missing}\nmodule alternate;\nmodule incompatible;\n");
    replace_text(&mut client, &document, 9, &ambiguous);
    let diagnostics = diagnostics_of(&mut client, &uri(&main), Some(9), |items| {
        mentions(items, "unknown value `weight`")
    });
    let actions = client.call("textDocument/codeAction", quick_fixes(&diagnostics, 1));
    let titles = array(&actions)
        .iter()
        .map(|action| string(&action["title"]))
        .collect::<Vec<_>>();
    assert_eq!(
        titles,
        [
            "Import `root::alternate::weight`",
            "Import `root::shapes::weight`"
        ],
        "{actions}",
    );
    let completions = client.call(
        "textDocument/completion",
        json!({ "textDocument": document, "position": end }),
    );
    let descriptions = array(&completions)
        .iter()
        .map(|item| string(&item["labelDetails"]["description"]))
        .collect::<Vec<_>>();
    assert_eq!(
        descriptions,
        ["root::alternate::weight", "root::shapes::weight"],
        "{completions}",
    );

    // Organizing imports sorts every import list and keeps the diagnostics.
    let mut unordered = updated
        .replace(
            "using std::process;",
            "using std::process;\n\
             using root::shapes::{weight as w, distance as d};\n\
             using root::shapes as geo;",
        )
        .replace("shapes::Shape", "geo::Shape")
        .replace("shapes::distance(low, 10)", "d(low, 10) + w()");
    unordered += "\n// Public API\npub using root::shapes::{weight as mass, distance as width};\n";
    unordered += concat!(
        "fun local() i32 {\n",
        "    using root::shapes::weight as measured_weight;\n",
        "    using root::shapes::distance as measured_distance;\n",
        "    measured_weight() + measured_distance(2, 4)\n",
        "}\n",
    );
    unordered += "fun unfinished() i32 { pendingValue }\n";
    replace_text(&mut client, &document, 10, &unordered);
    let before_organize = diagnostics_of(&mut client, &uri(&main), Some(10), |items| {
        mentions(items, "unknown value `pendingValue`")
    });
    let organize_params = json!({
        "textDocument": document,
        "range": range(&point(0, 0), &point(0, 0)),
        "context": { "diagnostics": [], "only": ["source.organizeImports"] },
    });
    let actions = client.call("textDocument/codeAction", organize_params.clone());
    let action = single(&actions);
    assert_eq!(action["kind"], "source.organizeImports", "{actions}");
    let change = single(&action["edit"]["documentChanges"]);
    assert_eq!(change["textDocument"], versioned(&document, 10), "{change}");
    let [reorder, public, local] = array(&change["edits"]) else {
        panic!("expected three organize edits, got {change}");
    };
    assert_eq!(
        reorder["newText"],
        concat!(
            "using root::shapes as geo;\n",
            "using root::shapes::{distance as d, weight as w};\n",
            "using std::process;",
        ),
        "{reorder}",
    );
    assert_eq!(
        public["newText"], "distance as width, weight as mass",
        "{public}"
    );
    assert_eq!(
        local["newText"],
        concat!(
            "using root::shapes::distance as measured_distance;\n",
            "    using root::shapes::weight as measured_weight;",
        ),
        "{local}",
    );
    edit_text(
        &mut client,
        &document,
        11,
        content_changes(array(&change["edits"]).iter().rev()),
    );
    let after_organize = diagnostics_of(&mut client, &uri(&main), Some(11), |_| true);
    assert_eq!(after_organize, before_organize);

    // Member completion while the member is still being typed.
    let member_source = updated.replace("shape.height", "shape.he");
    replace_text(&mut client, &document, 12, &member_source);
    let start = position(&member_source, "he\n");
    let end = shifted(&start, 2);
    let mut completions = client.call(
        "textDocument/completion",
        json!({ "textDocument": document, "position": end, "context": { "triggerKind": 1 } }),
    );
    if completions.is_object() {
        assert_eq!(completions["isIncomplete"], true, "{completions}");
        completions = completions["items"].take();
    }
    let heights = array(&completions)
        .iter()
        .filter(|item| item["label"] == "height")
        .collect::<Vec<_>>();
    let [height] = heights.as_slice() else {
        panic!("expected one `height` completion, got {completions}");
    };
    assert_eq!(
        height["textEdit"],
        json!({ "range": range(&start, &end), "newText": "height" }),
        "{height}",
    );
    diagnostics_of(&mut client, &uri(&main), Some(12), |items| {
        has_error(items, "`he`")
    });
    let completions = client.call(
        "textDocument/completion",
        json!({ "textDocument": document, "position": end, "context": { "triggerKind": 3 } }),
    );
    let labels = array(&completions)
        .iter()
        .map(|item| string(&item["label"]))
        .collect::<Vec<_>>();
    assert_eq!(labels, ["height"], "{completions}");

    // An extension method offers the import of the module that provides it.
    let missing_method = updated.replace("shape.height", "shape.measureHeight()")
        + "module extensions;\nmodule other_extensions;\n";
    replace_text(&mut client, &document, 13, &missing_method);
    let diagnostics = diagnostics_of(&mut client, &uri(&main), Some(13), |items| {
        mentions(items, "unknown method `measureHeight`")
    });
    let unknown_methods = diagnostics
        .iter()
        .filter(|item| string(&item["message"]).contains("unknown method"))
        .collect::<Vec<_>>();
    let [unknown_method] = unknown_methods.as_slice() else {
        panic!("expected one unknown method, got {diagnostics:?}");
    };
    let fixes = client.call(
        "textDocument/codeAction",
        json!({
            "textDocument": document,
            "range": unknown_method["range"],
            "context": { "diagnostics": [unknown_method], "only": ["quickfix"] },
        }),
    );
    let fix = single(&fixes);
    assert_eq!(fix["title"], "Import `root::extensions`", "{fix}");
    let change = single(&fix["edit"]["documentChanges"]).clone();
    assert_eq!(change["textDocument"], versioned(&document, 13), "{change}");
    assert_eq!(
        change["edits"][0]["newText"], "using root::extensions;\n\n",
        "{change}"
    );
    edit_text(
        &mut client,
        &document,
        14,
        content_changes(array(&change["edits"])),
    );
    diagnostics_of(&mut client, &uri(&main), Some(14), <[Value]>::is_empty);

    for (version, suffix) in [(15, "()"), (16, "")] {
        let partial_member =
            missing_method.replace("measureHeight()", &format!("measureHe{suffix}"));
        replace_text(&mut client, &document, version, &partial_member);
        let start = position(&partial_member, "measureHe");
        let end = shifted(&start, "measureHe".len() as i64);
        let completions = client.call(
            "textDocument/completion",
            json!({ "textDocument": document, "position": end }),
        );
        let completion = single(&completions);
        assert_eq!(completion["label"], "measureHeight", "{completion}");
        assert_eq!(completion["kind"], 2, "{completion}");
        assert_eq!(
            completion["textEdit"],
            json!({ "range": range(&start, &end), "newText": "measureHeight" }),
            "{completion}",
        );
        assert_eq!(
            completion["additionalTextEdits"], change["edits"],
            "{completion}"
        );
    }

    // A broken import keeps its diagnostic and is not organized.
    let broken_import = unordered.replacen("distance as d", "missingSelector as d", 1);
    replace_text(&mut client, &document, 17, &broken_import);
    let broken_diagnostics = diagnostics_of(&mut client, &uri(&main), Some(17), |_| true);
    let selector = position(&broken_import, "missingSelector");
    assert!(
        broken_diagnostics.iter().any(|item| item["severity"] == 1
            && item["range"]["start"]["line"] == selector["line"]),
        "{broken_diagnostics:?}",
    );
    assert_eq!(
        client.call("textDocument/codeAction", organize_params),
        json!([]),
    );

    // Renamed files rewrite module paths, unsaved buffers included.
    client.notify("textDocument/didClose", params);
    diagnostics_of(&mut client, &uri(&main), None, <[Value]>::is_empty);
    let palette = root.join("palette.nia");
    write_text(&palette, "pub struct Palette {}\n");
    updated += "module palette;\nfun color() palette::Palette { .{} }\n";
    write_text(&main, &updated);
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [
            { "uri": uri(&main), "type": 2 },
            { "uri": uri(&palette), "type": 1 },
        ]}),
    );
    let unsaved = read_text(&shapes) + "\npub struct BufferOnly {}\n";
    open(&mut client, &shapes, 11, &unsaved);
    diagnostics_of(&mut client, &uri(&shapes), Some(11), <[Value]>::is_empty);
    let mut renamed = None;
    for (old_name, new_name) in [("shapes", "geometry"), ("geometry", "Geometry")] {
        let old_file = root.join(format!("{old_name}.nia"));
        let renamed_file = root.join(format!("{new_name}.nia"));
        let mut pairs = vec![(old_file.clone(), renamed_file.clone())];
        if old_name == "shapes" {
            pairs.push((palette.clone(), root.join("colors.nia")));
        }
        let stem = |path: &Path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .expect("UTF-8 file stem")
                .to_owned()
        };
        let names = pairs
            .iter()
            .map(|(old, new)| (stem(old), stem(new)))
            .collect::<Vec<_>>();
        let files = pairs
            .iter()
            .map(|(old, new)| json!({ "oldUri": uri(old), "newUri": uri(new) }))
            .collect::<Vec<_>>();
        let move_params = json!({ "files": files });
        let edit = client.call("workspace/willRenameFiles", move_params.clone());
        assert_eq!(keys(&edit), BTreeSet::from(["documentChanges"]), "{edit}");
        let change = single(&edit["documentChanges"]);
        assert_eq!(change["textDocument"]["uri"], uri(&main), "{change}");
        let expected = names
            .iter()
            .flat_map(|(old, new)| {
                std::iter::repeat_n(new.as_str(), if old == "palette" { 2 } else { 3 })
            })
            .collect::<Vec<_>>();
        let new_texts = array(&change["edits"])
            .iter()
            .map(|item| string(&item["newText"]))
            .collect::<Vec<_>>();
        assert_eq!(new_texts, expected, "{edit}");
        for item in array(&change["edits"]) {
            let (start, end) = (&item["range"]["start"], &item["range"]["end"]);
            assert_eq!(start["line"], end["line"], "{item}");
            let replaced = &updated[byte_offset(&updated, start)..byte_offset(&updated, end)];
            let new_name = names
                .iter()
                .find(|(old, _)| old == replaced)
                .map(|(_, new)| new.as_str());
            assert_eq!(Some(string(&item["newText"])), new_name, "{item}");
        }
        updated = apply_edits(&updated, array(&change["edits"]));
        write_text(&main, &updated);
        for (old, new) in &pairs {
            rename(old, new);
        }
        let entries = std::fs::read_dir(root)
            .expect("list the workspace")
            .map(|entry| entry.expect("workspace entry").file_name())
            .collect::<Vec<_>>();
        assert!(
            entries
                .iter()
                .any(|name| *name == *renamed_file.file_name().expect("file name")),
            "{entries:?}",
        );
        client.notify("workspace/didRenameFiles", move_params);
        diagnostics_of(&mut client, &uri(&old_file), None, <[Value]>::is_empty);
        diagnostics_of(
            &mut client,
            &uri(&renamed_file),
            Some(11),
            <[Value]>::is_empty,
        );
        for query in [
            format!("fixture::{new_name}::Shape"),
            format!("fixture::{new_name}::BufferOnly"),
        ] {
            let symbols = client.call("workspace/symbol", json!({ "query": query }));
            assert_eq!(symbol_uris(&symbols), [uri(&renamed_file)], "{symbols}");
        }
        let symbols = client.call(
            "workspace/symbol",
            json!({ "query": "fixture::colors::Palette" }),
        );
        assert_eq!(
            symbol_uris(&symbols),
            [uri(&root.join("colors.nia"))],
            "{symbols}"
        );
        renamed = Some(renamed_file);
    }
    let renamed = renamed.expect("the files were renamed");
    client.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": uri(&renamed) } }),
    );
    diagnostics_of(&mut client, &uri(&renamed), None, <[Value]>::is_empty);
    assert!(is_empty(
        &client.call("workspace/symbol", json!({ "query": "BufferOnly" }))
    ));

    // Replacing the workspace folder replaces the indexed packages.
    let other = root.join("next-workspace");
    create_dir_all(&other);
    write_text(
        &other.join("nia.toml"),
        &manifest.replace("fixture", "next"),
    );
    write_text(&other.join("root.nia"), "pub struct NewWorkspace {}\n");
    client.notify(
        "workspace/didChangeWorkspaceFolders",
        json!({ "event": {
            "added": [{ "uri": uri(&other), "name": "next" }],
            "removed": [{ "uri": uri(root), "name": "fixture" }],
        }}),
    );
    assert!(is_empty(
        &client.call("workspace/symbol", json!({ "query": "Shape" }))
    ));
    let symbols = client.call("workspace/symbol", json!({ "query": "next::NewWorkspace" }));
    assert_eq!(
        symbol_uris(&symbols),
        [uri(&other.join("root.nia"))],
        "{symbols}"
    );
    client.shutdown(TIMEOUT);
}

/// Moving a module file and its child directory rewrites every path to it,
/// in the package, in a dependent package, and in an unsaved buffer.
#[test]
fn module_move() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let workspace = Workspace::new("nia-lsp-move-smoke");
    let root = workspace.root.as_path();
    write_text(
        &root.join("nia.toml"),
        "[workspace]\nmembers = [\"api\", \"client\"]\n",
    );
    let api = root.join("api");
    create_dir_all(&api);
    write_text(
        &api.join("nia.toml"),
        "[package]\nname = \"move_fixture\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
    );
    let downstream = root.join("client/root.nia");
    create_dir_all(downstream.parent().expect("client directory"));
    write_text(
        &downstream.with_file_name("nia.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n\
         [dependencies]\nfacade = { path = \"../api\" }\n",
    );
    let (entry, left, right) = (
        api.join("root.nia"),
        api.join("left.nia"),
        api.join("right.nia"),
    );
    let (old, new) = (api.join("left/value.nia"), api.join("right/amount.nia"));
    let (old_tree, new_tree) = (old.with_extension(""), new.with_extension(""));
    create_dir_all(&old_tree);
    create_dir_all(new.parent().expect("right directory"));
    let mut sources = vec![
        (
            entry.clone(),
            concat!(
                "pub module left;\npub module right;\n",
                "using root::{left::{value as reading, origin}}; // Keep the import comment.\n",
                "fun calculate() i32 { left::value::number() + reading::extra::bonus() + origin() }\n",
            )
            .to_owned(),
        ),
        (
            left.clone(),
            "/// Reading data.\npub module value; // Keep this comment.\npub fun origin() i32 { 40 }\n"
                .to_owned(),
        ),
        (right.clone(), "// Destination\npub fun base() i32 { 3 }\n".to_owned()),
        (
            old.clone(),
            "pub module extra;\npub fun number() i32 { super::origin() }\n".to_owned(),
        ),
        (old_tree.join("extra.nia"), "pub fun bonus() i32 { 2 }\n".to_owned()),
        (
            downstream.clone(),
            concat!(
                "using facade::{left::{value as remote}};\n",
                "fun calculate() i32 { facade::left::value::number() + remote::extra::bonus() }\n",
            )
            .to_owned(),
        ),
    ];
    for (path, text) in &sources {
        write_lf(path, text);
    }
    let source = |sources: &[(std::path::PathBuf, String)], path: &Path| {
        sources
            .iter()
            .find(|(file, _)| file == path)
            .map(|(_, text)| text.clone())
            .expect("a fixture source")
    };
    let unsaved = source(&sources, &old) + "pub struct Unsaved {}\n";
    let mut client = Client::start(root, TIMEOUT);
    client.call(
        "initialize",
        json!({
            "processId": null,
            "rootUri": uri(root),
            "capabilities": { "workspace": {
                "fileOperations": { "willRename": true, "didRename": true },
                "workspaceEdit": { "documentChanges": true },
            }},
        }),
    );
    client.notify("initialized", json!({}));
    open(&mut client, &old, 5, &unsaved);
    diagnostics_of(&mut client, &uri(&old), Some(5), <[Value]>::is_empty);
    let move_params = json!({ "files": [
        { "oldUri": uri(&old), "newUri": uri(&new) },
        { "oldUri": uri(&old_tree), "newUri": uri(&new_tree) },
    ]});
    let result = client.call("workspace/willRenameFiles", move_params.clone());
    for change in array(&result["documentChanges"]) {
        let changed = string(&change["textDocument"]["uri"]);
        let index = sources
            .iter()
            .position(|(path, _)| uri(path) == changed)
            .unwrap_or_else(|| panic!("an edit of an unexpected file: {change}"));
        let path = sources[index].0.clone();
        let text = if path == old {
            &unsaved
        } else {
            &sources[index].1
        };
        let text = apply_edits(text, array(&change["edits"]));
        if path == old {
            assert_eq!(change["textDocument"]["version"], 5, "{change}");
            assert!(text.contains("root::left::origin()"), "{text}");
            client.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri(&old), "version": 6 },
                    "contentChanges": [{ "text": text }],
                }),
            );
        } else {
            write_lf(&path, &text);
            sources[index].1 = text;
        }
    }
    assert_eq!(source(&sources, &left), "pub fun origin() i32 { 40 }\n");
    assert!(
        source(&sources, &right)
            .ends_with("/// Reading data.\npub module amount; // Keep this comment.\n"),
        "{}",
        source(&sources, &right),
    );
    let entry_text = source(&sources, &entry);
    assert!(
        entry_text.contains("root::right::amount::number() + reading::extra::bonus() + origin()"),
        "{entry_text}",
    );
    assert!(
        entry_text.contains("using root::{left::{origin}}; // Keep the import comment."),
        "{entry_text}",
    );
    assert!(
        entry_text.contains("using root::right::amount as reading;"),
        "{entry_text}"
    );
    let downstream_text = source(&sources, &downstream);
    assert!(
        downstream_text.contains("using facade::right::amount as remote;"),
        "{downstream_text}",
    );
    assert!(
        downstream_text.contains("facade::right::amount::number()"),
        "{downstream_text}",
    );
    rename(&old, &new);
    rename(&old_tree, &new_tree);
    client.notify("workspace/didRenameFiles", move_params);
    diagnostics_of(&mut client, &uri(&new), Some(6), <[Value]>::is_empty);
    open(&mut client, &entry, 1, &entry_text);
    let target = definition(&mut client, &entry, &entry_text, "bonus()");
    assert_eq!(target["uri"], uri(&new_tree.join("extra.nia")), "{target}");
    open(&mut client, &downstream, 1, &downstream_text);
    let target = definition(&mut client, &downstream, &downstream_text, "number()");
    assert_eq!(target["uri"], uri(&new), "{target}");
    let symbols = client.call("workspace/symbol", json!({ "query": "Unsaved" }));
    assert_eq!(symbol_uris(&symbols), [uri(&new)], "{symbols}");
    client.shutdown(Duration::from_secs(30));
}

/// How a package-move scenario is set up.
#[derive(Clone, Copy, Default)]
struct PackageMove {
    /// Move a file of `api` that refers back to its original package.
    retain_original: bool,
    /// The destination package does not yet depend on the source, so the
    /// move adds the dependency to its manifest.
    add_dependency: bool,
    /// The source package has a build script, so its module graph comes from
    /// the build plan.
    configured: bool,
}

/// Moving a file into another package rewrites paths across packages and
/// adds the dependencies the move needs.
fn package_move(name: &str, scenario: PackageMove) {
    let PackageMove {
        retain_original,
        add_dependency,
        configured,
    } = scenario;
    let _resources = acquire_test_resources(if configured {
        TestWorkload::Build
    } else {
        TestWorkload::Compiler
    });
    let workspace = Workspace::new(name);
    let root = workspace.root.as_path();
    let add_caller = add_dependency && !retain_original;
    let members = if add_caller {
        "[\"api\", \"kit\", \"client\"]"
    } else {
        "[\"api\", \"kit\"]"
    };
    write_text(
        &root.join("nia.toml"),
        &format!("[workspace]\nmembers = {members}\n"),
    );
    let (api, kit) = (root.join("api/root.nia"), root.join("kit/root.nia"));
    for (entry, package) in [(&api, "api"), (&kit, "kit")] {
        create_dir_all(entry.parent().expect("package directory"));
        let mut manifest = format!(
            "[package]\nname = \"{package}\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n"
        );
        if retain_original && entry == &kit && !add_dependency {
            manifest += "[dependencies]\noriginal = { path = \"../api\" }\n";
        } else if !retain_original && entry == &api && !add_dependency {
            manifest += "[dependencies]\nhelper = { path = \"../kit\" }\n";
        }
        write_text(&entry.with_file_name("nia.toml"), &manifest);
    }
    let (old, new) = (
        api.with_file_name("data.nia"),
        kit.with_file_name("reading.nia"),
    );
    let mut sources: Vec<(std::path::PathBuf, String)> = vec![
        (
            api.clone(),
            "pub module data;\nfun run() i32 { data::value() + helper::base() }\n".to_owned(),
        ),
        (kit.clone(), "pub fun base() i32 { 12 }\n".to_owned()),
        (
            old.clone(),
            "fun local() i32 { 42 }\npub fun value() i32 { root::data::local() }\n".to_owned(),
        ),
    ];
    if retain_original {
        sources[0].1 = "pub module data;\npub fun base() i32 { 42 }\n".to_owned();
        sources[2].1 = "pub fun value() i32 { root::base() + super::base() }\n".to_owned();
    } else if add_dependency {
        sources[0].1 =
            "pub module data;\nfun run() i32 { data::value() }\npub fun base() i32 { 12 }\n"
                .to_owned();
    }
    let downstream = root.join("client/root.nia");
    let downstream_manifest = downstream.with_file_name("nia.toml");
    if add_caller {
        create_dir_all(downstream.parent().expect("client directory"));
        sources.push((
            downstream_manifest.clone(),
            "[package]\nname = \"client\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n\
             [dependencies]\nkit = { path = \"../api\" } # Existing alias for api\n"
                .to_owned(),
        ));
        sources.push((
            downstream.clone(),
            "fun run() i32 { kit::data::value() + kit::base() }\n".to_owned(),
        ));
    }
    let extra = api.with_file_name("extra.nia");
    if configured {
        sources.push((extra.clone(), "pub fun offset() i32 { 3 }\n".to_owned()));
        sources[0].1 = sources[0]
            .1
            .replace("data::value()", "data::value() + extra::offset()");
        sources.push((
            api.with_file_name("build.nia"),
            r#"using std::build;
using std::fs;
pub fun build(b: &mut build::Build) build::Error!() {
    let imports = [build::ModuleImport::init("extra", fs::PathView::init("extra.nia"))];
    let library = b.addModule(build::ModuleOptions::init("package_check", fs::PathView::init("root.nia")).withImports(imports)).?;
    _ = b.addCheckModuleStep("check", library).?;
    !()
}
"#
            .to_owned(),
        ));
    }
    for (file, text) in &sources {
        write_lf(file, text);
    }
    let api_dir = api.parent().expect("api directory").to_path_buf();
    let check = || {
        let output = nia_command()
            .args(["build", "check"])
            .current_dir(&api_dir)
            .output_timeout_in_session("nia build check");
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(root.join("build/out/api/build-plan.bin").is_file());
    };
    if configured {
        check();
    }
    let manifest = if retain_original { &kit } else { &api }.with_file_name("nia.toml");
    if add_dependency {
        sources.push((manifest.clone(), read_text(&manifest)));
    }
    let text_of = |sources: &[(std::path::PathBuf, String)], path: &Path| {
        sources
            .iter()
            .find(|(file, _)| file == path)
            .map(|(_, text)| text.clone())
            .expect("a fixture source")
    };
    let mut client = Client::start(root, TIMEOUT);
    client.call(
        "initialize",
        json!({
            "processId": null,
            "rootUri": uri(root),
            "capabilities": { "workspace": {
                "fileOperations": { "willRename": true, "didRename": true },
                "workspaceEdit": { "documentChanges": true },
            }},
        }),
    );
    client.notify("initialized", json!({}));
    let move_params = json!({ "files": [{ "oldUri": uri(&old), "newUri": uri(&new) }] });
    let result = client.call("workspace/willRenameFiles", move_params.clone());
    // Manifests change only through the returned edit.
    if add_dependency {
        assert_eq!(read_text(&manifest), text_of(&sources, &manifest));
    }
    if add_caller {
        assert_eq!(
            read_text(&downstream_manifest),
            text_of(&sources, &downstream_manifest),
        );
    }
    for change in array(&result["documentChanges"]) {
        let changed = string(&change["textDocument"]["uri"]);
        let index = sources
            .iter()
            .position(|(path, _)| uri(path) == changed)
            .unwrap_or_else(|| panic!("an edit of an unexpected file: {change}"));
        let text = apply_edits(&sources[index].1, array(&change["edits"]));
        write_lf(&sources[index].0, &text);
        sources[index].1 = text;
    }
    let api_text = text_of(&sources, &api);
    let old_text = text_of(&sources, &old);
    // The manifest that gains a dependency, when the move adds one.
    let manifest_text = if add_dependency {
        text_of(&sources, &manifest)
    } else {
        String::new()
    };
    if retain_original {
        let alias = if add_dependency { "api" } else { "original" };
        assert_eq!(
            old_text.matches(&format!("{alias}::base()")).count(),
            2,
            "{old_text}"
        );
        if add_dependency {
            assert!(
                manifest_text.contains("api = { path = \"../api\" }"),
                "{manifest_text}",
            );
        }
    } else {
        let alias = if add_dependency { "kit" } else { "helper" };
        assert!(
            api_text.contains(&format!("{alias}::reading::value()")),
            "{api_text}"
        );
        if add_dependency {
            assert!(
                manifest_text.contains("kit = { path = \"../kit\" }"),
                "{manifest_text}",
            );
        }
        if add_caller {
            let downstream_text = text_of(&sources, &downstream);
            assert!(
                downstream_text.contains("kit_2::reading::value() + kit::base()"),
                "{downstream_text}",
            );
            let downstream_manifest_text = text_of(&sources, &downstream_manifest);
            assert!(
                downstream_manifest_text
                    .contains("kit = { path = \"../api\" } # Existing alias for api"),
                "{downstream_manifest_text}",
            );
            assert!(
                downstream_manifest_text.contains("kit_2 = { path = \"../kit\" }"),
                "{downstream_manifest_text}",
            );
        }
        assert!(old_text.contains("root::reading::local()"), "{old_text}");
    }
    rename(&old, &new);
    for (path, _) in &mut sources {
        if *path == old {
            path.clone_from(&new);
        }
    }
    if configured {
        check();
    }
    client.notify("workspace/didRenameFiles", move_params);
    let mut lookups = if retain_original {
        vec![(new.clone(), "base()", api.clone())]
    } else {
        vec![(api.clone(), "value()", new.clone())]
    };
    if add_caller {
        lookups.push((downstream.clone(), "value()", new.clone()));
    }
    for (caller, needle, declaration) in lookups {
        let text = text_of(&sources, &caller);
        open(&mut client, &caller, 1, &text);
        diagnostics_of(&mut client, &uri(&caller), Some(1), <[Value]>::is_empty);
        let target = definition(&mut client, &caller, &text, needle);
        assert_eq!(target["uri"], uri(&declaration), "{target}");
    }
    if configured {
        let target = definition(&mut client, &api, &text_of(&sources, &api), "offset()");
        assert_eq!(target["uri"], uri(&extra), "{target}");
    }
    client.shutdown(Duration::from_secs(30));
}

#[test]
fn package_move_to_dependency() {
    package_move("nia-lsp-package-move-smoke", PackageMove::default());
}

#[test]
fn package_move_retaining_original() {
    package_move(
        "nia-lsp-package-move-original-smoke",
        PackageMove {
            retain_original: true,
            ..PackageMove::default()
        },
    );
}

#[test]
fn package_move_adding_dependency_on_original() {
    package_move(
        "nia-lsp-package-move-dependency-smoke",
        PackageMove {
            retain_original: true,
            add_dependency: true,
            ..PackageMove::default()
        },
    );
}

#[test]
fn package_move_adding_caller_dependency() {
    package_move(
        "nia-lsp-package-move-caller-dependency-smoke",
        PackageMove {
            add_dependency: true,
            ..PackageMove::default()
        },
    );
}

#[test]
fn package_move_in_configured_package() {
    package_move(
        "nia-lsp-configured-package-move-smoke",
        PackageMove {
            add_dependency: true,
            configured: true,
            ..PackageMove::default()
        },
    );
}

/// Navigation follows the configuration of the last `nia build`: profile,
/// target, CPU level, generated modules, build dependencies, and tests.
#[test]
fn build_context() {
    let _resources = acquire_test_resources(TestWorkload::Build);
    let workspace = Workspace::new("nia-lsp-build-smoke");
    let root = workspace.root.as_path();
    let member = root.join("app");
    let write = |path: &str, text: &str| {
        let file = root.join(path);
        create_dir_all(file.parent().expect("parent directory"));
        write_lf(&file, text);
        file
    };
    write("nia.toml", "[workspace]\nmembers = [\"app\", \"flags\"]\n");
    let flags = write("flags/root.nia", "pub fun marker() i32 { 1 }\n");
    write(
        "flags/nia.toml",
        "[package]\nname = \"flags\"\nversion = \"1.0.0\"\n[library]\npath = \"root.nia\"\n",
    );
    write(
        "app/nia.toml",
        r#"[package]
name = "matrix"
version = "1.0.0"
[library]
path = "root.nia"
[[test]]
name = "unit"
path = "unit.nia"
[build-dependencies]
flags = { path = "../flags" }
"#,
    );
    let library = write("app/root.nia", "pub fun answer() i32 { 42 }\n");
    let unit_src = r#"using matrix;
using std::process;
@[test] fun test_value() i32 { matrix::answer() }
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    _ = test_value();
    !()
}
"#;
    let unit = write("app/unit.nia", unit_src);
    let build_src = r#"using std::build;
using std::fs;
using flags;
pub fun build(b: &mut build::Build) build::Error!() {
    _ = flags::marker();
    let generated = build::BuildPathView::init("generated.nia");
    _ = b.addGeneratedFileStep("generate", generated, b"pub fun marker() i32 { 1 }\n").?;
    let imports = [build::ModuleImport::fromBuild("generated", generated)];
    let app = b.addModule(build::ModuleOptions::init("app", fs::PathView::init("main.nia")).withImports(imports)).?;
    let exe = b.addExecutable(build::ExecutableOptions::init("app", app)).?;
    _ = b.addCheckExecutableStep("check", exe).?;
    !()
}
"#;
    let build = write("app/build.nia", build_src);
    let app_src = r#"using generated;
using std::process;
@[debug] @[if os == "windows" and not hasFeature("avx2")] fun selected() i32 { 0 }
@[release] @[if os == "linux" and hasFeature("avx2")] fun selected() i32 { 1 }
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    _ = selected();
    _ = generated::marker();
    !()
}
"#;
    let app = write("app/main.nia", app_src);
    let plan = root.join("build/out/app/build-plan.bin");
    let generated = plan.with_file_name("generated.nia");

    // Runs a build step and returns the diagnostics of a failed one.
    let run = |step: &str, args: &[&str], status: i32| -> Vec<Value> {
        let output = nia_command()
            .args(["build", step, "--diagnostics-format=json"])
            .args(args)
            .current_dir(&member)
            .output_timeout_in_session(&format!("nia build {step}"));
        assert_eq!(
            output.status.code(),
            Some(status),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(plan.is_file(), "{}", plan.display());
        if status == 0 {
            return Vec::new();
        }
        let report: Value =
            serde_json::from_slice(&output.stderr).expect("a JSON diagnostics report");
        array(&report["diagnostics"]).to_vec()
    };

    run("library", &[], 0);
    run(
        "check",
        &[
            "--debug",
            "--cpu",
            "baseline",
            "--target",
            "x86_64-pc-windows-msvc",
        ],
        0,
    );
    let mut client = Client::start(root, TIMEOUT);
    client.call(
        "initialize",
        json!({ "processId": null, "rootUri": uri(root), "capabilities": {} }),
    );
    client.notify("initialized", json!({}));
    let opened = |client: &mut Client, file: &Path, source: &str| {
        open(client, file, 1, source);
        diagnostics_of(client, &uri(file), Some(1), |_| true)
    };
    let changed = |client: &mut Client, file: &Path| {
        client.notify(
            "workspace/didChangeWatchedFiles",
            json!({ "changes": [{ "uri": uri(file), "type": 2 }] }),
        );
    };
    let definition_line = |client: &mut Client, file: &Path, source: &str, needle: &str| {
        integer(&definition(client, file, source, needle)["range"]["start"]["line"])
    };
    let reference_lines = |references: &Value| {
        array(references)
            .iter()
            .map(|item| integer(&item["range"]["start"]["line"]))
            .collect::<BTreeSet<_>>()
    };

    assert_eq!(opened(&mut client, &app, app_src), Vec::<Value>::new());
    assert_eq!(
        definition_line(&mut client, &app, app_src, "selected();"),
        2
    );
    assert_eq!(
        definition(&mut client, &app, app_src, "marker();")["uri"],
        uri(&generated),
    );
    assert_eq!(opened(&mut client, &build, build_src), Vec::<Value>::new());
    assert_eq!(
        definition(&mut client, &build, build_src, "marker();")["uri"],
        uri(&flags),
    );
    let selected = json!({
        "textDocument": { "uri": uri(&app) },
        "position": position(app_src, "selected();"),
        "context": { "includeDeclaration": true },
    });
    let rename_params = json!({
        "textDocument": selected["textDocument"],
        "position": position(app_src, "init:"),
        "newName": "context",
    });
    let initial = client.call("textDocument/rename", rename_params.clone());
    assert_eq!(array(&initial["changes"][uri(&app)]).len(), 2, "{initial}");
    let references = client.call("textDocument/references", selected.clone());
    assert_eq!(
        reference_lines(&references),
        BTreeSet::from([2, 6]),
        "{references}"
    );
    assert_eq!(
        definition_line(&mut client, &app, app_src, "selected();"),
        2
    );

    // A build with another configuration replaces the configuration the
    // server navigates by, and a rename started before it is rejected.
    run(
        "check",
        &[
            "--release",
            "--cpu",
            "x86_64-v3",
            "--target",
            "x86_64-unknown-linux",
        ],
        0,
    );
    let rejected = client.request("textDocument/rename", rename_params.clone());
    let rejected = client.wait(rejected);
    assert!(
        rejected["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("configuration changed")),
        "{rejected}",
    );
    let target = definition(&mut client, &app, app_src, "selected();");
    assert_eq!(target["range"]["start"]["line"], 3, "{target}");
    let references = client.call("textDocument/references", selected.clone());
    assert_eq!(
        reference_lines(&references),
        BTreeSet::from([3, 6]),
        "{references}"
    );
    assert_eq!(client.call("textDocument/rename", rename_params), initial);
    changed(&mut client, &plan);
    assert_eq!(
        definition_line(&mut client, &app, app_src, "selected();"),
        3
    );
    assert_eq!(
        definition(&mut client, &app, app_src, "marker();")["uri"],
        uri(&generated),
    );

    // A failed build publishes its diagnostics on the source it names.
    let build_errors = run(
        "check",
        &[
            "--release",
            "--cpu",
            "baseline",
            "--target",
            "x86_64-unknown-linux",
        ],
        1,
    );
    let [build_error] = build_errors.as_slice() else {
        panic!("expected one build diagnostic, got {build_errors:?}");
    };
    assert_eq!(
        support::physical_path(Path::new(string(&build_error["path"]))),
        app,
        "{build_error}",
    );
    let summary = string(&build_error["summary"]);
    assert!(summary.contains("selected"), "{build_error}");
    changed(&mut client, &plan);
    let diagnostics = diagnostics_of(&mut client, &uri(&app), Some(1), |items| {
        has_error(items, summary)
    });
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("expected one diagnostic, got {diagnostics:?}");
    };
    assert_eq!(
        diagnostic["code"], build_error["code"],
        "{diagnostic} {build_error}"
    );
    assert_eq!(
        diagnostic["range"]["start"],
        position(app_src, "selected();"),
        "{diagnostic}",
    );

    // A test artifact's build opens its root to navigation.
    run("unit", &[], 0);
    changed(&mut client, &plan);
    assert_eq!(opened(&mut client, &unit, unit_src), Vec::<Value>::new());
    assert_eq!(
        definition(&mut client, &unit, unit_src, "answer()")["uri"],
        uri(&library),
    );
    assert_eq!(
        definition_line(&mut client, &unit, unit_src, "test_value();"),
        2
    );
    client.shutdown(Duration::from_secs(30));
}

/// The code generator of the generated-chain fixture; see the file.
const GENERATOR_SOURCE: &str = include_str!("fixtures/generate.nia");

/// Navigation and renames reach through a chain of two source-mapped
/// external commands to the input they were generated from, and refuse to
/// rename what the mapping cannot account for.
#[test]
fn generated_chain() {
    let _resources = acquire_test_resources(TestWorkload::Build);
    let tools = Workspace::new("nia-lsp-generator");
    let generator_name = format!("generate{}", std::env::consts::EXE_SUFFIX);
    let generator_source = tools.root.join("generate.nia");
    write_lf(&generator_source, GENERATOR_SOURCE);
    let output = nia_command()
        .args(["emit", "--exe"])
        .arg(&generator_source)
        .arg("-o")
        .arg(tools.root.join(&generator_name))
        .output_timeout_in_session("nia emit --exe generator");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let workspace = Workspace::new("nia-lsp-generated-chain");
    let root = workspace.root.as_path();
    write_text(
        &root.join("nia.toml"),
        "[package]\nname = \"mapping\"\nversion = \"0.1.0\"\n",
    );
    let (origin, entry) = (root.join("input.nia"), root.join("root.nia"));
    // The generator publishes the BLAKE3 digests of these exact contents.
    write_bytes(&origin, b"// input\npub fun amount() i32 { 42 }\n");
    let source = "fun run() i32 { generated::amount() }\n";
    write_text(&entry, source);
    let build = r#"using std::build;
using std::fs;
pub fun build(b: &mut build::Build) build::Error!() {
    let first = build::BuildPathView::init("first.txt");
    let second = build::BuildPathView::init("second.nia");
    let firstArgs = [build::CommandArgument::literal("1"),
        build::CommandArgument::packageInput(b.rootPackage(), fs::PathView::init("input.nia")),
        build::CommandArgument::buildOutput(first),
        build::CommandArgument::buildOutput(build::BuildPathView::init("first.txt.map"))];
    let secondArgs = [build::CommandArgument::literal("2"),
        build::CommandArgument::buildInput(first), build::CommandArgument::buildOutput(second),
        build::CommandArgument::buildOutput(build::BuildPathView::init("second.nia.map")),
        build::CommandArgument::buildOutput(build::BuildPathView::init("unused.txt")),
        build::CommandArgument::buildOutput(build::BuildPathView::init("unused.txt.map"))];
    let prepare = b.addExternalCommandStep("first", build::ExternalCommandOptions::search("__GENERATOR__").withArguments(firstArgs)).?;
    let generate = b.addExternalCommandStep("second", build::ExternalCommandOptions::search("__GENERATOR__").withArguments(secondArgs)).?;
    b.dependOn(generate, prepare).?;
    let imports = [build::ModuleImport::fromBuild("generated", second)];
    let app = b.addModule(build::ModuleOptions::init("app", fs::PathView::init("root.nia")).withImports(imports)).?;
    let check = b.addCheckModuleStep("check", app).?;
    b.dependOn(check, generate).?;
    !()
}
"#;
    write_text(
        &root.join("build.nia"),
        &build.replace("__GENERATOR__", &generator_name),
    );
    let search_path = path_with(&tools.root);
    let check = || {
        let output = nia_command()
            .args(["build", "check"])
            .current_dir(root)
            .env("PATH", &search_path)
            .output_timeout_in_session("nia build check");
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    };

    check();
    let generated = root.join("build/out/second.nia");
    let mut client = Client::start(root, TIMEOUT);
    client.call(
        "initialize",
        json!({ "processId": null, "rootUri": uri(root), "capabilities": {} }),
    );
    client.notify("initialized", json!({}));
    open(&mut client, &entry, 1, source);
    let definition_params = json!({
        "textDocument": { "uri": uri(&entry) },
        "position": position(source, "amount"),
    });
    let target = client.call("textDocument/definition", definition_params.clone());
    assert_eq!(
        target,
        json!({ "uri": uri(&origin), "range": range(&point(1, 8), &point(1, 14)) }),
    );
    // A changed input or a broken map leaves navigation in the generated file.
    let original = read_bytes(&origin);
    write_bytes(&origin, &[b"// changed\n".as_slice(), &original].concat());
    assert_eq!(
        client.call("textDocument/definition", definition_params.clone())["uri"],
        uri(&generated),
    );
    write_bytes(&origin, &original);
    let first_map = generated.with_file_name("first.txt.map");
    let map_text = read_bytes(&first_map);
    write_bytes(&first_map, b"{}");
    assert_eq!(
        client.call("textDocument/definition", definition_params.clone())["uri"],
        uri(&generated),
    );
    write_bytes(&first_map, &map_text);
    assert_eq!(
        client.call("textDocument/definition", definition_params.clone()),
        target,
    );

    // Renaming at the input reaches an intermediate output it cannot edit.
    let origin_text = String::from_utf8(original.clone()).expect("UTF-8 input");
    let origin_document = json!({ "uri": uri(&origin) });
    open(&mut client, &origin, 1, &origin_text);
    let origin_rename = json!({
        "textDocument": origin_document,
        "position": position(&origin_text, "amount"),
        "newName": "total",
    });
    let rejected = client.request("textDocument/rename", origin_rename);
    let rejected = client.wait(rejected);
    let message = rejected["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("another generated occurrence") && message.contains("first.txt"),
        "{rejected}",
    );
    client.notify(
        "textDocument/didClose",
        json!({ "textDocument": origin_document }),
    );

    // A stale sibling output with the same mapping blocks the rename until
    // the next build replaces it.
    let mut rename_params = definition_params.clone();
    rename_params["newName"] = json!("total");
    let sibling = generated.with_file_name("unused.txt");
    let sibling_map = generated.with_file_name("unused.txt.map");
    write_bytes(&sibling, &read_bytes(&generated));
    write_bytes(
        &sibling_map,
        &read_bytes(&generated.with_file_name("second.nia.map")),
    );
    let rejected = client.request("textDocument/rename", rename_params.clone());
    let rejected = client.wait(rejected);
    let message = rejected["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("another generated occurrence") && message.contains("unused.txt"),
        "{rejected}",
    );
    check();
    let result = client.call("textDocument/rename", rename_params);
    let changes = &result["changes"];
    assert_eq!(
        keys(changes),
        BTreeSet::from([uri(&entry).as_str(), uri(&origin).as_str()]),
        "{changes}",
    );
    assert_eq!(
        changes[uri(&origin)],
        json!([{ "range": target["range"], "newText": "total" }]),
    );
    let start = position(source, "amount");
    assert_eq!(
        changes[uri(&entry)],
        json!([{ "range": range(&start, &shifted(&start, 6)), "newText": "total" }]),
    );
    let contains = |haystack: &[u8], needle: &[u8]| {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    };
    assert!(contains(&read_bytes(&generated), b"fun amount()"));
    // Applying the rename to the sources renames the generated code.
    write_bytes(
        &origin,
        String::from_utf8(original)
            .expect("UTF-8 input")
            .replace("amount", "total")
            .as_bytes(),
    );
    write_text(&entry, &source.replace("amount", "total"));
    check();
    assert!(contains(
        &read_bytes(&generated.with_file_name("first.txt")),
        b"fun total()",
    ));
    assert!(contains(&read_bytes(&generated), b"fun total()"));
    client.shutdown(Duration::from_secs(30));
}

/// A function body of hundreds of calls is navigated, highlighted, and
/// rechecked after edits.
#[test]
fn long_expression() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let workspace = Workspace::new("nia-lsp-long-expression");
    let root = workspace.root.as_path();
    let (entry, helper) = (root.join("root.nia"), root.join("helper.nia"));
    write_text(
        &root.join("nia.toml"),
        "[package]\nname = \"long_expression\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
    );
    write_text(&helper, "pub fun value() i32 { 1 }\n");
    let terms = vec!["helper::value()"; 576].join(" + ");
    let source = format!("module helper;\npub fun probe() i32 {{ {terms} }}\n");
    let nested = format!(
        "pub fun probe() i32 {{ {}1{} }}",
        "(".repeat(64),
        ")".repeat(64)
    );
    write_text(&entry, &nested);
    let rejected = nia_command()
        .arg("check")
        .arg(&entry)
        .current_dir(root)
        .output_timeout_in_session("nia check");
    assert_eq!(rejected.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("nesting limit"));
    write_text(&entry, &source);
    let output = nia_command()
        .arg("check")
        .arg(&entry)
        .current_dir(root)
        .output_timeout_in_session("nia check");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let mut client = Client::start(root, TIMEOUT);
    client.call(
        "initialize",
        json!({ "rootUri": uri(root), "capabilities": {} }),
    );
    client.notify("initialized", json!({}));
    let document = json!({ "uri": uri(&entry) });
    open(&mut client, &entry, 1, &source);
    let target = definition(&mut client, &entry, &source, "value()");
    assert_eq!(target["uri"], uri(&helper), "{target}");
    let tokens = client.call(
        "textDocument/semanticTokens/full",
        json!({ "textDocument": document }),
    );
    assert!(!is_empty(&tokens["data"]), "{tokens}");
    let diagnostics = diagnostics_of(&mut client, &uri(&entry), Some(1), |_| true);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    for (version, text) in [
        (2, source.replacen("value()", "missing()", 1)),
        (3, source.clone()),
        (4, nested),
        (5, source.clone()),
    ] {
        replace_text(&mut client, &document, version, &text);
        let diagnostics = diagnostics_of(&mut client, &uri(&entry), Some(version), |_| true);
        if version == 2 {
            assert!(mentions(&diagnostics, "missing"), "{diagnostics:?}");
        } else if version == 4 {
            assert!(mentions(&diagnostics, "nesting limit"), "{diagnostics:?}");
        } else {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
        }
    }
    client.shutdown(Duration::from_secs(30));
}
