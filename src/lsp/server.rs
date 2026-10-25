//! The language server: JSON-RPC framing, document store and request handlers.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};

use super::analysis::complete::{call_context, closing_delimiters, completion_context, CompletionContext};
use super::analysis::{Analysis, TOKEN_MODIFIERS, TOKEN_TYPES};
use super::json::Json;
use super::uri::{path_to_uri, uri_to_path};
use crate::vfs::{path, Vfs};

// JSON-RPC error codes
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const SERVER_NOT_INITIALIZED: i64 = -32002;
const REQUEST_FAILED: i64 = -32803;
const INTERNAL_ERROR: i64 = -32603;

/// Reads one `Content-Length`-framed message; `Ok(None)` at end of input.
pub fn read_message<R: BufRead>(input: &mut R) -> io::Result<Option<String>> {
    let mut length: Option<usize> = None;
    let mut saw_header = false;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            if saw_header {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"));
            }
            continue; // stray blank line before any header
        }
        saw_header = true;
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok();
            }
        }
    }
    let n = length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    if n > 256 << 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut body = vec![0u8; n];
    input.read_exact(&mut body)?;
    String::from_utf8(body).map(Some).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "message is not UTF-8"))
}

pub fn write_message<W: Write>(out: &mut W, body: &Json) -> io::Result<()> {
    let text = body.to_string();
    write!(out, "Content-Length: {}\r\n\r\n{text}", text.len())?;
    out.flush()
}

/// Open documents laid over a base file system, so imports see unsaved edits.
#[derive(Debug)]
struct Overlay<'a> {
    base: &'a dyn Vfs,
    files: HashMap<String, String>,
}

impl Vfs for Overlay<'_> {
    fn read_to_string(&self, p: &str) -> io::Result<String> {
        match self.files.get(&path::normalize(p)) {
            Some(t) => Ok(t.clone()),
            None => self.base.read_to_string(p),
        }
    }

    fn exists(&self, p: &str) -> bool {
        self.files.contains_key(&path::normalize(p)) || self.base.exists(p)
    }

    fn is_file(&self, p: &str) -> bool {
        self.files.contains_key(&path::normalize(p)) || self.base.is_file(p)
    }

    fn canonicalize(&self, p: &str) -> io::Result<String> {
        if self.files.contains_key(&path::normalize(p)) {
            return Ok(path::normalize(p));
        }
        self.base.canonicalize(p).or_else(|_| Ok(path::normalize(p)))
    }

    fn resolve(&self, importer: &str, spec: &str) -> String {
        self.base.resolve(importer, spec)
    }
}

struct Doc {
    text: String,
    version: i64,
    analysis: Option<Analysis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Starting,
    Running,
    ShuttingDown,
}

pub struct Server<'v, W: Write> {
    base: &'v dyn Vfs,
    out: W,
    docs: HashMap<String, Doc>,
    state: State,
    /// Set by the `exit` notification.
    pub exit_code: Option<i32>,
}

impl<W: Write> std::fmt::Debug for Server<'_, W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").field("state", &self.state).field("documents", &self.docs.len()).finish()
    }
}

type Reply = Result<Json, (i64, String)>;

fn invalid(msg: &str) -> (i64, String) {
    (INVALID_PARAMS, msg.to_string())
}

