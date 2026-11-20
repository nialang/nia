// SPDX-License-Identifier: GPL-3.0-or-later
//! Incremental analysis equals clean analysis.
//!
//! Each test edits one snapshot of a workspace in a running server, then
//! reverts it, and compares what the server reports after each step with a
//! fresh session started on that snapshot: definitions, completions,
//! diagnostics, and published semantic tokens.

mod support;

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use nia_test_support::{TestWorkload, acquire_test_resources};
use serde_json::{Map, Value, json};
use support::{
    Client, Workspace, array, create_dir_all, is_empty, position, read_text, resource_root,
    shifted, string, uri, write_text,
};

const TIMEOUT: Duration = Duration::from_secs(120);

/// The text of every open document, in the order they are opened.
type Snapshot = Vec<(PathBuf, String)>;

/// A workspace, the document whose results are compared, its original
/// snapshot, and one edit of it.
struct Workload {
    root: PathBuf,
    source: PathBuf,
    original: Snapshot,
    edit: (PathBuf, String),
    /// Where to ask for a definition and a completion: a needle in the
    /// source and the name in it. The first must resolve.
    probes: Vec<(&'static str, &'static str)>,
    _workspace: Option<Workspace>,
}

/// Writes a workspace with an application package that depends on a
/// library, and returns the application's root file.
fn fixture(workspace: &Path) -> PathBuf {
    for package in ["app", "dep"] {
        create_dir_all(&workspace.join(package));
        let dependencies = if package == "app" {
            "[dependencies]\nutil = { path = \"../dep\" }\n"
        } else {
            ""
        };
        write_text(
            &workspace.join(package).join("nia.toml"),
            &format!(
                "[package]\nname = \"{package}\"\nversion = \"0.1.0\"\n\
                 [library]\npath = \"root.nia\"\n{dependencies}"
            ),
        );
    }
    write_text(
        &workspace.join("nia.toml"),
        "[workspace]\nmembers = [\"app\", \"dep\"]\n",
    );
    let dependency = concat!(
        "pub fun amount() i32 { 42 }\n",
        "pub struct Counter { value: i32 }\n",
        "extend Counter { pub fun measure(&self) i32 { self.value } }\n",
    );
    write_text(&workspace.join("dep/root.nia"), dependency);
    let source = workspace.join("app/root.nia");
    write_text(
        &source,
        concat!(
            "using util;\n",
            "pub fun result() i32 { util::amount() + 1 }\n",
            "pub fun observe(counter: &util::Counter) i32 { counter.measure() }\n",
            "pub fun unaffected() i32 { util::amount() }\n",
        ),
    );
    source
}

fn replace_once(text: &str, old: &str, new: &str) -> String {
    assert_eq!(
        text.matches(old).count(),
        1,
        "expected one occurrence of {old:?}"
    );
    text.replacen(old, new, 1)
}

/// The edits of one file: a comment that moves every position, a changed
/// body, and a changed signature.
fn file_workload(
    root: PathBuf,
    source: PathBuf,
    edit: &str,
    body: (&str, &str),
    signature: (&str, &str),
    probe: (&'static str, &'static str),
    workspace: Option<Workspace>,
) -> Workload {
    let clean = read_text(&source);
    let edited = match edit {
        "comment" => format!("// Position-changing audit edit\n{clean}"),
        "body" => replace_once(&clean, body.0, body.1),
        "signature" => replace_once(&clean, signature.0, signature.1),
        _ => panic!("unknown edit {edit}"),
    };
    Workload {
        root,
        original: vec![(source.clone(), clean)],
        edit: (source.clone(), edited),
        source,
        probes: vec![probe],
        _workspace: workspace,
    }
}

/// A workspace of two packages.
fn workspace_workload(edit: &str) -> Workload {
    let workspace = Workspace::new("nia-lsp-incremental");
    let source = fixture(&workspace.root);
    file_workload(
        workspace.root.clone(),
        source,
        edit,
        ("util::amount() + 1", "util::amount() + 2"),
        ("fun result() i32", "fun result() i64"),
        ("util::amount() +", "amount"),
        Some(workspace),
    )
}

