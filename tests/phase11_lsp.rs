//! The language server, driven in memory through its public protocol surface.

use neon::lsp::analysis::complete::{call_context, completion_context, CompletionContext};
use neon::lsp::json::Json;
use neon::lsp::server::{read_message, run, write_message, Server};
use neon::vfs::MemVfs;

/// A client talking to an in-process server.
struct Client<'v> {
    server: Server<'v, Vec<u8>>,
    next_id: i64,
    /// Notifications received since the last call.
    pub inbox: Vec<Json>,
}

fn parse_all(bytes: &[u8]) -> Vec<Json> {
    let mut cursor = std::io::Cursor::new(bytes.to_vec());
    let mut out = Vec::new();
    while let Some(text) = read_message(&mut cursor).unwrap() {
        out.push(Json::parse(&text).unwrap());
    }
    out
}

impl<'v> Client<'v> {
    fn new(vfs: &'v MemVfs) -> Client<'v> {
        let mut c = Client { server: Server::new(vfs, Vec::new()), next_id: 1, inbox: Vec::new() };
        let r = c.request("initialize", obj(vec![("capabilities", obj(vec![]))]));
        assert!(r.get("capabilities").is_some());
        c.notify("initialized", obj(vec![]));
        c
    }

    fn drain(&mut self) -> Vec<Json> {
        let out = std::mem::take(self.server.output_mut());
        parse_all(&out)
    }

    fn notify(&mut self, method: &str, params: Json) {
        let msg = obj(vec![("jsonrpc", "2.0".into()), ("method", method.into()), ("params", params)]);
        self.server.handle(msg);
        let out = self.drain();
        self.inbox.extend(out);
    }

    /// Sends a request and returns the `result` (panics on an error response).
    fn request(&mut self, method: &str, params: Json) -> Json {
        self.try_request(method, params).unwrap_or_else(|e| panic!("{method} failed: {e}"))
    }

    fn try_request(&mut self, method: &str, params: Json) -> Result<Json, String> {
        let id = self.next_id;
        self.next_id += 1;
        let msg =
            obj(vec![("jsonrpc", "2.0".into()), ("id", id.into()), ("method", method.into()), ("params", params)]);
        self.server.handle(msg);
        let mut result = None;
        for m in self.drain() {
            if m.get("id").and_then(Json::as_i64) == Some(id) {
                result = Some(match m.get("error") {
                    Some(e) => Err(e.get("message").and_then(Json::as_str).unwrap_or("").to_string()),
                    None => Ok(m.get("result").cloned().unwrap_or(Json::Null)),
                });
            } else {
                self.inbox.push(m);
            }
        }
        result.expect("no response")
    }

    fn open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            obj(vec![(
                "textDocument",
                obj(vec![
                    ("uri", uri.into()),
                    ("languageId", "neon".into()),
                    ("version", 1i64.into()),
                    ("text", text.into()),
                ]),
            )]),
        );
    }

    /// The most recent diagnostics published for `uri`.
    fn diagnostics(&self, uri: &str) -> Vec<Json> {
        self.inbox
            .iter()
            .rev()
            .find(|m| {
                m.get("method").and_then(Json::as_str) == Some("textDocument/publishDiagnostics")
                    && m.path(&["params", "uri"]).and_then(Json::as_str) == Some(uri)
            })
            .and_then(|m| m.path(&["params", "diagnostics"]).and_then(Json::as_array).map(<[Json]>::to_vec))
            .unwrap_or_else(|| panic!("no diagnostics published for {uri}"))
    }

    fn at(&mut self, method: &str, uri: &str, line: u32, ch: u32) -> Json {
        self.request(method, text_pos(uri, line, ch))
    }
}

fn obj(pairs: Vec<(&str, Json)>) -> Json {
    Json::object(pairs)
}

fn text_pos(uri: &str, line: u32, ch: u32) -> Json {
    obj(vec![
        ("textDocument", obj(vec![("uri", uri.into())])),
        ("position", obj(vec![("line", line.into()), ("character", ch.into())])),
    ])
}

fn range_of(v: &Json) -> (u32, u32, u32, u32) {
    let r = v.get("range").unwrap_or(v);
    let g = |a: &str, b: &str| r.path(&[a, b]).and_then(Json::as_u32).unwrap();
    (g("start", "line"), g("start", "character"), g("end", "line"), g("end", "character"))
}

fn labels(items: &Json) -> Vec<String> {
    items
        .get("items")
        .and_then(Json::as_array)
        .unwrap()
        .iter()
        .map(|i| i.get("label").and_then(Json::as_str).unwrap().to_string())
        .collect()
}

const URI: &str = "file:///proj/main.neon";

