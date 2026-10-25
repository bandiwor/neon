//! A read-eval-print loop.
//!
//! Neon programs are compiled whole, so the session keeps the source of every
//! successful cell and *replays* it: each evaluation compiles all previous
//! cells plus the new one, runs them with output suppressed (recorded `input()`
//! answers are fed back), and shows only what the new cell prints. A cell that
//! fails to compile or traps is dropped, leaving the session as it was.

use std::collections::VecDeque;
use std::io::{self, BufRead, Write};

use crate::driver::{compile_source, Options};
use crate::vfs::Vfs;
use crate::vm::{self, VmConfig, VmIo};

/// Marks the end of the replayed history in the program's output.
const MARK: &str = "\u{1}";
/// Marks where an expression cell's value starts (after its side effects).
const VALUE_MARK: &str = "\u{2}";

#[derive(Debug, Clone, Copy, Default)]
pub struct ReplConfig {
    pub options: Options,
    pub vm: VmConfig,
    /// Print `>>> ` / `... ` prompts (for terminals).
    pub prompt: bool,
}

const HELP: &str = "\
Enter statements, declarations or expressions; expressions print their value.
Unfinished input (open brackets) continues on the next line.
  :help          this text
  :quit, :q      leave
  :reset         forget everything
  :history       show the accepted cells
  :load <file>   evaluate a file as one cell
";

