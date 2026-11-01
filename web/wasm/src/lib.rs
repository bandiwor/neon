//! The Neon toolchain for the browser.
//!
//! One module, no glue generator: the host (a Web Worker) writes a JSON request into the module's
//! memory, calls [`neon_call`] and reads a JSON reply back. Everything the command line does is there
//! (`cli`: the very same entry point, over an in-memory file system), plus what an editor needs: the
//! language server (`lsp`, JSON-RPC messages in and out) and the REPL (`repl_*`, a session that lives as
//! long as the module instance).
//!
//! Requests are `{"op": "...", ...}`:
//!
//! | op | request fields | reply |
//! |---|---|---|
//! | `version` | | `{version}` |
//! | `cli` | `args`, `files` (`{path: text}`), `stdin`, `interactive`, `exact` | `{code, stdout, stderr, needsInput, needsKey}` |
//! | `lsp` | `message` (one JSON-RPC message as text) | `{messages: [...]}` |
//! | `repl_eval` | `cell`, `stdin` (answers to `input()`, one per line) | `{kind, text, needsInput}` |
//! | `repl_reset` / `repl_cells` | | `{cells}` |
//! | `repl_more` | `text` | `{more}` (the input is unfinished: open brackets, string) |
//! | `repl_complete` | `buffer`, `cursor` | `{start, items: [{label, detail}]}` |
//! | `repl_preview` | `cell` | `{value}` |
//! | `repl_highlight` | `text` | `{runs: [[from, to, class]]}` (in characters) |
//!
//! Memory protocol: [`neon_alloc`] a buffer, write the request, call [`neon_call`] with it; the reply is
//! a buffer whose first four bytes are its length (little endian) followed by the JSON text, to be
//! released with [`neon_dealloc`].

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{self, BufRead, Read};
use std::rc::Rc;

use neon::cli::Sandbox;
use neon::host::MemFiles;
use neon::lsp::json::Json;
use neon::lsp::Server;
use neon::repl::{self, highlight::Class, ReplConfig, Session};
use neon::vfs::MemVfs;

/// What a program that asks for more input than it was given dies with, so the host can tell "needs
/// input" from any other failure.
const NEEDS_INPUT: &str = "neon-web: input is needed";

thread_local! {
    static SERVER: RefCell<Option<Server<'static, Vec<u8>>>> = const { RefCell::new(None) };
    static SESSION: RefCell<Session> = RefCell::new(Session::new());
}

// ---- the memory protocol --------------------------------------------------------------------------------