const DEMO: &str = "\
struct Point { x: i64, y: i64 }
impl Point {
    fn new(x: i64, y: i64) -> Point { Point { x: x, y: y } }
    fn sum(self) -> i64 { self.x + self.y }
}
fn add(a: i64, b: i64) -> i64 { a + b }
let p = Point::new(1, 2);
let total = add(p.x, p.sum());
println(total);
";

// ---- lifecycle ---------------------------------------------------------------------------------

#[test]
fn requests_before_initialize_are_refused_and_shutdown_orders_the_exit_code() {
    let vfs = MemVfs::new();
    let mut s = Server::new(&vfs, Vec::new());
    let early = obj(vec![
        ("jsonrpc", "2.0".into()),
        ("id", 1i64.into()),
        ("method", "textDocument/hover".into()),
        ("params", Json::Null),
    ]);
    s.handle(early);
    let out = parse_all(&std::mem::take(s.output_mut()));
    assert_eq!(out[0].path(&["error", "code"]).and_then(Json::as_i64), Some(-32002));

    let mut c = Client::new(&vfs);
    c.request("shutdown", Json::Null);
    c.notify("exit", Json::Null);
    assert_eq!(c.server.exit_code, Some(0));
    // exit without shutdown is an error exit
    let mut c2 = Client::new(&vfs);
    c2.notify("exit", Json::Null);
    assert_eq!(c2.server.exit_code, Some(1));
}

#[test]
fn unknown_methods_and_bad_json_get_error_responses() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    let e = c.try_request("textDocument/nonsense", Json::Null).unwrap_err();
    assert!(e.contains("not supported"), "{e}");
    c.server.handle_text("{ not json");
    let out = c.drain();
    assert_eq!(out[0].path(&["error", "code"]).and_then(Json::as_i64), Some(-32700));
    // requests about documents that were never opened fail cleanly
    let e = c.try_request("textDocument/hover", text_pos("file:///nope.neon", 0, 0)).unwrap_err();
    assert!(e.contains("not open"), "{e}");
}

#[test]
fn framing_round_trips_and_tolerates_extra_headers() {
    let body = Json::object(vec![("a", Json::from(1u32))]);
    let mut buf = Vec::new();
    write_message(&mut buf, &body).unwrap();
    assert!(String::from_utf8_lossy(&buf).starts_with("Content-Length: 7\r\n\r\n"));
    let mut cur = std::io::Cursor::new(buf);
    assert_eq!(read_message(&mut cur).unwrap().as_deref(), Some("{\"a\":1}"));
    assert_eq!(read_message(&mut cur).unwrap(), None);
    let raw = "content-type: application/vscode-jsonrpc; charset=utf-8\r\nCONTENT-LENGTH: 2\r\n\r\n{}";
    assert_eq!(read_message(&mut std::io::Cursor::new(raw.as_bytes().to_vec())).unwrap().as_deref(), Some("{}"));
    assert!(read_message(&mut std::io::Cursor::new(b"X: 1\r\n\r\n".to_vec())).is_err());
    // multi-byte bodies are counted in bytes
    let mut buf = Vec::new();
    write_message(&mut buf, &Json::from("héllo")).unwrap();
    assert_eq!(read_message(&mut std::io::Cursor::new(buf)).unwrap().as_deref(), Some("\"héllo\""));
}

#[test]
fn run_loop_processes_a_whole_session() {
    let vfs = MemVfs::new();
    let mut input = Vec::new();
    for m in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
        r#"{"jsonrpc":"2.0","method":"exit"}"#,
    ] {
        input.extend_from_slice(format!("Content-Length: {}\r\n\r\n{m}", m.len()).as_bytes());
    }
    let mut out = Vec::new();
    let code = run(std::io::Cursor::new(input), &mut out, &vfs);
    assert_eq!(code, 0);
    let msgs = parse_all(&out);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].get("result"), Some(&Json::Null));
    // end of input without `exit` is an abnormal termination
    assert_eq!(run(std::io::Cursor::new(Vec::new()), Vec::new(), &vfs), 1);
}

// ---- diagnostics ------------------------------------------------------------------------------------