/// The standard library's `ArrayList`, a large module with many callers.
fn array_list_workload(edit: &str) -> Workload {
    let root = resource_root().join("std");
    let source = root.join("collections/array_list/list.nia");
    file_workload(
        root,
        source,
        edit,
        ("self.len() == 0", "self.len() != 0"),
        ("fun isEmpty(&self) bool", "fun isEmpty(&self) usize"),
        ("self.len() ", "len"),
        None,
    )
}

/// Edits across packages: a dependency's signature, an extension provider,
/// and a caller's demand for that provider.
fn dependency_workload(edit: &str) -> Workload {
    let workspace = Workspace::new("nia-lsp-incremental");
    let source = fixture(&workspace.root);
    let dependency = workspace.root.join("dep/root.nia");
    let provider = dependency.with_file_name("provider.nia");
    let dependency_text =
        "pub module provider;\npub fun amount() i32 { 42 }\npub struct Counter {}\n";
    let provider_text =
        "using root::Counter;\nextend Counter { pub fun measure(&self) i32 { 42 } }\n";
    write_text(&dependency, dependency_text);
    write_text(&provider, provider_text);
    let source_text = format!("using util::provider;\n{}", read_text(&source));
    let edited = match edit {
        "dependency_signature" => (
            dependency.clone(),
            replace_once(
                dependency_text,
                "amount() i32 { 42 }",
                "amount() bool { true }",
            ),
        ),
        "provider_demand" => (
            source.clone(),
            replace_once(&source_text, "{ counter.measure() }", "{ _ = counter; 42 }"),
        ),
        "provider_definition" => (
            provider.clone(),
            replace_once(provider_text, "fun measure(", "fun renamed("),
        ),
        "provider_position" => (
            provider.clone(),
            format!("// Move the extension method.\n{provider_text}"),
        ),
        "unresolved_name" => (
            source.clone(),
            replace_once(
                &source_text,
                "unaffected() i32 { util::amount() }",
                "unaffected() i32 { unknown_value }",
            ),
        ),
        _ => panic!("unknown edit {edit}"),
    };
    Workload {
        root: workspace.root.clone(),
        original: vec![
            (source.clone(), source_text),
            (dependency, dependency_text.to_owned()),
            (provider, provider_text.to_owned()),
        ],
        edit: edited,
        source,
        probes: vec![
            ("util::amount() +", "amount"),
            ("counter.measure(", "measure"),
        ],
        _workspace: Some(workspace),
    }
}

/// Request parameters at `label` inside the first `needle` of `text`.
fn definition_params(document: &Value, text: &str, needle: &str, label: &str) -> Value {
    let offset = needle
        .find(label)
        .unwrap_or_else(|| panic!("{label:?} is not in {needle:?}"));
    json!({
        "textDocument": document,
        "position": shifted(&position(text, needle), offset as i64),
    })
}

/// A JSON array sorted by each element's canonical text, so results compare
/// regardless of the order the server lists them in.
fn sorted(items: &[Value]) -> Value {
    let mut items = items.to_vec();
    items.sort_by_cached_key(Value::to_string);
    Value::Array(items)
}

/// The definition and the completions at a probe.
fn inspect(client: &mut Client, document: &Value, text: &str, needle: &str, label: &str) -> Value {
    let params = definition_params(document, text, needle, label);
    let definition = client.call("textDocument/definition", params.clone());
    let completion = client.call(
        "textDocument/completion",
        json!({
            "textDocument": document,
            "position": shifted(&params["position"], label.len() as i64),
        }),
    );
    let items = match &completion {
        Value::Object(list) => list.get("items").cloned().unwrap_or(Value::Null),
        _ => completion,
    };
    let items = if is_empty(&items) {
        &[][..]
    } else {
        array(&items)
    };
    json!({ "definition": definition, "completion": sorted(items) })
}

/// Cancels a definition request and checks that it either completes or
/// reports the cancellation.
fn cancel_definition(client: &mut Client, params: Value) {
    let id = client.request("textDocument/definition", params);
    client.notify("$/cancelRequest", json!({ "id": id }));
    let response = client.wait(id);
    if response.get("result").is_none() {
        assert_eq!(
            response["error"]["code"], -32800,
            "unexpected cancellation response: {response}",
        );
    }
}