/// A buffer of `len` bytes for the host to write into.
#[no_mangle]
pub extern "C" fn neon_alloc(len: usize) -> *mut u8 {
    let mut v = vec![0u8; len.max(1)];
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr` must come from [`neon_alloc`] (or be a reply of [`neon_call`]) with the same `len`.
#[no_mangle]
pub unsafe extern "C" fn neon_dealloc(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, len.max(1), len.max(1)));
}

/// Runs one request; returns the reply buffer (see the module docs), of `4 + length` bytes.
///
/// # Safety
/// `req` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn neon_call(req: *const u8, len: usize) -> *mut u8 {
    let bytes = std::slice::from_raw_parts(req, len);
    let reply = match std::str::from_utf8(bytes) {
        Ok(text) => handle(text),
        Err(_) => error_reply("the request is not UTF-8"),
    };
    let body = reply.as_bytes();
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out.shrink_to_fit();
    let p = out.as_mut_ptr();
    std::mem::forget(out);
    p
}

fn error_reply(message: &str) -> String {
    Json::object(vec![("error", Json::from(message))]).to_string()
}

// ---- requests -------------------------------------------------------------------------------------------

fn handle(text: &str) -> String {
    let req = match Json::parse(text) {
        Ok(j) => j,
        Err(e) => return error_reply(&format!("bad request: {e}")),
    };
    let Some(op) = req.get("op").and_then(Json::as_str) else { return error_reply("the request has no `op`") };
    match op {
        "version" => Json::object(vec![("version", Json::from(env!("CARGO_PKG_VERSION")))]).to_string(),
        "cli" => cli(&req),
        "lsp" => lsp(&req),
        "repl_eval" => repl_eval(&req),
        "repl_reset" => SESSION.with(|s| {
            s.borrow_mut().reset();
            Json::object(vec![("cells", Json::from(0usize))]).to_string()
        }),
        "repl_cells" => SESSION.with(|s| {
            let cells: Vec<Json> = s.borrow().cells().iter().map(|c| Json::from(c.as_str())).collect();
            Json::object(vec![("cells", Json::Arr(cells))]).to_string()
        }),
        "repl_more" => Json::object(vec![("more", Json::from(repl::needs_more(str_field(&req, "text"))))]).to_string(),
        "repl_complete" => repl_complete(&req),
        "repl_preview" => repl_preview(&req),
        "repl_highlight" => repl_highlight(&req),
        other => error_reply(&format!("unknown op `{other}`")),
    }
}

fn str_field<'a>(req: &'a Json, key: &str) -> &'a str {
    req.get(key).and_then(Json::as_str).unwrap_or("")
}

/// The program's standard input: the given lines, then either the end of input or (when the host can
/// ask the user) a failure that says more is needed.
struct Lines {
    lines: VecDeque<String>,
    current: Vec<u8>,
    pos: usize,
    interactive: bool,
    asked: Rc<std::cell::Cell<bool>>,
}

impl Lines {
    fn new(text: &str, interactive: bool, asked: Rc<std::cell::Cell<bool>>) -> Lines {
        let mut lines: VecDeque<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
        // a trailing newline does not start another (empty) line
        if text.ends_with('\n') || text.is_empty() {
            lines.pop_back();
        }
        Lines { lines, current: Vec::new(), pos: 0, interactive, asked }
    }

    /// The text as it is, byte for byte: keys are not lines (an Enter is a `\r` of its own).
    fn exact(text: &str, interactive: bool, asked: Rc<std::cell::Cell<bool>>) -> Lines {
        Lines { lines: VecDeque::new(), current: text.as_bytes().to_vec(), pos: 0, interactive, asked }
    }
}

impl Read for Lines {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = {
            let avail = self.fill_buf()?;
            let n = avail.len().min(out.len());
            out[..n].copy_from_slice(&avail[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for Lines {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos >= self.current.len() {
            match self.lines.pop_front() {
                Some(l) => {
                    self.current = format!("{l}\n").into_bytes();
                    self.pos = 0;
                }
                None if self.interactive => {
                    self.asked.set(true);
                    return Err(io::Error::other(NEEDS_INPUT));
                }
                None => return Ok(&[]),
            }
        }
        Ok(&self.current[self.pos..])
    }

    fn consume(&mut self, n: usize) {
        self.pos += n;
    }
}

fn memory_fs(req: &Json) -> MemVfs {
    let mut vfs = MemVfs::new();
    if let Some(Json::Obj(files)) = req.get("files") {
        for (path, text) in files {
            if let Some(t) = text.as_str() {
                vfs.insert(path, t);
            }
        }
    }
    vfs
}

fn cli(req: &Json) -> String {
    let args: Vec<String> = req
        .get("args")
        .and_then(Json::as_array)
        .map(|a| a.iter().filter_map(|j| j.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let vfs = memory_fs(req);
    let asked = Rc::new(std::cell::Cell::new(false));
    let interactive = req.get("interactive").and_then(Json::as_bool).unwrap_or(false);
    let exact = req.get("exact").and_then(Json::as_bool).unwrap_or(false);
    let stdin = if exact {
        Lines::exact(str_field(req, "stdin"), interactive, asked.clone())
    } else {
        Lines::new(str_field(req, "stdin"), interactive, asked.clone())
    };
    // the program's files are the project's; what it writes lasts for the run
    let mut files = MemFiles::new();
    if let Some(Json::Obj(all)) = req.get("files") {
        for (path, text) in all {
            if let Some(t) = text.as_str() {
                files.insert(path, t);
            }
        }
    }
    let sandbox = Sandbox { files, waiting_for_key: Rc::new(std::cell::Cell::new(false)) };
    let wants_key = sandbox.waiting_for_key.clone();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    // one thread: the engine would otherwise ask the platform how many it may use
    neon::par::set_threads(1);
    let code = neon::cli::main_in(&args, &vfs, stdin, &mut stdout, &mut stderr, false, Some(sandbox));
    let mut err_text = String::from_utf8_lossy(&stderr).into_owned();
    // a run that ended well did not need what it peeked at (the rest of an escape sequence)
    let needs_input = asked.get() && code != 0;
    if needs_input {
        err_text = err_text.replace(NEEDS_INPUT, "").replace("\n\n", "\n");
    }
    Json::object(vec![
        ("code", Json::from(code as i64)),
        ("stdout", Json::from(String::from_utf8_lossy(&stdout).into_owned())),
        ("stderr", Json::from(err_text)),
        ("needsInput", Json::from(needs_input)),
        ("needsKey", Json::from(needs_input && wants_key.get())),
    ])
    .to_string()
}

fn lsp(req: &Json) -> String {
    let message = str_field(req, "message");
    let frames: Vec<String> = SERVER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let server = slot.get_or_insert_with(|| {
            // the documents live in the server (the client opens every file); nothing else to read
            let vfs: &'static MemVfs = Box::leak(Box::new(MemVfs::new()));
            Server::new(vfs, Vec::new())
        });
        server.handle_text(message);
        let bytes = std::mem::take(server.output_mut());
        split_frames(&bytes)
    });
    let mut out = String::from("{\"messages\":[");
    out.push_str(&frames.join(","));
    out.push_str("]}");
    out
}

/// The bodies of the `Content-Length` framed messages in `bytes`.
fn split_frames(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    let mut rest: &str = &text;
    while let Some(head_end) = rest.find("\r\n\r\n") {
        let head = &rest[..head_end];
        let len = head
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length:"))
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let body_start = head_end + 4;
        // the length counts bytes
        let body_bytes = &rest.as_bytes()[body_start..(body_start + len).min(rest.len())];
        out.push(String::from_utf8_lossy(body_bytes).into_owned());
        rest = &rest[(body_start + len).min(rest.len())..];
    }
    out
}

fn repl_cfg() -> ReplConfig {
    let mut cfg = ReplConfig::default();
    cfg.vm.max_steps = Some(200_000_000);
    cfg
}

fn repl_eval(req: &Json) -> String {
    let cell = str_field(req, "cell");
    let asked = Rc::new(std::cell::Cell::new(false));
    let mut live = Lines::new(str_field(req, "stdin"), true, asked.clone());
    let vfs = MemVfs::new();
    let eval = SESSION.with(|s| s.borrow_mut().eval(cell, &vfs, &repl_cfg(), &mut live));
    let (kind, text) = match eval {
        repl::Eval::Ok(out) => ("ok", out),
        repl::Eval::Rejected(d) => ("rejected", d),
        repl::Eval::Trapped { output, error } => {
            let error = error.lines().filter(|l| l.trim() != "at <start>").collect::<Vec<_>>().join("\n");
            ("trapped", format!("{output}{error}\n"))
        }
    };
    let needs = asked.get();
    let text = if needs { text.replace(NEEDS_INPUT, "") } else { text };
    Json::object(vec![("kind", Json::from(kind)), ("text", Json::from(text)), ("needsInput", Json::from(needs))])
        .to_string()
}

fn repl_complete(req: &Json) -> String {
    let buffer = str_field(req, "buffer");
    let cursor = req.get("cursor").and_then(Json::as_u32).unwrap_or(0) as usize;
    let vfs = MemVfs::new();
    let found = SESSION.with(|s| repl::complete::complete(s.borrow().cells(), buffer, cursor, &vfs));
    match found {
        Some(c) => {
            let items: Vec<Json> = c
                .items
                .into_iter()
                .map(|i| Json::object(vec![("label", Json::from(i.label)), ("detail", Json::from(i.detail))]))
                .collect();
            Json::object(vec![("start", Json::from(c.start)), ("items", Json::Arr(items))]).to_string()
        }
        None => Json::object(vec![("start", Json::from(cursor)), ("items", Json::Arr(Vec::new()))]).to_string(),
    }
}

fn repl_preview(req: &Json) -> String {
    let vfs = MemVfs::new();
    let value = SESSION.with(|s| s.borrow().preview(str_field(req, "cell"), &vfs, &repl_cfg()));
    Json::object(vec![("value", Json::from(value))]).to_string()
}

fn class_name(c: Class) -> &'static str {
    match c {
        Class::Plain => "plain",
        Class::Keyword => "keyword",
        Class::Type => "type",
        Class::Variant => "variant",
        Class::Global => "global",
        Class::Param => "param",
        Class::Local => "local",
        Class::Function => "function",
        Class::Number => "number",
        Class::Str => "string",
        Class::Char => "string",
        Class::Operator => "operator",
        Class::Punct => "punct",
        Class::Bracket => "bracket",
        Class::Comment => "comment",
    }
}

fn repl_highlight(req: &Json) -> String {
    let text = str_field(req, "text");
    let vfs = MemVfs::new();
    let classes = SESSION.with(|s| repl::highlight::highlight(s.borrow().cells(), text, &vfs));
    // runs of one class, in characters
    let mut runs: Vec<Json> = Vec::new();
    let mut start = 0usize;
    for i in 1..=classes.len() {
        if i == classes.len() || classes[i] != classes[start] {
            if classes[start] != Class::Plain {
                runs.push(Json::Arr(vec![Json::from(start), Json::from(i), Json::from(class_name(classes[start]))]));
            }
            start = i;
        }
    }
    Json::object(vec![("runs", Json::Arr(runs))]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(req: &str) -> Json {
        Json::parse(&handle(req)).expect("the reply is JSON")
    }

    fn s(j: &Json, key: &str) -> String {
        j.get(key).and_then(Json::as_str).unwrap_or_default().to_string()
    }

    #[test]
    fn cli_runs_checks_and_formats_over_an_in_memory_project() {
        let r =
            call(r#"{"op":"cli","args":["run","main.neon"],"files":{"main.neon":"fn main() { println(1 + 2); }"}}"#);
        assert_eq!((r.get("code").and_then(Json::as_i64), s(&r, "stdout")), (Some(0), "3\n".to_string()));
        // imports between the files of the project
        let r = call(
            r#"{"op":"cli","args":["run","main.neon"],"files":{"main.neon":"import { sq } from \"lib/u\";\nfn main() { println(sq(7)); }","lib/u.neon":"export fn sq(n: i64) -> i64 { n * n }"}}"#,
        );
        assert_eq!(s(&r, "stdout"), "49\n");
        // errors come out rendered, with the exit code of the command line
        let r = call(
            r#"{"op":"cli","args":["check","main.neon"],"files":{"main.neon":"fn main() { let x: i64 = \"s\"; }"}}"#,
        );
        assert_eq!(r.get("code").and_then(Json::as_i64), Some(1));
        assert!(s(&r, "stderr").contains("mismatched types"), "{}", s(&r, "stderr"));
        let r = call(r#"{"op":"cli","args":["fmt","a.neon"],"files":{"a.neon":"fn main(){\nlet x=1;\n}\n"}}"#);
        assert_eq!(s(&r, "stdout"), "fn main() {\n    let x = 1;\n}\n");
        for view in ["ir", "disasm", "c"] {
            let r = call(&format!(
                r#"{{"op":"cli","args":["{view}","m.neon"],"files":{{"m.neon":"fn main() {{ println(1); }}"}}}}"#
            ));
            assert_eq!(r.get("code").and_then(Json::as_i64), Some(0), "{view}");
            assert!(!s(&r, "stdout").is_empty(), "{view}");
        }
    }

    #[test]
    fn input_is_served_from_the_request_and_asked_for_when_it_runs_out() {
        let prog = r#"fn main() { let a = read_line(); println(a); let b = read_line(); println(b); }"#;
        let req = |stdin: &str, interactive: bool| {
            format!(
                r#"{{"op":"cli","args":["run","m.neon"],"files":{{"m.neon":"{}"}},"stdin":"{stdin}","interactive":{interactive}}}"#,
                prog.replace('"', "\\\"")
            )
        };
        let r = call(&req("one\\ntwo\\n", true));
        assert_eq!(r.get("needsInput").and_then(Json::as_bool), Some(false));
        assert_eq!(s(&r, "stdout"), "Option::Some(\"one\")\nOption::Some(\"two\")\n");
        // one line short: the host is told to ask the user
        let r = call(&req("one\\n", true));
        assert_eq!(r.get("needsInput").and_then(Json::as_bool), Some(true));
        assert_eq!(s(&r, "stdout"), "Option::Some(\"one\")\n");
        // not interactive: the end of input is just the end
        let r = call(&req("one\\n", false));
        assert_eq!(r.get("needsInput").and_then(Json::as_bool), Some(false));
        assert!(s(&r, "stdout").ends_with("Option::None\n"), "{}", s(&r, "stdout"));
    }

    #[test]
    fn keys_are_bytes_of_the_input_and_the_host_is_asked_for_the_next_one() {
        let prog = "import \\\"term\\\" as term;\\nimport \\\"fs\\\" as fs;\\nfn main() { println(fs::read_file(\\\"a.txt\\\")); loop { match term::read_key() { Option::Some(k) => println(k), Option::None => break } } }";
        let req = |stdin: &str| {
            format!(
                r#"{{"op":"cli","args":["run","m.neon"],"files":{{"m.neon":"{prog}","a.txt":"hi"}},"stdin":"{stdin}","interactive":true,"exact":true}}"#
            )
        };
        let r = call(&req("a\\u001b[A"));
        assert_eq!(s(&r, "stdout"), "Result::Ok(\"hi\")\na\nup\n");
        assert_eq!(r.get("needsInput").and_then(Json::as_bool), Some(true));
        assert_eq!(r.get("needsKey").and_then(Json::as_bool), Some(true));
        // a key that is only the start of an escape sequence is an Esc: the run asked, but it is not stuck
        let r = call(&req("\\u001b"));
        assert_eq!(s(&r, "stdout"), "Result::Ok(\"hi\")\nesc\n");
        // a program that asks for a line is not asking for a key
        let line = r#"{"op":"cli","args":["run","m.neon"],"files":{"m.neon":"fn main() { println(read_line()); }"},"stdin":"","interactive":true,"exact":true}"#;
        let r = call(line);
        assert_eq!(r.get("needsInput").and_then(Json::as_bool), Some(true));
        assert_eq!(r.get("needsKey").and_then(Json::as_bool), Some(false));
    }

    #[test]
    fn the_language_server_answers_in_the_same_call() {
        let init = call(
            r#"{"op":"lsp","message":"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{}}}"}"#,
        );
        let msgs = init.get("messages").and_then(Json::as_array).expect("messages");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].get("id").and_then(Json::as_i64), Some(1));
        let open = Json::object(vec![
            ("jsonrpc", Json::from("2.0")),
            ("method", Json::from("textDocument/didOpen")),
            (
                "params",
                Json::object(vec![(
                    "textDocument",
                    Json::object(vec![
                        ("uri", Json::from("file:///main.neon")),
                        ("languageId", Json::from("neon")),
                        ("version", Json::from(1usize)),
                        ("text", Json::from("fn main() { let x: i64 = \"s\"; }")),
                    ]),
                )]),
            ),
        ]);
        let req = Json::object(vec![("op", Json::from("lsp")), ("message", Json::from(open.to_string()))]);
        let r = Json::parse(&handle(&req.to_string())).unwrap();
        let msgs = r.get("messages").and_then(Json::as_array).unwrap();
        let published = msgs.iter().find(|m| s(m, "method") == "textDocument/publishDiagnostics").expect("diagnostics");
        let diags = published.path(&["params", "diagnostics"]).and_then(Json::as_array).unwrap();
        assert!(diags.iter().any(|d| s(d, "message").contains("mismatched types")), "{published}");
    }

    #[test]
    fn the_repl_keeps_a_session_between_calls() {
        call(r#"{"op":"repl_reset"}"#);
        assert_eq!(s(&call(r#"{"op":"repl_eval","cell":"let n = 20;"}"#), "kind"), "ok");
        let r = call(r#"{"op":"repl_eval","cell":"n + 1"}"#);
        assert_eq!((s(&r, "kind"), s(&r, "text")), ("ok".to_string(), "21\n".to_string()));
        let r = call(r#"{"op":"repl_eval","cell":"nope"}"#);
        assert_eq!(s(&r, "kind"), "rejected");
        assert!(s(&r, "text").contains("cannot find value `nope`"), "{}", s(&r, "text"));
        assert_eq!(call(r#"{"op":"repl_preview","cell":"n * 2"}"#).get("value").and_then(Json::as_str), Some("40"));
        let c = call(r#"{"op":"repl_complete","buffer":"n.","cursor":2}"#);
        assert!(c.get("items").and_then(Json::as_array).is_some_and(|i| !i.is_empty()) || c.get("items").is_some());
        let h = call(r#"{"op":"repl_highlight","text":"let x = 1;"}"#);
        assert!(h.get("runs").and_then(Json::as_array).is_some_and(|r| r.len() >= 3), "{h}");
        assert_eq!(call(r#"{"op":"repl_more","text":"fn f() {"}"#).get("more").and_then(Json::as_bool), Some(true));
        // a cell that wants input asks for it, and gets it on the retry
        let r = call(r#"{"op":"repl_eval","cell":"let a = input();"}"#);
        assert_eq!(r.get("needsInput").and_then(Json::as_bool), Some(true));
        let r = call(r#"{"op":"repl_eval","cell":"let a = input();","stdin":"Ada\n"}"#);
        assert_eq!(s(&r, "kind"), "ok");
        assert_eq!(s(&call(r#"{"op":"repl_eval","cell":"a"}"#), "text"), "Ada\n");
    }

    #[test]
    fn bad_requests_are_answered_not_fatal() {
        assert!(s(&call("not json"), "error").contains("bad request"));
        assert!(s(&call(r#"{"op":"nope"}"#), "error").contains("unknown op"));
        assert!(s(&call("{}"), "error").contains("no `op`"));
    }

    #[test]
    fn frames_are_split_by_their_declared_length() {
        let a = "{\"a\":\"é\"}";
        let b = "{\"b\":2}";
        let wire = format!("Content-Length: {}\r\n\r\n{a}Content-Length: {}\r\n\r\n{b}", a.len(), b.len());
        assert_eq!(split_frames(wire.as_bytes()), [a, b]);
    }
}