#[test]
fn diagnostics_follow_edits() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    c.open(URI, "let x: i32 = \"s\";\nprintln(y);\n");
    let d = c.diagnostics(URI);
    assert_eq!(d.len(), 3, "{d:?}");
    assert_eq!(range_of(&d[0]), (0, 13, 0, 16));
    assert_eq!(d[0].get("code").and_then(Json::as_str), Some("E0211"));
    assert_eq!(d[0].get("severity").and_then(Json::as_i64), Some(1));
    assert_eq!(d[0].get("source").and_then(Json::as_str), Some("neon"));
    assert!(d[1].get("message").and_then(Json::as_str).unwrap().contains("cannot find value `y`"));

    // fix the first line with an incremental edit
    c.notify(
        "textDocument/didChange",
        obj(vec![
            ("textDocument", obj(vec![("uri", URI.into()), ("version", 2i64.into())])),
            (
                "contentChanges",
                vec![obj(vec![
                    (
                        "range",
                        obj(vec![
                            ("start", obj(vec![("line", 0u32.into()), ("character", 13u32.into())])),
                            ("end", obj(vec![("line", 0u32.into()), ("character", 16u32.into())])),
                        ]),
                    ),
                    ("text", "5".into()),
                ])]
                .into(),
            ),
        ]),
    );
    let d = c.diagnostics(URI);
    assert_eq!(d.len(), 2, "{d:?}");
    // a full-document replacement fixes the rest
    c.notify(
        "textDocument/didChange",
        obj(vec![
            ("textDocument", obj(vec![("uri", URI.into()), ("version", 3i64.into())])),
            ("contentChanges", vec![obj(vec![("text", "let x = 1;\nprintln(x);\n".into())])].into()),
        ]),
    );
    assert!(c.diagnostics(URI).is_empty());
    // closing a document clears its diagnostics
    c.notify("textDocument/didClose", obj(vec![("textDocument", obj(vec![("uri", URI.into())]))]));
    assert!(c.diagnostics(URI).is_empty());
}

#[test]
fn syntax_errors_are_reported_together_with_type_errors() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    c.open(URI, "fn f( { 1 }\nlet a: i32 = \"x\";\nlet mut b = 1;\nif a { 1 } else if b { 2 }\n");
    let d = c.diagnostics(URI);
    let codes: Vec<&str> = d.iter().filter_map(|x| x.get("code").and_then(Json::as_str)).collect();
    assert!(codes.contains(&"E0101") && codes.contains(&"E0211"), "{codes:?}");
    assert!(codes.contains(&"E0109") && codes.contains(&"E0110"), "old spellings are explained: {codes:?}");
    let text: String = d.iter().filter_map(|x| x.get("message").and_then(Json::as_str)).collect::<Vec<_>>().join("|");
    assert!(text.contains("`let mut` is spelled `var`") && text.contains("`else if` is spelled `elif`"), "{text}");
}

#[test]
fn diagnostic_columns_count_utf16_units() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    // the emoji takes two UTF-16 units, "é" one
    c.open(URI, "let s = \"é😀\"; let n: i32 = s;\n");
    let d = c.diagnostics(URI);
    assert_eq!(d.len(), 1, "{d:?}");
    // `s` after `= ` : bytes differ from utf-16 columns
    let line = "let s = \"é😀\"; let n: i32 = s;";
    let byte = line.rfind('s').unwrap();
    let expected = line[..byte].encode_utf16().count() as u32;
    assert_eq!(range_of(&d[0]).1, expected);
    assert_ne!(byte as u32, expected);
}

// ---- navigation ----------------------------------------------------------------------------------------

#[test]
fn hover_describes_definitions_and_expressions() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    c.open(URI, DEMO);
    let hover = |c: &mut Client<'_>, l, ch| -> String {
        let r = c.at("textDocument/hover", URI, l, ch);
        r.path(&["contents", "value"]).and_then(Json::as_str).unwrap_or("<none>").to_string()
    };
    assert!(hover(&mut c, 7, 13).contains("fn add(a: i64, b: i64) -> i64"), "call of add");
    assert!(hover(&mut c, 8, 9).contains("let total: i64"), "a variable");
    assert!(hover(&mut c, 0, 8).contains("struct Point {") && hover(&mut c, 0, 8).contains("x: i64"));
    let field = hover(&mut c, 3, 31);
    assert!(field.contains("// field of `Point`") && field.contains("x: i64"), "{field}");
    let sum = hover(&mut c, 7, 24);
    assert!(sum.contains("fn sum(self) -> i64") && sum.contains("in `impl Point`"), "{sum}");
    // a literal shows its type
    assert!(hover(&mut c, 6, 19).contains("i64"), "{}", hover(&mut c, 6, 19));
    // nothing interesting on whitespace
    assert_eq!(hover(&mut c, 9, 0), "<none>");
}

#[test]
fn hover_on_builtin_methods_and_constants() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    c.open(URI, "const LIMIT: i64 = 6 * 7;\nvar xs = [1, 2, 3];\nxs.push(LIMIT);\nprintln(xs.len());\n");
    let h = |c: &mut Client<'_>, l, ch| {
        c.at("textDocument/hover", URI, l, ch)
            .path(&["contents", "value"])
            .and_then(Json::as_str)
            .unwrap_or("<none>")
            .to_string()
    };
    let limit = h(&mut c, 2, 10);
    assert!(limit.contains("const LIMIT: i64 = 6 * 7"), "{limit}");
    let push = h(&mut c, 2, 4);
    assert!(push.contains("[i64].push(i64)") || push.contains("push"), "{push}");
    let xs = h(&mut c, 1, 5);
    assert!(xs.contains("var xs: [i64]"), "{xs}");
}