/// Opens `snapshots` one after another in one session and returns the
/// server's results after each. Only documents whose text changed are sent.
fn session(workload: &Workload, snapshots: &[Snapshot], cancel_retry: bool) -> Vec<Value> {
    let source = &workload.source;
    let document = json!({ "uri": uri(source) });
    let mut client = Client::start(&workload.root, TIMEOUT);
    client.call(
        "initialize",
        json!({ "processId": null, "rootUri": uri(&workload.root), "capabilities": {} }),
    );
    client.notify("initialized", json!({}));
    let mut values = Vec::new();
    let mut previous: &[(PathBuf, String)] = &[];
    let mut versions: Vec<(PathBuf, i64)> = Vec::new();
    for (index, snapshot) in snapshots.iter().enumerate() {
        let started = Instant::now();
        for (path, text) in snapshot {
            if previous
                .iter()
                .any(|(old, old_text)| old == path && old_text == text)
            {
                continue;
            }
            let version = match versions.iter_mut().find(|(opened, _)| opened == path) {
                Some((_, version)) => {
                    *version += 1;
                    *version
                }
                None => {
                    versions.push((path.clone(), 1));
                    1
                }
            };
            if version == 1 {
                client.notify(
                    "textDocument/didOpen",
                    json!({ "textDocument": {
                        "uri": uri(path), "version": version, "languageId": "nia", "text": text,
                    }}),
                );
            } else {
                client.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": uri(path), "version": version },
                        "contentChanges": [{ "text": text }],
                    }),
                );
            }
        }
        let text = &snapshot
            .iter()
            .find(|(path, _)| path == source)
            .expect("every snapshot holds the source")
            .1;
        if cancel_retry && index == 1 {
            let (needle, label) = workload.probes[0];
            cancel_definition(
                &mut client,
                definition_params(&document, text, needle, label),
            );
        }
        // Editing a dependency republishes caller diagnostics under the same
        // caller version, which earlier snapshots may have published too.
        let mut diagnostics = Vec::new();
        for (path, version) in &versions {
            if path != source
                && previous
                    .iter()
                    .any(|old| &old.0 == path && snapshot.contains(old))
            {
                continue;
            }
            let (_, published) =
                client.wait_diagnostics(&uri(path), Some(*version), Some(started), |_| true);
            if path == source {
                diagnostics = published;
            }
        }
        previous = snapshot;
        // Published highlighting is captured before a navigation request
        // can rebuild it.
        let mut tokens = Map::new();
        for (path, _) in &versions {
            let result = client.call(
                "textDocument/semanticTokens/full",
                json!({ "textDocument": { "uri": uri(path) } }),
            );
            tokens.insert(uri(path), result["data"].clone());
        }
        let (needle, label) = workload.probes[0];
        let mut value = inspect(&mut client, &document, text, needle, label);
        let completes = array(&value["completion"])
            .iter()
            .any(|item| item["label"] == label);
        assert!(
            !is_empty(&value["definition"]) && completes,
            "expected a definition and completion at {needle:?}: {value}",
        );
        for &(needle, label) in &workload.probes[1..] {
            if text.contains(needle) {
                value[label] = inspect(&mut client, &document, text, needle, label);
            }
        }
        value["diagnostics"] = sorted(&diagnostics);
        value["semantic_tokens"] = Value::Object(tokens);
        values.push(value);
    }
    client.shutdown(Duration::from_secs(30));
    values
}

/// Whether a result has an error diagnostic.
fn has_errors(value: &Value) -> bool {
    array(&value["diagnostics"])
        .iter()
        .any(|item| item["severity"] == 1)
}

/// The cross-package edits change exactly the results they should.
fn check_dependency_results(edit: &str, original: &Value, edited: &Value, edited_path: &Path) {
    let original_definition = &original["measure"]["definition"];
    assert!(
        !has_errors(original) && !is_empty(original_definition),
        "expected a bound provider in the original workspace: {original}",
    );
    let expected_errors = matches!(
        edit,
        "dependency_signature" | "provider_definition" | "unresolved_name"
    );
    assert_eq!(
        has_errors(edited),
        expected_errors,
        "{edit}: unexpected caller diagnostics: {}",
        edited["diagnostics"],
    );
    if edit == "provider_definition" {
        assert!(
            is_empty(&edited["measure"]["definition"]),
            "{edit}: still resolves the removed method",
        );
    }
    if edit == "provider_demand" {
        let unused = array(&edited["diagnostics"])
            .iter()
            .any(|item| item["code"] == "W0201" && string(&item["message"]).contains("`provider`"));
        assert!(unused, "{edit}: the import is still marked as used");
    }
    if edit == "provider_position" {
        let updated = &edited["measure"]["definition"];
        assert!(
            !is_empty(updated)
                && updated["uri"] == uri(edited_path)
                && updated["range"]["start"]["line"]
                    == json!(support::integer(&original_definition["range"]["start"]["line"]) + 1),
            "{edit}: expected the method definition to move down one line: {updated}",
        );
    }
}

