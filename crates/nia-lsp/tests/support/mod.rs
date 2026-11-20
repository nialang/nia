// SPDX-License-Identifier: GPL-3.0-or-later
//! A language-server client for the stdio tests, and the fixtures they share.
//!
//! The server under test is the `NIA_LSP_SMOKE_SERVER` executable when it is
//! set, normally the `bin/nia-lsp` of an unpacked release archive, which finds
//! its own resources. Otherwise it is this package's `nia-lsp`, started with
//! the checkout's `lib`. Commands that build run the `nia` beside the server.

use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, Command},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

use nia_test_support::{InteractiveChild, TestDir};
use serde_json::{Value, json};

const SERVER_ENV: &str = "NIA_LSP_SMOKE_SERVER";

/// The packaged server named by `NIA_LSP_SMOKE_SERVER`, if any.
pub fn packaged_server() -> Option<PathBuf> {
    std::env::var_os(SERVER_ENV).map(PathBuf::from)
}

/// The server under test, ready to be started in a workspace.
pub fn server_command() -> Command {
    match packaged_server() {
        Some(server) => Command::new(server),
        None => {
            let mut command = Command::new(env!("CARGO_BIN_EXE_nia-lsp"));
            command.arg("--resource-root").arg(resource_root());
            command
        }
    }
}

/// The toolchain resources the server under test reads: the `lib` beside a
/// packaged server's `bin`, or this checkout's `lib`.
pub fn resource_root() -> PathBuf {
    match packaged_server() {
        Some(server) => physical_path(
            &server
                .parent()
                .and_then(Path::parent)
                .expect("a packaged server lives in `bin`")
                .join("lib"),
        ),
        None => nia_test_support::workspace_root().join("lib"),
    }
}

/// An owned scratch workspace and its physical path, which is the spelling
/// the server reports in its URIs.
pub struct Workspace {
    _dir: TestDir,
    pub root: PathBuf,
}

impl Workspace {
    pub fn new(name: &str) -> Self {
        let dir = nia_test_support::test_dir(name);
        let root = physical_path(&dir);
        Self { _dir: dir, root }
    }
}

/// The canonical spelling of an existing `path`: macOS reports temporary
/// directories under `/private`, and Windows resolves 8.3 short names. The
/// Windows verbatim prefix is dropped so the path stays usable everywhere.
pub fn physical_path(path: &Path) -> PathBuf {
    let canonical = path
        .canonicalize()
        .unwrap_or_else(|error| panic!("canonicalize {}: {error}", path.display()));
    match canonical
        .to_str()
        .and_then(|text| text.strip_prefix(r"\\?\"))
    {
        Some(stripped) if cfg!(windows) => PathBuf::from(stripped),
        _ => canonical,
    }
}

/// The `file:` URI of an absolute path.
pub fn uri(path: &Path) -> String {
    url::Url::from_file_path(path)
        .unwrap_or_else(|()| panic!("{} has no file URI", path.display()))
        .into()
}

/// Writes `text` with the platform's line endings, as an editor or a text
/// tool would on that platform; Windows fixtures therefore hold CRLF files.
pub fn write_text(path: &Path, text: &str) {
    let text = if cfg!(windows) {
        text.replace('\n', "\r\n")
    } else {
        text.to_owned()
    };
    fs::write(path, text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

/// Reads a text file with its line endings normalized to `\n`.
pub fn read_text(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .replace("\r\n", "\n")
}

pub fn create_dir_all(path: &Path) {
    fs::create_dir_all(path).unwrap_or_else(|error| panic!("create {}: {error}", path.display()));
}

/// The LSP position of the first occurrence of `needle` in `text`.
pub fn position(text: &str, needle: &str) -> Value {
    let offset = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} does not occur in the text"));
    position_at(text, offset)
}

/// The LSP position of byte `offset` in `text`, in UTF-16 code units.
pub fn position_at(text: &str, offset: usize) -> Value {
    let head = &text[..offset];
    let line_start = head.rfind('\n').map_or(0, |index| index + 1);
    point(
        head.matches('\n').count(),
        head[line_start..].encode_utf16().count(),
    )
}

pub fn point(line: usize, character: usize) -> Value {
    json!({ "line": line, "character": character })
}

/// `point` moved `delta` UTF-16 code units along its line.
pub fn shifted(point: &Value, delta: i64) -> Value {
    let mut moved = point.clone();
    moved["character"] = json!(integer(&point["character"]) + delta);
    moved
}

pub fn integer(value: &Value) -> i64 {
    value
        .as_i64()
        .unwrap_or_else(|| panic!("expected an integer, got {value}"))
}

pub fn string(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("expected a string, got {value}"))
}

pub fn array(value: &Value) -> &[Value] {
    value
        .as_array()
        .unwrap_or_else(|| panic!("expected an array, got {value}"))
}

/// Whether `value` is empty in the protocol's sense: `null`, `false`, zero,
/// or an empty string, array, or object. Optional results use any of them.
pub fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(flag) => !flag,
        Value::Number(number) => number.as_f64() == Some(0.0),
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(members) => members.is_empty(),
    }
}

/// One message the reader thread received, or why reading stopped.
type Received = Result<(Instant, Value), String>;

/// Published diagnostics: when they arrived and what they hold.
pub type Published = (Instant, Vec<Value>);