#[test]
fn go_to_definition_within_a_file() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    c.open(URI, DEMO);
    let def = |c: &mut Client<'_>, l, ch| c.at("textDocument/definition", URI, l, ch);
    // `add` at its call
    let r = def(&mut c, 7, 13);
    assert_eq!(r.as_array().unwrap().len(), 1);
    assert_eq!(r.as_array().unwrap()[0].get("uri").and_then(Json::as_str), Some(URI));
    assert_eq!(range_of(&r.as_array().unwrap()[0]), (5, 3, 5, 6));
    // the local `p` in `p.x`
    let r = def(&mut c, 7, 16);
    assert_eq!(range_of(&r.as_array().unwrap()[0]), (6, 4, 6, 5));
    // the struct from a path expression
    let r = def(&mut c, 6, 9);
    assert_eq!(range_of(&r.as_array().unwrap()[0]), (0, 7, 0, 12));
    // a keyword has no definition
    assert!(def(&mut c, 6, 0).is_null());
}

#[test]
fn definitions_cross_files_and_see_unsaved_edits() {
    let vfs = MemVfs::new().with_file("/proj/util.neon", "export fn twice(x: i64) -> i64 { x * 2 }\n");
    let mut c = Client::new(&vfs);
    let main = "import { twice } from \"util\";\nprintln(twice(21));\n";
    c.open(URI, main);
    assert!(c.diagnostics(URI).is_empty(), "{:?}", c.diagnostics(URI));
    let r = c.at("textDocument/definition", URI, 1, 9);
    let loc = &r.as_array().unwrap()[0];
    assert_eq!(loc.get("uri").and_then(Json::as_str), Some("file:///proj/util.neon"));
    assert_eq!(range_of(loc), (0, 10, 0, 15));

    // open the library with an unsaved edit that renames the function: main now has an error
    c.open("file:///proj/util.neon", "export fn double(x: i64) -> i64 { x * 2 }\n");
    let d = c.diagnostics(URI);
    assert!(!d.is_empty(), "main must notice the edit of util.neon");
    // closing the library goes back to the saved version
    c.notify("textDocument/didClose", obj(vec![("textDocument", obj(vec![("uri", "file:///proj/util.neon".into())]))]));
    assert!(c.diagnostics(URI).is_empty());
}

#[test]
fn references_highlights_and_rename() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    c.open(URI, DEMO);
    // `total`: declared on line 6, used on line 7
    let refs = c.request(
        "textDocument/references",
        obj(vec![
            ("textDocument", obj(vec![("uri", URI.into())])),
            ("position", obj(vec![("line", 8u32.into()), ("character", 9u32.into())])),
            ("context", obj(vec![("includeDeclaration", true.into())])),
        ]),
    );
    let ranges: Vec<_> = refs.as_array().unwrap().iter().map(range_of).collect();
    assert_eq!(ranges, vec![(7, 4, 7, 9), (8, 8, 8, 13)]);
    let refs = c.request(
        "textDocument/references",
        obj(vec![
            ("textDocument", obj(vec![("uri", URI.into())])),
            ("position", obj(vec![("line", 8u32.into()), ("character", 9u32.into())])),
            ("context", obj(vec![("includeDeclaration", false.into())])),
        ]),
    );
    assert_eq!(refs.as_array().unwrap().len(), 1);
    let hl = c.at("textDocument/documentHighlight", URI, 8, 9);
    assert_eq!(hl.as_array().unwrap().len(), 2);

    // rename `add` everywhere
    let prep = c.at("textDocument/prepareRename", URI, 7, 13);
    assert_eq!(prep.get("placeholder").and_then(Json::as_str), Some("add"));
    let edit = c.request(
        "textDocument/rename",
        obj(vec![
            ("textDocument", obj(vec![("uri", URI.into())])),
            ("position", obj(vec![("line", 7u32.into()), ("character", 13u32.into())])),
            ("newName", "plus".into()),
        ]),
    );
    let edits = edit.path(&["changes", URI]).and_then(Json::as_array).unwrap();
    let mut ranges: Vec<_> = edits.iter().map(range_of).collect();
    ranges.sort();
    assert_eq!(ranges, vec![(5, 3, 5, 6), (7, 12, 7, 15)]);
    assert!(edits.iter().all(|e| e.get("newText").and_then(Json::as_str) == Some("plus")));

    // invalid targets and names are refused with a reason
    for new in ["", "1x", "has space", "fn", "_"] {
        let e = c
            .try_request(
                "textDocument/rename",
                obj(vec![
                    ("textDocument", obj(vec![("uri", URI.into())])),
                    ("position", obj(vec![("line", 7u32.into()), ("character", 13u32.into())])),
                    ("newName", new.into()),
                ]),
            )
            .unwrap_err();
        assert!(e.contains("identifier") || e.contains("keyword"), "{new}: {e}");
    }
    // a built-in (the prelude's `Option`) cannot be renamed
    let mut c2 = Client::new(&vfs);
    c2.open(URI, "let a: Option<i64> = Option::None;\n");
    let e = c2.try_request("textDocument/prepareRename", text_pos(URI, 0, 9)).unwrap_err();
    assert!(e.contains("built-in"), "{e}");
}