impl<'v, W: Write> Server<'v, W> {
    pub fn new(base: &'v dyn Vfs, out: W) -> Server<'v, W> {
        Server { base, out, docs: HashMap::new(), state: State::Starting, exit_code: None }
    }

    pub fn into_output(self) -> W {
        self.out
    }

    /// The writer the server answers on (tests read it between steps).
    pub fn output_mut(&mut self) -> &mut W {
        &mut self.out
    }

    fn send(&mut self, body: Json) {
        // a closed pipe means the client is gone; there is nobody left to tell
        let _ = write_message(&mut self.out, &body);
    }

    fn respond(&mut self, id: Json, result: Reply) {
        let body = match result {
            Ok(r) => Json::object(vec![("jsonrpc", "2.0".into()), ("id", id), ("result", r)]),
            Err((code, message)) => Json::object(vec![
                ("jsonrpc", "2.0".into()),
                ("id", id),
                ("error", Json::object(vec![("code", Json::from(code)), ("message", message.into())])),
            ]),
        };
        self.send(body);
    }

    fn notify(&mut self, method: &str, params: Json) {
        self.send(Json::object(vec![("jsonrpc", "2.0".into()), ("method", method.into()), ("params", params)]));
    }

    /// Processes one raw message.
    pub fn handle_text(&mut self, text: &str) {
        match Json::parse(text) {
            Ok(msg) => self.handle(msg),
            Err(e) => self.respond(Json::Null, Err((PARSE_ERROR, e.to_string()))),
        }
    }

    pub fn handle(&mut self, msg: Json) {
        let Some(method) = msg.get("method").and_then(Json::as_str).map(str::to_string) else {
            // a response to a request we never send, or garbage
            if msg.get("id").is_some() && msg.get("result").is_none() && msg.get("error").is_none() {
                self.respond(
                    msg.get("id").cloned().unwrap_or(Json::Null),
                    Err((INVALID_REQUEST, "missing method".into())),
                );
            }
            return;
        };
        let params = msg.get("params").cloned().unwrap_or(Json::Null);
        match msg.get("id").cloned() {
            Some(id) => {
                // a bug in one query must not take the editor session down with it
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.request(&method, &params)))
                    .unwrap_or_else(|_| Err((INTERNAL_ERROR, format!("internal error while handling `{method}`"))));
                self.respond(id, result);
            }
            None => {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.notification(&method, &params)));
            }
        }
    }

    // ---- notifications ----------------------------------------------------------------------------

    fn notification(&mut self, method: &str, params: &Json) {
        match method {
            "exit" => self.exit_code = Some(if self.state == State::ShuttingDown { 0 } else { 1 }),
            _ if self.state == State::Starting => {}
            "textDocument/didOpen" => {
                let Some(td) = params.get("textDocument") else { return };
                let (Some(uri), Some(text)) =
                    (td.get("uri").and_then(Json::as_str), td.get("text").and_then(Json::as_str))
                else {
                    return;
                };
                let version = td.get("version").and_then(Json::as_i64).unwrap_or(0);
                self.docs.insert(uri.to_string(), Doc { text: text.to_string(), version, analysis: None });
                self.refresh(uri);
                self.refresh_importers_of(uri);
            }
            "textDocument/didChange" => {
                let Some(uri) = params.path(&["textDocument", "uri"]).and_then(Json::as_str) else { return };
                let version = params.path(&["textDocument", "version"]).and_then(Json::as_i64).unwrap_or(0);
                let Some(changes) = params.get("contentChanges").and_then(Json::as_array) else { return };
                let Some(doc) = self.docs.get_mut(uri) else { return };
                for c in changes {
                    match (c.get("range"), c.get("text").and_then(Json::as_str)) {
                        // incremental change
                        (Some(range), Some(new)) if !range.is_null() => apply_change(&mut doc.text, range, new),
                        // whole-document replacement
                        (_, Some(new)) => doc.text = new.to_string(),
                        _ => {}
                    }
                }
                doc.version = version;
                self.refresh(uri);
                self.refresh_importers_of(uri);
            }
            "textDocument/didSave" => {
                if let Some(uri) = params.path(&["textDocument", "uri"]).and_then(Json::as_str) {
                    if let Some(text) = params.get("text").and_then(Json::as_str) {
                        if let Some(doc) = self.docs.get_mut(uri) {
                            doc.text = text.to_string();
                        }
                    }
                    self.refresh(uri);
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = params.path(&["textDocument", "uri"]).and_then(Json::as_str) {
                    self.docs.remove(uri);
                    self.notify(
                        "textDocument/publishDiagnostics",
                        Json::object(vec![("uri", Json::from(uri)), ("diagnostics", Json::Arr(vec![]))]),
                    );
                    // documents that import this one now read it from disk again
                    self.refresh_importers_of(uri);
                }
            }
            _ => {} // initialized, $/cancelRequest, configuration changes, ...
        }
    }

    fn overlay(&self) -> Overlay<'v> {
        let files = self.docs.iter().map(|(uri, d)| (path::normalize(&uri_to_path(uri)), d.text.clone())).collect();
        Overlay { base: self.base, files }
    }

    fn analyze(&self, uri: &str, text: &str) -> Analysis {
        let overlay = self.overlay();
        Analysis::run(&uri_to_path(uri), text, &overlay)
    }

    /// Re-analyzes the other open documents whose import graph contains `uri`, so they see its
    /// latest text.
    fn refresh_importers_of(&mut self, uri: &str) {
        let overlay = self.overlay();
        let target = overlay.canonicalize(&uri_to_path(uri)).unwrap_or_else(|_| path::normalize(&uri_to_path(uri)));
        let importers: Vec<String> = self
            .docs
            .iter()
            .filter(|(u, _)| u.as_str() != uri)
            .filter(|(_, d)| d.analysis.as_ref().is_some_and(|a| a.prog.modules.iter().any(|m| m.path == target)))
            .map(|(u, _)| u.clone())
            .collect();
        for u in importers {
            self.refresh(&u);
        }
    }

    /// Re-analyzes `uri` and publishes its diagnostics.
    fn refresh(&mut self, uri: &str) {
        let Some(text) = self.docs.get(uri).map(|d| d.text.clone()) else { return };
        let analysis = self.analyze(uri, &text);
        let diagnostics = analysis.diagnostics();
        let version = self.docs.get(uri).map(|d| d.version);
        if let Some(doc) = self.docs.get_mut(uri) {
            doc.analysis = Some(analysis);
        }
        let mut params = vec![("uri", Json::from(uri)), ("diagnostics", Json::Arr(diagnostics))];
        if let Some(v) = version {
            params.push(("version", Json::from(v)));
        }
        self.notify("textDocument/publishDiagnostics", Json::object(params));
    }

    /// The analysis of `uri`, computed if the document was changed without one.
    fn analysis_of(&mut self, uri: &str) -> Result<&mut Analysis, (i64, String)> {
        let needs = match self.docs.get(uri) {
            None => return Err((REQUEST_FAILED, format!("document `{uri}` is not open"))),
            Some(d) => d.analysis.is_none(),
        };
        if needs {
            let text = self.docs[uri].text.clone();
            let a = self.analyze(uri, &text);
            self.docs.get_mut(uri).expect("open").analysis = Some(a);
        }
        Ok(self.docs.get_mut(uri).expect("open").analysis.as_mut().expect("analyzed"))
    }

    // ---- requests -------------------------------------------------------------------------------------

    fn request(&mut self, method: &str, params: &Json) -> Reply {
        if method == "initialize" {
            self.state = State::Running;
            return Ok(initialize_result());
        }
        if self.state == State::Starting {
            return Err((SERVER_NOT_INITIALIZED, "the server has not been initialized".into()));
        }
        if method == "shutdown" {
            self.state = State::ShuttingDown;
            return Ok(Json::Null);
        }
        if self.state == State::ShuttingDown {
            return Err((INVALID_REQUEST, "the server is shutting down".into()));
        }
        let uri = params.path(&["textDocument", "uri"]).and_then(Json::as_str).unwrap_or("").to_string();
        let position = || -> Result<(u32, u32), (i64, String)> {
            let p = params.get("position").ok_or_else(|| invalid("missing position"))?;
            let line = p.get("line").and_then(Json::as_u32).ok_or_else(|| invalid("bad position"))?;
            let ch = p.get("character").and_then(Json::as_u32).ok_or_else(|| invalid("bad position"))?;
            Ok((line, ch))
        };
        match method {
            "textDocument/hover" => {
                let (line, ch) = position()?;
                let a = self.analysis_of(&uri)?;
                let offset = a.offset_at(line, ch);
                Ok(match a.hover(offset) {
                    Some((markdown, span)) => Json::object(vec![
                        ("contents", Json::object(vec![("kind", "markdown".into()), ("value", markdown.into())])),
                        ("range", a.range(span)),
                    ]),
                    None => Json::Null,
                })
            }
            "textDocument/definition" => {
                let (line, ch) = position()?;
                let a = self.analysis_of(&uri)?;
                let offset = a.offset_at(line, ch);
                let locs: Vec<Json> = a.definition(offset).iter().map(Analysis::location_json).collect();
                Ok(if locs.is_empty() { Json::Null } else { Json::Arr(locs) })
            }
            "textDocument/references" => {
                let (line, ch) = position()?;
                let include = params.path(&["context", "includeDeclaration"]).and_then(Json::as_bool).unwrap_or(true);
                let a = self.analysis_of(&uri)?;
                let offset = a.offset_at(line, ch);
                Ok(Json::Arr(a.references(offset, include).iter().map(Analysis::location_json).collect()))
            }
            "textDocument/documentHighlight" => {
                let (line, ch) = position()?;
                let a = self.analysis_of(&uri)?;
                let offset = a.offset_at(line, ch);
                let hs: Vec<Json> = a
                    .highlights(offset)
                    .into_iter()
                    .map(|s| Json::object(vec![("range", a.range(s)), ("kind", Json::from(1u32))]))
                    .collect();
                Ok(Json::Arr(hs))
            }
            "textDocument/prepareRename" => {
                let (line, ch) = position()?;
                let a = self.analysis_of(&uri)?;
                let offset = a.offset_at(line, ch);
                match a.prepare_rename(offset) {
                    Ok((span, name)) => Ok(Json::object(vec![("range", a.range(span)), ("placeholder", name.into())])),
                    Err(e) => Err((REQUEST_FAILED, e)),
                }
            }
            "textDocument/rename" => {
                let (line, ch) = position()?;
                let new_name =
                    params.get("newName").and_then(Json::as_str).ok_or_else(|| invalid("missing newName"))?;
                let a = self.analysis_of(&uri)?;
                let offset = a.offset_at(line, ch);
                let spans = a.rename(offset, new_name).map_err(|e| (REQUEST_FAILED, e))?;
                let mut by_uri: Vec<(String, Vec<Json>)> = Vec::new();
                for s in spans {
                    let Some(loc) = a.location(s) else { continue };
                    let edit = Json::object(vec![("range", loc.range), ("newText", new_name.into())]);
                    match by_uri.iter_mut().find(|(u, _)| *u == loc.uri) {
                        Some((_, edits)) => edits.push(edit),
                        None => by_uri.push((loc.uri, vec![edit])),
                    }
                }
                let changes = Json::Obj(by_uri.into_iter().map(|(u, e)| (u, Json::Arr(e))).collect());
                Ok(Json::object(vec![("changes", changes)]))
            }
            "textDocument/documentSymbol" => Ok(self.analysis_of(&uri)?.document_symbols()),
            "textDocument/semanticTokens/full" => {
                let data = self.analysis_of(&uri)?.semantic_tokens();
                Ok(Json::object(vec![("data", Json::Arr(data.into_iter().map(Json::from).collect()))]))
            }
            "textDocument/completion" => {
                let (line, ch) = position()?;
                self.completion(&uri, line, ch)
            }
            "textDocument/signatureHelp" => {
                let (line, ch) = position()?;
                self.signature_help(&uri, line, ch)
            }
            other => Err((METHOD_NOT_FOUND, format!("method `{other}` is not supported"))),
        }
    }

    fn completion(&mut self, uri: &str, line: u32, ch: u32) -> Reply {
        let text = self.docs.get(uri).ok_or_else(|| (REQUEST_FAILED, "document is not open".to_string()))?.text.clone();
        let offset = super::position::LineIndex::new(&text).offset(line, ch);
        let items: Vec<Json> = match completion_context(&text, offset) {
            CompletionContext::Nothing => Vec::new(),
            CompletionContext::Members { dot } => {
                // `x.pre` is not valid code: analyze `x` alone to learn its type
                let patched = format!("{}{}{}", &text[..dot], closing_delimiters(&text, dot), &text[offset..]);
                let mut a = self.analyze(uri, &patched);
                a.complete_members(dot as u32).iter().map(|i| i.to_json('0')).collect()
            }
            CompletionContext::Path { qualifier } => {
                let a = self.analysis_of(uri)?;
                a.complete_path(&qualifier).iter().map(|i| i.to_json('0')).collect()
            }
            CompletionContext::Scope => {
                let a = self.analysis_of(uri)?;
                let offset = a.offset_at(line, ch);
                a.complete_scope(offset).iter().map(|(g, i)| i.to_json(*g)).collect()
            }
        };
        Ok(Json::object(vec![("isIncomplete", false.into()), ("items", Json::Arr(items))]))
    }

    fn signature_help(&mut self, uri: &str, line: u32, ch: u32) -> Reply {
        let text = self.docs.get(uri).ok_or_else(|| (REQUEST_FAILED, "document is not open".to_string()))?.text.clone();
        let offset = super::position::LineIndex::new(&text).offset(line, ch);
        let Some(ctx) = call_context(&text, offset) else { return Ok(Json::Null) };
        // close the call so the half-typed argument list parses
        let patched = format!("{}{}{}", &text[..offset], closing_delimiters(&text, offset), &text[offset..]);
        let mut a = self.analyze(uri, &patched);
        Ok(a.signature_help(&ctx).unwrap_or(Json::Null))
    }
}