/// A language-server client over the server's standard streams.
///
/// Responses, diagnostics, and requests from the server are kept as they
/// arrive, so a test can wait for the one it needs while others are pending.
/// Requests from the server are answered with a `null` result.
pub struct Client {
    child: InteractiveChild,
    stdin: Option<ChildStdin>,
    messages: Receiver<Received>,
    timeout: Duration,
    sequence: i64,
    responses: HashMap<i64, Value>,
    diagnostics: HashMap<(String, Option<i64>), Published>,
    /// The methods of the requests the server sent, and when the last of
    /// each arrived.
    pub server_requests: HashMap<String, Instant>,
}

impl Client {
    /// Starts the server under test in `root`; every wait gives up after
    /// `timeout`.
    pub fn start(root: &Path, timeout: Duration) -> Self {
        Self::spawn(server_command().current_dir(root), timeout)
    }

    fn spawn(command: &mut Command, timeout: Duration) -> Self {
        let mut child = InteractiveChild::spawn_in_session(command, "nia-lsp");
        let stdin = child.take_stdin();
        let stdout = child.take_stdout().expect("stdout is piped");
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let message = read_message(&mut reader).map(|value| (Instant::now(), value));
                let stop = message.is_err();
                if sender.send(message).is_err() || stop {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            messages,
            timeout,
            sequence: 0,
            responses: HashMap::new(),
            diagnostics: HashMap::new(),
            server_requests: HashMap::new(),
        }
    }

    fn send(&mut self, mut message: Value) {
        message["jsonrpc"] = json!("2.0");
        let body = message.to_string();
        let stdin = self.stdin.as_mut().expect("the server's input is open");
        let written = stdin
            .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
            .and_then(|()| stdin.write_all(body.as_bytes()))
            .and_then(|()| stdin.flush());
        if let Err(error) = written {
            self.fail(&format!("write to the server: {error}"));
        }
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "method": method, "params": params }));
    }

    /// Sends a request and returns its id.
    pub fn request(&mut self, method: &str, params: Value) -> i64 {
        self.sequence += 1;
        let id = self.sequence;
        self.send(json!({ "id": id, "method": method, "params": params }));
        id
    }

    /// The whole response to request `id`.
    pub fn wait(&mut self, id: i64) -> Value {
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Some(response) = self.responses.remove(&id) {
                return response;
            }
            self.receive(deadline, &format!("the response to request {id}"));
        }
    }

    /// The result of a request that must succeed.
    pub fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params);
        let mut response = self.wait(id);
        if let Some(error) = response.get("error") {
            self.fail(&format!("{method}: {error}"));
        }
        response["result"].take()
    }

    /// Reads one message, recording it, or fails at `deadline`.
    pub fn receive(&mut self, deadline: Instant, waiting_for: &str) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (received, message) = match self.messages.recv_timeout(remaining) {
            Ok(Ok(message)) => message,
            Ok(Err(error)) => self.fail(&format!("waiting for {waiting_for}: {error}")),
            Err(RecvTimeoutError::Timeout) => self.fail(&format!(
                "timed out after {:?} waiting for {waiting_for}",
                self.timeout,
            )),
            Err(RecvTimeoutError::Disconnected) => {
                self.fail(&format!("waiting for {waiting_for}: the reader stopped"))
            }
        };
        match message.get("method").and_then(Value::as_str) {
            Some("textDocument/publishDiagnostics") => {
                let params = &message["params"];
                let key = (
                    string(&params["uri"]).to_owned(),
                    params["version"].as_i64(),
                );
                let items = array(&params["diagnostics"]).to_vec();
                self.diagnostics.insert(key, (received, items));
            }
            Some(method) => {
                if let Some(id) = message.get("id").cloned() {
                    self.server_requests.insert(method.to_owned(), received);
                    self.send(json!({ "id": id, "result": null }));
                }
            }
            None => {
                if let Some(id) = message.get("id").and_then(Value::as_i64) {
                    self.responses.insert(id, message);
                }
            }
        }
    }

    /// The diagnostics published for `uri` at `version` that `accept`
    /// accepts, and when they arrived. Publications that arrived before
    /// `since` or that `accept` rejects are dropped while waiting for a later
    /// one.
    pub fn wait_diagnostics(
        &mut self,
        uri: &str,
        version: Option<i64>,
        since: Option<Instant>,
        accept: impl Fn(&[Value]) -> bool,
    ) -> Published {
        let key = (uri.to_owned(), version);
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Some((received, items)) = self.diagnostics.remove(&key)
                && since.is_none_or(|since| received >= since)
                && accept(&items)
            {
                return (received, items);
            }
            self.receive(
                deadline,
                &format!("diagnostics of {uri} at version {version:?}"),
            );
        }
    }

    /// Shuts the server down and checks that it exits successfully.
    pub fn shutdown(mut self, timeout: Duration) {
        self.call("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        self.stdin = None;
        let status = self.child.wait_timeout(timeout);
        if !status.success() {
            self.fail(&format!("the server exited with {status}"));
        }
    }

    fn fail(&self, message: &str) -> ! {
        panic!(
            "nia-lsp: {message}\nstderr tail:\n{}",
            self.child.stderr_tail()
        );
    }
}

/// Reads one `Content-Length` framed JSON-RPC message.
fn read_message(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut length = None;
    loop {
        let mut header = String::new();
        let read = reader
            .read_line(&mut header)
            .map_err(|error| format!("read a header: {error}"))?;
        if read == 0 {
            return Err("the server closed its output".to_owned());
        }
        let header = header.trim();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| format!("bad Content-Length {value:?}: {error}"))?,
            );
        }
    }
    let length = length.ok_or("a message has no Content-Length")?;
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("read a message body: {error}"))?;
    serde_json::from_slice(&body).map_err(|error| format!("parse a message: {error}"))
}