#[test]
fn document_symbols_form_an_outline() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    c.open(URI, &format!("{DEMO}enum Shape {{ Dot, Circle(f64) }}\nconst K = 1;\ntrait T {{ fn go(self); }}\n"));
    let syms = c.request("textDocument/documentSymbol", obj(vec![("textDocument", obj(vec![("uri", URI.into())]))]));
    let syms = syms.as_array().unwrap();
    let name = |s: &Json| s.get("name").and_then(Json::as_str).unwrap().to_string();
    let names: Vec<String> = syms.iter().map(name).collect();
    assert_eq!(names, ["Point", "impl Point", "add", "p", "total", "Shape", "K", "T"]);
    let point = &syms[0];
    assert_eq!(point.get("kind").and_then(Json::as_i64), Some(23));
    assert_eq!(point.get("children").and_then(Json::as_array).unwrap().len(), 2);
    let imp = &syms[1];
    let methods: Vec<String> = imp.get("children").and_then(Json::as_array).unwrap().iter().map(name).collect();
    assert_eq!(methods, ["new", "sum"]);
    assert_eq!(imp.get("children").and_then(Json::as_array).unwrap()[0].get("kind").and_then(Json::as_i64), Some(6));
    let shape = syms.iter().find(|s| name(s) == "Shape").unwrap();
    assert_eq!(shape.get("children").and_then(Json::as_array).unwrap().len(), 2);
    // every selection range lies inside its symbol's range
    for s in syms {
        let (a, b, c2, d) = range_of(s);
        let sel = s.get("selectionRange").unwrap();
        let (sa, sb, _, _) = range_of(sel);
        assert!((a, b) <= (sa, sb) && (sa, sb) <= (c2, d), "{s}");
    }
}

// ---- completion -------------------------------------------------------------------------------------------

#[test]
fn completion_of_members_scope_paths_and_primitives() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    let src = "\
struct Point { x: i64, y: i64 }
impl Point { fn sum(self) -> i64 { self.x + self.y } fn make() -> Point { Point { x: 0, y: 0 } } }
enum Shape { Dot, Circle(f64) }
fn helper() {}
let p = Point::make();
let xs = [1, 2];
let name = \"neon\";
p.;
xs.;
name.;
Shape::;
i32::;
hel;
";
    c.open(URI, src);
    let at = |c: &mut Client<'_>, line: u32, ch: u32| labels(&c.at("textDocument/completion", URI, line, ch));
    let p = at(&mut c, 7, 2);
    assert!(p.contains(&"x".to_string()) && p.contains(&"y".to_string()) && p.contains(&"sum".to_string()), "{p:?}");
    assert!(!p.contains(&"make".to_string()), "associated functions are not methods: {p:?}");
    let xs = at(&mut c, 8, 3);
    assert!(
        xs.contains(&"push".to_string()) && xs.contains(&"len".to_string()) && xs.contains(&"sort".to_string()),
        "{xs:?}"
    );
    assert!(!xs.contains(&"to_upper".to_string()), "no string methods on arrays");
    let s = at(&mut c, 9, 5);
    assert!(
        s.contains(&"to_upper".to_string()) && s.contains(&"split".to_string()) && !s.contains(&"push".to_string()),
        "{s:?}"
    );
    let shape = at(&mut c, 10, 7);
    assert_eq!(shape, ["Dot", "Circle"]);
    let prim = at(&mut c, 11, 5);
    assert!(prim.contains(&"MAX".to_string()) && prim.contains(&"MIN".to_string()), "{prim:?}");
    let scope = at(&mut c, 12, 3);
    for want in
        ["helper", "p", "xs", "name", "Point", "Shape", "println", "Option", "Result", "let", "var", "elif", "const"]
    {
        assert!(scope.contains(&want.to_string()), "{want} missing from {scope:?}");
    }
    assert!(!scope.contains(&"mut".to_string()), "`mut` is gone");
}