/// Whether `text` ends inside an open bracket, string or block comment.
pub fn needs_more(text: &str) -> bool {
    let mut depth = 0i32;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '"' => {
                // a string literal; an unterminated one on this line means "keep reading"
                let mut closed = false;
                while let Some(d) = chars.next() {
                    if d == '\\' {
                        chars.next();
                    } else if d == '"' {
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return true;
                }
            }
            '\'' => {
                // a char literal like 'a' or '\n'; a lone quote is left to the lexer to reject
                let mut look = chars.clone();
                let first = look.next();
                if first == Some('\\') {
                    look.next();
                }
                if let Some(pos) = look.position(|d| d == '\'') {
                    if pos < 12 {
                        for _ in 0..(if first == Some('\\') { 1 } else { 0 }) + pos + 1 + 1 {
                            chars.next();
                        }
                        // `chars` is now after the closing quote only when the scan above matched
                    }
                }
            }
            '/' => match chars.peek() {
                Some('/') => {
                    for d in chars.by_ref() {
                        if d == '\n' {
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut prev = ' ';
                    let mut closed = false;
                    for d in chars.by_ref() {
                        if prev == '*' && d == '/' {
                            closed = true;
                            break;
                        }
                        prev = d;
                    }
                    if !closed {
                        return true;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    depth > 0
}

/// Whether a cell is a bare expression whose value should be shown.
fn is_expression(cell: &str) -> bool {
    let t = cell.trim();
    if t.is_empty() || t.ends_with(';') || t.ends_with('}') {
        return false;
    }
    let first = t.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or("");
    !matches!(
        first,
        "fn" | "struct"
            | "enum"
            | "trait"
            | "impl"
            | "let"
            | "const"
            | "import"
            | "export"
            | "for"
            | "while"
            | "loop"
            | "if"
            | "match"
            | "return"
            | "break"
            | "continue"
    )
}

/// Routes the program's I/O during a replay: the replayed part is silent and
/// re-reads recorded input; the part after the marker goes to the session.
struct ReplayIo<'a> {
    replay: VecDeque<String>,
    live: &'a mut dyn BufRead,
    fresh_input: Vec<String>,
    silent: bool,
    output: String,
    value_start: Option<usize>,
    read_error: Option<String>,
}

impl VmIo for ReplayIo<'_> {
    /// The markers are single characters inside the output, wherever the optimizer happened to
    /// cut the writes: adjacent constant prints may arrive as one chunk.
    fn print(&mut self, s: &str) {
        for ch in s.chars() {
            if self.silent {
                if ch.to_string() == MARK {
                    self.silent = false;
                }
            } else if ch.to_string() == VALUE_MARK {
                self.value_start = Some(self.output.len());
            } else {
                self.output.push(ch);
            }
        }
    }

    fn input(&mut self) -> Result<Option<String>, String> {
        if let Some(line) = self.replay.pop_front() {
            return Ok(Some(line));
        }
        let mut line = String::new();
        match self.live.read_line(&mut line) {
            Ok(0) => Ok(None),
            Ok(_) => {
                while line.ends_with('\n') || line.ends_with('\r') {
                    line.pop();
                }
                self.fresh_input.push(line.clone());
                Ok(Some(line))
            }
            Err(e) => {
                self.read_error = Some(e.to_string());
                Err(e.to_string())
            }
        }
    }
}

/// The accepted cells and the input they consumed.
#[derive(Debug, Default)]
pub struct Session {
    cells: Vec<String>,
    inputs: Vec<String>,
}

/// What evaluating one cell produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Eval {
    /// The cell ran; this is what it printed.
    Ok(String),
    /// Compile errors, already rendered.
    Rejected(String),
    /// A run-time error (the cell is discarded).
    Trapped { output: String, error: String },
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    pub fn cells(&self) -> &[String] {
        &self.cells
    }

    pub fn reset(&mut self) {
        *self = Session::default();
    }

    /// Evaluates `cell` after the session's history; keeps it if it succeeds.
    pub fn eval(&mut self, cell: &str, vfs: &dyn Vfs, cfg: &ReplConfig, live: &mut dyn BufRead) -> Eval {
        let expr = is_expression(cell);
        // evaluate first, then mark where the value text begins
        let text = if expr {
            format!("{{ let value = {}; print(\"{VALUE_MARK}\"); println(value); }}", cell.trim())
        } else {
            cell.to_string()
        };
        let mut source = String::new();
        for c in &self.cells {
            source.push_str(c);
            source.push('\n');
        }
        source.push_str(&format!("print(\"{MARK}\");\n"));
        source.push_str(&text);
        source.push('\n');

        let compiled = compile_source("<repl>", &source, vfs, &cfg.options);
        let Some(bc) = compiled.bytecode.as_ref().filter(|_| !compiled.diags.has_errors()) else {
            return Eval::Rejected(compiled.render_diagnostics());
        };
        let mut io = ReplayIo {
            replay: self.inputs.iter().cloned().collect(),
            live,
            fresh_input: Vec::new(),
            silent: true,
            output: String::new(),
            value_start: None,
            read_error: None,
        };
        let report = vm::run(bc, &mut io, &cfg.vm);
        match report.result {
            Ok(()) => {
                self.cells.push(text);
                self.inputs.extend(io.fresh_input.iter().cloned());
                let mut out = io.output.clone();
                // a unit-valued expression prints `()`; there is nothing worth showing
                if let Some(at) = io.value_start.filter(|&at| expr && out[at..] == *"()\n") {
                    out.truncate(at);
                }
                Eval::Ok(out)
            }
            Err(e) => {
                // the failed cell may have consumed input; keep it so a retry does not ask twice
                self.inputs.extend(io.fresh_input);
                Eval::Trapped { output: io.output, error: e.to_string() }
            }
        }
    }
}

/// Runs the loop until end of input or `:quit`.
pub fn run<R: BufRead, W: Write>(input: &mut R, out: &mut W, vfs: &dyn Vfs, cfg: &ReplConfig) -> io::Result<()> {
    let mut session = Session::new();
    if cfg.prompt {
        writeln!(out, "Neon {} — :help for commands, :quit to leave", env!("CARGO_PKG_VERSION"))?;
    }
    loop {
        let mut cell = String::new();
        let mut first = true;
        loop {
            if cfg.prompt {
                write!(out, "{}", if first { ">>> " } else { "... " })?;
                out.flush()?;
            }
            let mut line = String::new();
            if input.read_line(&mut line)? == 0 {
                if cell.trim().is_empty() {
                    return Ok(());
                }
                break; // evaluate what we have
            }
            first = false;
            cell.push_str(&line);
            if !needs_more(&cell) {
                break;
            }
        }
        let trimmed = cell.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(cmd) = trimmed.strip_prefix(':') {
            let (name, rest) = cmd.split_once(char::is_whitespace).unwrap_or((cmd, ""));
            match name {
                "q" | "quit" | "exit" => return Ok(()),
                "help" | "h" => write!(out, "{HELP}")?,
                "reset" => {
                    session.reset();
                    writeln!(out, "session cleared")?;
                }
                "history" => {
                    for (i, c) in session.cells().iter().enumerate() {
                        writeln!(out, "[{}] {}", i + 1, c.trim_end())?;
                    }
                }
                "load" => match vfs.read_to_string(rest.trim()) {
                    Ok(text) => report(&mut session, &text, vfs, cfg, input, out)?,
                    Err(e) => writeln!(out, "cannot read `{}`: {e}", rest.trim())?,
                },
                other => writeln!(out, "unknown command `:{other}` (try :help)")?,
            }
            continue;
        }
        report(&mut session, trimmed, vfs, cfg, input, out)?;
    }
}

fn report<R: BufRead, W: Write>(
    session: &mut Session,
    cell: &str,
    vfs: &dyn Vfs,
    cfg: &ReplConfig,
    input: &mut R,
    out: &mut W,
) -> io::Result<()> {
    match session.eval(cell, vfs, cfg, input) {
        Eval::Ok(text) => write!(out, "{text}"),
        Eval::Rejected(diags) => write!(out, "{diags}"),
        Eval::Trapped { output, error } => {
            write!(out, "{output}")?;
            writeln!(out, "{error}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemVfs;

    fn session(inputs: &[&str]) -> String {
        let text = inputs.join("\n") + "\n";
        let mut out = Vec::new();
        run(&mut io::Cursor::new(text.into_bytes()), &mut out, &MemVfs::new(), &ReplConfig::default()).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn bracket_balance_decides_continuation() {
        assert!(needs_more("fn f() {"));
        assert!(needs_more("let a = [1,"));
        assert!(!needs_more("fn f() { 1 }"));
        assert!(!needs_more("println(\"}\");"), "brackets inside strings do not count");
        assert!(needs_more("let s = \"abc"));
        assert!(!needs_more("// {\nlet a = 1;"), "nor do those in comments");
        assert!(needs_more("/* open"));
        assert!(!needs_more("let c = '{';"));
        assert!(!needs_more("let q = '\\'';"));
    }

    #[test]
    fn expressions_print_and_state_persists() {
        let out = session(&[
            "let x = 20;",
            "x + 1",
            "fn double(n: i64) -> i64 { n * 2 }",
            "double(x)",
            "var y = 1;",
            "y += 5;",
            "y",
        ]);
        assert_eq!(out, "21\n40\n6\n");
    }

    #[test]
    fn side_effects_are_not_repeated_by_replay() {
        let out = session(&["println(\"once\");", "let a = 1;", "println(a);", "a + 1"]);
        assert_eq!(out, "once\n1\n2\n");
    }

    #[test]
    fn closures_keep_their_state_across_cells() {
        let out = session(&[
            "fn counter() -> fn() -> i64 { var n = 0; || => { n += 1; n } }",
            "let c = counter();",
            "c()",
            "c()",
            "c()",
        ]);
        assert_eq!(out, "1\n2\n3\n");
    }

    #[test]
    fn unit_expressions_print_nothing() {
        let out = session(&["fn hello() { println(\"hi\"); }", "hello()", "1"]);
        assert_eq!(out, "hi\n1\n");
    }

    #[test]
    fn multi_line_cells_are_gathered() {
        let out =
            session(&["fn fact(n: u64) -> u64 {", "    if n <= 1 { 1 } else { n * fact(n - 1) }", "}", "fact(10)"]);
        assert_eq!(out, "3628800\n");
    }

    #[test]
    fn errors_reject_the_cell_and_keep_the_session() {
        let out = session(&["let x = 1;", "let y: i64 = \"no\";", "x + y", "x + 1"]);
        assert!(out.contains("error"), "{out}");
        assert!(out.ends_with("2\n"), "{out}");
        let out = session(&["let z = 0;", "10 / z", "z + 7"]);
        assert!(out.contains("division by zero"), "{out}");
        assert!(out.ends_with("7\n"), "{out}");
    }

    #[test]
    fn input_is_recorded_and_replayed() {
        // the program's input() reads the lines that follow its cell
        let out = session(&[
            "let a = input();",
            "Ada",
            "println(\"hello \" + a);",
            "let b = input();",
            "Bob",
            "println(a + b);",
        ]);
        assert_eq!(out, "hello Ada\nAdaBob\n");
    }

    #[test]
    fn commands() {
        let out = session(&[":help", "let a = 5;", ":history", ":reset", ":history", "a", ":nope", ":q", "1"]);
        assert!(out.contains(":quit") && out.contains("[1] let a = 5;"), "{out}");
        assert!(out.contains("session cleared") && out.contains("unknown command `:nope`"), "{out}");
        assert!(out.contains("error"), "`a` is gone after :reset: {out}");
        assert!(!out.ends_with("1\n"), "nothing runs after :quit");
    }

    #[test]
    fn load_reads_through_the_vfs() {
        let vfs = MemVfs::new().with_file("lib.neon", "fn triple(n: i64) -> i64 { n * 3 }\nlet base = 14;");
        let mut out = Vec::new();
        let text = ":load lib.neon\ntriple(base)\n:load missing.neon\n";
        run(&mut io::Cursor::new(text.as_bytes().to_vec()), &mut out, &vfs, &ReplConfig::default()).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with("42\n") && out.contains("cannot read `missing.neon`"), "{out}");
    }

    #[test]
    fn prompts_appear_only_in_prompt_mode() {
        let mut out = Vec::new();
        let cfg = ReplConfig { prompt: true, ..ReplConfig::default() };
        run(&mut io::Cursor::new(b"1 + 1\nfn f() {\n}\n".to_vec()), &mut out, &MemVfs::new(), &cfg).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains(">>> ") && out.contains("... ") && out.contains("2\n"), "{out}");
    }

    #[test]
    fn end_of_input_evaluates_a_pending_cell() {
        let out = session(&["println(\"tail\");"]);
        assert_eq!(out, "tail\n");
    }
}