/// Edits a snapshot and reverts it in one session, and checks every result
/// against a fresh session on the same snapshot.
fn audit(name: &str, edit: &str, workload: Workload) {
    let original = workload.original.clone();
    let mut edited = original.clone();
    let (edited_path, edited_text) = &workload.edit;
    for (path, text) in &mut edited {
        if path == edited_path {
            text.clone_from(edited_text);
        }
    }
    let baseline = session(&workload, std::slice::from_ref(&original), false);
    let fresh = session(&workload, std::slice::from_ref(&edited), false);
    if name == "dependencies" {
        check_dependency_results(edit, &baseline[0], &fresh[0], edited_path);
    }
    let incremental = session(&workload, &[original.clone(), edited, original], true);
    assert_eq!(
        incremental[1], fresh[0],
        "{name}/{edit}: the edited snapshot differs from a fresh session",
    );
    for value in [&incremental[0], &incremental[2]] {
        assert_eq!(
            *value, baseline[0],
            "{name}/{edit}: the original snapshot differs from a fresh session",
        );
    }
}

fn audit_workspace(edit: &str) {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    audit("workspace", edit, workspace_workload(edit));
}

fn audit_array_list(edit: &str) {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    audit("array-list", edit, array_list_workload(edit));
}

fn audit_dependencies(edit: &str) {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    audit("dependencies", edit, dependency_workload(edit));
}

#[test]
fn workspace_comment() {
    audit_workspace("comment");
}

#[test]
fn workspace_body() {
    audit_workspace("body");
}

#[test]
fn workspace_signature() {
    audit_workspace("signature");
}

#[test]
fn narrowing_a_facade_import_rechecks_extension_methods() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let workspace = Workspace::new("nia-lsp-facade-import");
    for (path, text) in [
        (
            "nia.toml",
            "[package]\nname='sample'\nversion='1.0.0'\n[library]\npath='root.nia'\n",
        ),
        (
            "root.nia",
            "module geometry; module text; pub module api; module consumer; pub fun amount() usize { 1 }",
        ),
        ("geometry.nia", "pub struct Point { x: i32, y: i32 }"),
        (
            "text.nia",
            "extend [char] { pub fun countOf(&self, target: char) usize { let mut n: usize = 0; for &ch in self { if ch == target { n += 1; } } n } }",
        ),
        (
            "api.nia",
            "using root::geometry; using root::text; pub using geometry::Point; pub using text;",
        ),
    ] {
        write_text(&workspace.root.join(path), text);
    }
    let source = workspace.root.join("consumer.nia");
    let original =
        "using root::api;\nfun letters() usize { root::amount() + \"banana\".countOf('a') }";
    write_text(&source, original);
    let workload = Workload {
        root: workspace.root.clone(),
        source: source.clone(),
        original: vec![(source.clone(), original.into())],
        edit: (
            source,
            replace_once(original, "root::api;", "root::api::Point;"),
        ),
        probes: vec![("root::amount()", "amount"), (".countOf(", "countOf")],
        _workspace: Some(workspace),
    };
    audit("workspace", "facade import", workload);
}

#[test]
fn array_list_comment() {
    audit_array_list("comment");
}

#[test]
fn array_list_body() {
    audit_array_list("body");
}

#[test]
fn array_list_signature() {
    audit_array_list("signature");
}

#[test]
fn dependency_signature() {
    audit_dependencies("dependency_signature");
}

#[test]
fn provider_demand() {
    audit_dependencies("provider_demand");
}

#[test]
fn provider_definition() {
    audit_dependencies("provider_definition");
}

#[test]
fn provider_position() {
    audit_dependencies("provider_position");
}

#[test]
fn unresolved_name() {
    audit_dependencies("unresolved_name");
}