#[test]
fn completion_sees_locals_and_stays_quiet_in_strings_and_comments() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    let src = "fn f(first: i64) -> i64 {\n    let second = 2;\n    \n}\nlet outside = \"text\"; // note\n";
    c.open(URI, src);
    let inside = labels(&c.at("textDocument/completion", URI, 2, 4));
    assert!(inside.contains(&"first".to_string()) && inside.contains(&"second".to_string()), "{inside:?}");
    let outside = labels(&c.at("textDocument/completion", URI, 4, 0));
    assert!(!outside.contains(&"first".to_string()) && !outside.contains(&"second".to_string()), "{outside:?}");
    assert!(labels(&c.at("textDocument/completion", URI, 4, 18)).is_empty(), "inside a string");
    assert!(labels(&c.at("textDocument/completion", URI, 4, 28)).is_empty(), "inside a comment");
}

#[test]
fn completion_context_detection() {
    let t = "let a = foo.ba";
    assert_eq!(completion_context(t, t.len()), CompletionContext::Members { dot: 11 });
    let t = "Shape::Ci";
    assert_eq!(completion_context(t, t.len()), CompletionContext::Path { qualifier: "Shape".into() });
    assert_eq!(completion_context("let x = 1.", 10), CompletionContext::Nothing, "a float, not member access");
    assert_eq!(completion_context("let x = pri", 11), CompletionContext::Scope);
    assert_eq!(completion_context("let x = \"ab.c", 13), CompletionContext::Nothing);
    assert_eq!(completion_context("// a.b", 6), CompletionContext::Nothing);
    assert_eq!(completion_context("/* open ", 8), CompletionContext::Nothing);
    assert_eq!(completion_context("f(a, 'x', \"s)\") + g", 18), CompletionContext::Scope);
}

#[test]
fn call_context_counts_arguments() {
    let t = "let r = add(1, sub(2, 3), ";
    let ctx = call_context(t, t.len()).unwrap();
    assert_eq!((ctx.name_start, ctx.active, ctx.is_method), (8, 2, false));
    let t = "add(1, sub(2, ";
    let ctx = call_context(t, t.len()).unwrap();
    assert_eq!((&t[ctx.name_start..ctx.name_start + 3], ctx.active), ("sub", 1));
    let t = "xs.push(";
    let ctx = call_context(t, t.len()).unwrap();
    assert!(ctx.is_method && ctx.active == 0);
    assert!(call_context("add(1, 2)", 9).is_none(), "the call is closed");
    assert!(call_context("(1 + ", 5).is_none(), "grouping parentheses are not calls");
    assert!(call_context("f(\"a, b\", ", 10).unwrap().active == 1, "commas in strings do not count");
}

#[test]
fn signature_help_marks_the_active_parameter() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    c.open(URI, "fn add(a: i64, b: i64) -> i64 { a + b }\nstruct S { v: i64 }\nimpl S { fn put(self, k: i64, w: i64) {} }\nlet s = S { v: 1 };\nadd(1, \ns.put(1, \n");
    let sig = c.at("textDocument/signatureHelp", URI, 4, 7);
    assert_eq!(
        sig.path(&["signatures"]).and_then(Json::as_array).unwrap()[0].get("label").and_then(Json::as_str),
        Some("fn add(a: i64, b: i64) -> i64")
    );
    assert_eq!(sig.get("activeParameter").and_then(Json::as_i64), Some(1));
    let first = c.at("textDocument/signatureHelp", URI, 4, 4);
    assert_eq!(first.get("activeParameter").and_then(Json::as_i64), Some(0));
    // a method call: the receiver is not a parameter
    let m = c.at("textDocument/signatureHelp", URI, 5, 9);
    let params = m.path(&["signatures"]).and_then(Json::as_array).unwrap()[0]
        .get("parameters")
        .and_then(Json::as_array)
        .unwrap()
        .len();
    assert_eq!(params, 2, "{m}");
    assert_eq!(m.get("activeParameter").and_then(Json::as_i64), Some(1));
    // not inside a call
    assert!(c.at("textDocument/signatureHelp", URI, 3, 4).is_null());
}

// ---- semantic tokens -------------------------------------------------------------------------------------------

#[test]
fn semantic_tokens_classify_names_keywords_and_literals() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    c.open(URI, "// hi\nfn add(a: i64) -> i64 { a + 1 }\nlet n = add(2);\nvar s = \"x\";\n");
    let r = c.request("textDocument/semanticTokens/full", obj(vec![("textDocument", obj(vec![("uri", URI.into())]))]));
    let data: Vec<u32> = r.get("data").and_then(Json::as_array).unwrap().iter().map(|v| v.as_u32().unwrap()).collect();
    assert_eq!(data.len() % 5, 0);
    // decode to (line, col, len, type, modifiers)
    let (mut line, mut col) = (0, 0);
    let mut toks = Vec::new();
    for t in data.chunks(5) {
        line += t[0];
        col = if t[0] == 0 { col + t[1] } else { t[1] };
        toks.push((line, col, t[2], t[3], t[4]));
    }
    let types = neon::lsp::analysis::TOKEN_TYPES;
    let find = |l, ch| toks.iter().find(|t| t.0 == l && t.1 == ch).map(|t| (types[t.3 as usize], t.4));
    assert_eq!(find(0, 0), Some(("comment", 0)));
    assert_eq!(find(1, 0), Some(("keyword", 0)));
    assert_eq!(find(1, 3), Some(("function", 1)), "the declaration of add");
    assert_eq!(find(1, 7), Some(("parameter", 1 | 2)), "an immutable parameter, declared here");
    assert_eq!(find(1, 10), Some(("type", 4)), "a primitive type is a default-library type");
    assert_eq!(find(1, 24), Some(("parameter", 2)), "a use of the parameter");
    assert_eq!(find(1, 28), Some(("number", 0)));
    assert_eq!(find(2, 8), Some(("function", 0)), "a call of add");
    assert_eq!(find(3, 0), Some(("keyword", 0)));
    assert_eq!(find(3, 4), Some(("variable", 1)), "var s is a mutable variable");
    assert_eq!(find(3, 8), Some(("string", 0)));
}