/// Applies an incremental change (`range` + `text`) to a document.
fn apply_change(doc: &mut String, range: &Json, new: &str) {
    let pos = |p: &Json| -> Option<(u32, u32)> { Some((p.get("line")?.as_u32()?, p.get("character")?.as_u32()?)) };
    let (Some(start), Some(end)) = (range.get("start").and_then(pos), range.get("end").and_then(pos)) else { return };
    let (lo, hi) = {
        let index = super::position::LineIndex::new(doc);
        (index.offset(start.0, start.1), index.offset(end.0, end.1))
    };
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    doc.replace_range(lo..hi, new);
}

fn initialize_result() -> Json {
    let legend = Json::object(vec![
        ("tokenTypes", TOKEN_TYPES.iter().map(|s| Json::from(*s)).collect::<Vec<_>>().into()),
        ("tokenModifiers", TOKEN_MODIFIERS.iter().map(|s| Json::from(*s)).collect::<Vec<_>>().into()),
    ]);
    Json::object(vec![
        (
            "capabilities",
            Json::object(vec![
                ("positionEncoding", "utf-16".into()),
                (
                    "textDocumentSync",
                    Json::object(vec![
                        ("openClose", true.into()),
                        ("change", Json::from(2u32)),
                        ("save", Json::object(vec![("includeText", false.into())])),
                    ]),
                ),
                ("hoverProvider", true.into()),
                ("definitionProvider", true.into()),
                ("referencesProvider", true.into()),
                ("documentHighlightProvider", true.into()),
                ("renameProvider", Json::object(vec![("prepareProvider", true.into())])),
                ("documentSymbolProvider", true.into()),
                (
                    "completionProvider",
                    Json::object(vec![("triggerCharacters", vec![".", ":"].into()), ("resolveProvider", false.into())]),
                ),
                ("signatureHelpProvider", Json::object(vec![("triggerCharacters", vec!["(", ","].into())])),
                (
                    "semanticTokensProvider",
                    Json::object(vec![("legend", legend), ("full", true.into()), ("range", false.into())]),
                ),
            ]),
        ),
        ("serverInfo", Json::object(vec![("name", "neon-lsp".into()), ("version", env!("CARGO_PKG_VERSION").into())])),
    ])
}

/// Runs the server until `exit` or end of input; returns the process exit code.
pub fn run<R: BufRead, W: Write>(mut input: R, output: W, vfs: &dyn Vfs) -> i32 {
    let mut server = Server::new(vfs, output);
    loop {
        match read_message(&mut input) {
            Ok(Some(text)) => {
                server.handle_text(&text);
                if let Some(code) = server.exit_code {
                    return code;
                }
            }
            Ok(None) => return if server.state == State::ShuttingDown { 0 } else { 1 },
            Err(_) => return 1,
        }
    }
}

/// A publish-diagnostics notification with the given diagnostics (used by tests and tooling).
pub fn uri_of(path: &str) -> String {
    path_to_uri(path)
}

#[allow(dead_code)]
fn _unused(_: HashSet<u8>) {}