// ---- the real binary --------------------------------------------------------------------------------------------

#[test]
fn the_neon_binary_speaks_lsp_over_stdio() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut input = Vec::new();
    let msgs = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#.to_string(),
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#.to_string(),
        Json::object(vec![
            ("jsonrpc", "2.0".into()),
            ("method", "textDocument/didOpen".into()),
            (
                "params",
                obj(vec![(
                    "textDocument",
                    obj(vec![
                        ("uri", URI.into()),
                        ("languageId", "neon".into()),
                        ("version", 1i64.into()),
                        ("text", "let x: i32 = true;\n".into()),
                    ]),
                )]),
            ),
        ])
        .to_string(),
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#.to_string(),
        r#"{"jsonrpc":"2.0","method":"exit"}"#.to_string(),
    ];
    for m in &msgs {
        write!(input, "Content-Length: {}\r\n\r\n{m}", m.len()).unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_neon"))
        .args(["lsp", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let msgs = parse_all(&out.stdout);
    assert_eq!(msgs.len(), 3, "initialize, diagnostics, shutdown");
    let diag = &msgs[1];
    assert_eq!(diag.get("method").and_then(Json::as_str), Some("textDocument/publishDiagnostics"));
    assert_eq!(diag.path(&["params", "diagnostics"]).and_then(Json::as_array).unwrap().len(), 1);
}

// ---- robustness -------------------------------------------------------------------------------------------------

/// Every request kind at several positions of every prefix of every corpus program. Half-typed
/// code is the normal state of a document being edited; the server must answer (possibly with
/// nothing), never panic.
#[test]
fn no_request_panics_on_partially_typed_programs() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus");
    let mut programs: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "neon"))
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    programs.sort();
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    let mut requests = 0;
    for src in programs.iter().step_by(3) {
        let boundaries: Vec<usize> = (0..=src.len()).filter(|&i| src.is_char_boundary(i)).collect();
        let stride = (boundaries.len() / 14).max(1);
        for &cut in boundaries.iter().step_by(stride) {
            let text = &src[..cut];
            let lines = text.matches('\n').count() as u32;
            let last_col = text.rsplit('\n').next().unwrap().encode_utf16().count() as u32;
            c.open(URI, text);
            for (line, ch) in [(lines, last_col), (lines, last_col.saturating_sub(1)), (0, 0), (lines / 2, 3)] {
                for method in [
                    "textDocument/hover",
                    "textDocument/definition",
                    "textDocument/completion",
                    "textDocument/signatureHelp",
                    "textDocument/documentHighlight",
                    "textDocument/prepareRename",
                ] {
                    let _ = c.try_request(method, text_pos(URI, line, ch));
                    requests += 1;
                }
                let _ = c.try_request(
                    "textDocument/references",
                    obj(vec![
                        ("textDocument", obj(vec![("uri", URI.into())])),
                        ("position", obj(vec![("line", line.into()), ("character", ch.into())])),
                        ("context", obj(vec![("includeDeclaration", true.into())])),
                    ]),
                );
            }
            for method in ["textDocument/documentSymbol", "textDocument/semanticTokens/full"] {
                let r = c.request(method, obj(vec![("textDocument", obj(vec![("uri", URI.into())]))]));
                assert!(r.as_array().is_some() || r.get("data").is_some());
                requests += 1;
            }
            c.inbox.clear();
        }
    }
    assert!(requests > 1000, "{requests}");
}

#[test]
fn a_panicking_request_becomes_an_error_not_a_crash() {
    // positions far outside the document are clamped, not trusted
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    c.open(URI, "let x = 1;\n");
    for (l, ch) in [(0, 99999), (99999, 0), (u32::MAX, u32::MAX)] {
        let _ = c.try_request("textDocument/hover", text_pos(URI, l, ch));
        let _ = c.try_request("textDocument/completion", text_pos(URI, l, ch));
    }
    // and the session still works
    let r = c.at("textDocument/hover", URI, 0, 4);
    assert!(r.path(&["contents", "value"]).and_then(Json::as_str).unwrap().contains("let x"));
}

#[test]
fn fields_and_methods_are_navigable() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    c.open(URI, DEMO);
    // `p.x` on line 7: the field's definition, hover and references
    let r = c.at("textDocument/definition", URI, 7, 18);
    assert_eq!(range_of(&r.as_array().unwrap()[0]), (0, 15, 0, 16), "struct Point's field x");
    let refs = c.request(
        "textDocument/references",
        obj(vec![
            ("textDocument", obj(vec![("uri", URI.into())])),
            ("position", obj(vec![("line", 0u32.into()), ("character", 15u32.into())])),
            ("context", obj(vec![("includeDeclaration", true.into())])),
        ]),
    );
    let ranges: Vec<_> = refs.as_array().unwrap().iter().map(range_of).collect();
    assert!(
        ranges.contains(&(0, 15, 0, 16)) && ranges.contains(&(7, 18, 7, 19)) && ranges.contains(&(3, 31, 3, 32)),
        "{ranges:?}"
    );
    // a method: go to its declaration
    let r = c.at("textDocument/definition", URI, 7, 24);
    assert_eq!(range_of(&r.as_array().unwrap()[0]), (3, 7, 3, 10));
    // renaming a field updates declaration, struct literals and accesses
    let edit = c.request(
        "textDocument/rename",
        obj(vec![
            ("textDocument", obj(vec![("uri", URI.into())])),
            ("position", obj(vec![("line", 7u32.into()), ("character", 18u32.into())])),
            ("newName", "px".into()),
        ]),
    );
    let n = edit.path(&["changes", URI]).and_then(Json::as_array).unwrap().len();
    assert!(n >= 4, "declaration, `x: x` in the literal, self.x, p.x: {edit}");
}

#[test]
fn module_aliases_complete_and_navigate() {
    let vfs = MemVfs::new().with_file(
        "/proj/geo.neon",
        "export fn area(w: i64, h: i64) -> i64 { w * h }\nexport const SIDES = 4;\nexport struct Rect { w: i64 }\n",
    );
    let mut c = Client::new(&vfs);
    c.open(URI, "import \"geo\" as g;\nlet a = g::area(2, 3);\nlet z = g::;\n");
    let items = labels(&c.at("textDocument/completion", URI, 2, 11));
    assert!(
        items.contains(&"area".to_string())
            && items.contains(&"SIDES".to_string())
            && items.contains(&"Rect".to_string()),
        "{items:?}"
    );
    let r = c.at("textDocument/definition", URI, 1, 12);
    assert_eq!(r.as_array().unwrap()[0].get("uri").and_then(Json::as_str), Some("file:///proj/geo.neon"));
    // renaming something defined in another file edits that file too
    let edit = c.request(
        "textDocument/rename",
        obj(vec![
            ("textDocument", obj(vec![("uri", URI.into())])),
            ("position", obj(vec![("line", 1u32.into()), ("character", 12u32.into())])),
            ("newName", "surface".into()),
        ]),
    );
    let changes = edit.get("changes").unwrap();
    assert!(changes.get("file:///proj/geo.neon").is_some() && changes.get(URI).is_some(), "{edit}");
}

// ---- formatting --------------------------------------------------------------------------------

#[test]
fn formatting_returns_one_edit_for_the_whole_document() {
    let vfs = MemVfs::new();
    let mut c = Client::new(&vfs);
    let caps = c.request("initialize", obj(vec![("capabilities", obj(vec![]))]));
    assert_eq!(caps.path(&["capabilities", "documentFormattingProvider"]), Some(&Json::Bool(true)));
    let messy = "fn main() {\nlet x=1+2;\n   println(x);\n}";
    c.open(URI, messy);
    let params = obj(vec![
        ("textDocument", obj(vec![("uri", URI.into())])),
        ("options", obj(vec![("tabSize", 4u32.into()), ("insertSpaces", true.into())])),
    ]);
    let edits = c.request("textDocument/formatting", params.clone());
    let edits = edits.as_array().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(range_of(&edits[0]), (0, 0, 3, 1));
    assert_eq!(
        edits[0].get("newText").and_then(Json::as_str),
        Some("fn main() {\n    let x = 1 + 2;\n    println(x);\n}\n")
    );

    // already formatted: no edits; broken source: no edits and no error
    c.open(URI, "fn main() {\n}\n");
    assert_eq!(c.request("textDocument/formatting", params.clone()).as_array().unwrap().len(), 0);
    c.open(URI, "fn main( {");
    assert_eq!(c.request("textDocument/formatting", params).as_array().unwrap().len(), 0);
}
