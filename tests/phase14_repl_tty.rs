//! The interactive prompt in a real terminal: `tmux` provides the terminal and shows what is on its
//! screen. Skipped when tmux is not installed.

use std::process::Command;
use std::time::{Duration, Instant};

struct Pane {
    name: String,
}

fn tmux(args: &[&str]) -> Option<String> {
    let out = Command::new("tmux").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Pane {
    fn start(tag: &str, history: &str) -> Option<Pane> {
        tmux(&["-V"])?;
        let name = format!("neon-{tag}-{}", std::process::id());
        let cmd = format!(
            "env NEON_HISTORY={history} COLORTERM=truecolor TERM=xterm-256color {} repl; echo EXIT_CODE=$?; sleep 30",
            env!("CARGO_BIN_EXE_neon")
        );
        tmux(&["new-session", "-d", "-s", &name, "-x", "100", "-y", "30", &cmd])?;
        let pane = Pane { name };
        pane.wait_for("Neon 0.1.0");
        Some(pane)
    }

    fn screen(&self) -> String {
        tmux(&["capture-pane", "-t", &self.name, "-p"]).unwrap_or_default()
    }

    /// Types text literally (a trailing `;` would be taken by tmux as a command separator, so
    /// the texts here do not end with one).
    fn type_text(&self, text: &str) {
        tmux(&["send-keys", "-t", &self.name, "-l", text]).expect("send-keys");
    }

    fn key(&self, key: &str) {
        tmux(&["send-keys", "-t", &self.name, key]).expect("send-keys");
    }

    /// Waits until the screen contains `text`; panics with the screen if it does not.
    fn wait_for(&self, text: &str) -> String {
        let end = Instant::now() + Duration::from_secs(8);
        loop {
            let s = self.screen();
            if s.contains(text) {
                return s;
            }
            assert!(Instant::now() < end, "waiting for {text:?}; the screen shows:\n{s}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Pane {
    /// Waits until `text` is on the screen at least `n` times.
    fn wait_for_count(&self, text: &str, n: usize) -> String {
        let end = Instant::now() + Duration::from_secs(8);
        loop {
            let s = self.screen();
            if s.matches(text).count() >= n {
                return s;
            }
            assert!(Instant::now() < end, "waiting for {n}x {text:?}; the screen shows:\n{s}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = tmux(&["kill-session", "-t", &self.name]);
    }
}

#[test]
fn the_prompt_edits_previews_completes_and_remembers() {
    let history = std::env::temp_dir().join(format!("neon-tty-history-{}", std::process::id()));
    let _ = std::fs::remove_file(&history);
    let Some(p) = Pane::start("edit", history.to_str().unwrap()) else {
        eprintln!("no tmux: skipping");
        return;
    };

    p.type_text("let xs = [3, 1, 2]; ");
    p.key("Enter");
    p.type_text("fn foo() -> i64 { 42 }");
    p.key("Enter");

    // the gray preview appears under a complete expression, before Enter
    p.type_text("foo()");
    p.wait_for("↳ 42");
    p.key("C-u");

    // Tab completes members by the type of the receiver and cycles through the choices
    p.type_text("xs.ma");
    p.key("Tab");
    let s = p.wait_for("map");
    assert!(s.contains("max"), "{s}");
    p.key("Tab");
    p.wait_for(">>> xs.max");
    p.key("Escape");
    p.key("C-u");

    // Enter evaluates; Up brings the line back; the arrows move the cursor
    p.type_text("xs.map(|x| => x * 2)");
    p.key("Enter");
    p.wait_for("[6, 2, 4]");
    p.key("Up");
    p.wait_for(">>> xs.map(|x| => x * 2)");
    p.key("Left");
    p.key("BSpace");
    p.type_text("9");
    p.wait_for("x * 9)");
    p.key("C-u");

    // Esc is vi normal mode: k and j walk the history
    p.key("Escape");
    p.key("k");
    // (the history position is kept where Up left it: the entry before `xs.map`)
    p.wait_for("[n] fn foo() -> i64 { 42 }");
    p.key("j");
    p.wait_for("[n] xs.map(|x| => x * 2)");
    p.key("i");
    p.key("C-u");

    // unfinished input continues with an indented line
    p.type_text("fn twice(n: i64) -> i64 {");
    p.key("Enter");
    p.type_text("n * 2");
    p.key("Enter");
    p.type_text("}");
    p.key("Enter");
    p.type_text("twice(21)");
    p.key("Enter");
    p.wait_for("\n42");

    // an error rejects the cell, the session goes on
    p.type_text("1 / 0");
    p.key("Enter");
    p.wait_for("division by zero");

    p.key("C-d");
    p.wait_for("EXIT_CODE=0");

    // the history file has what was entered
    let saved = std::fs::read_to_string(&history).unwrap_or_default();
    assert!(saved.contains("xs.map(|x| => x * 2)") && saved.contains("fn twice(n: i64) -> i64 {\\n"), "{saved}");
    let _ = std::fs::remove_file(&history);
}

#[test]
fn control_c_drops_the_input_and_a_new_session_remembers_the_history() {
    let history = std::env::temp_dir().join(format!("neon-tty-history2-{}", std::process::id()));
    std::fs::write(&history, "let remembered = 7 \n").unwrap();
    let Some(p) = Pane::start("hist", history.to_str().unwrap()) else {
        eprintln!("no tmux: skipping");
        return;
    };
    p.type_text("garbage");
    p.key("C-c");
    p.wait_for("(interrupted)");
    p.key("Up");
    p.wait_for(">>> let remembered = 7");
    p.key("C-u");
    p.key("C-d");
    p.wait_for("EXIT_CODE=0");
    let _ = std::fs::remove_file(&history);
}

#[test]
fn control_c_stops_a_running_cell_not_the_session_and_errors_are_about_the_input() {
    let Some(p) = Pane::start("intr", "") else {
        eprintln!("no tmux: skipping");
        return;
    };
    p.type_text("fn spin() -> i64 { var i = 0; while true { i += 1; } i } ");
    p.key("Enter");
    p.type_text("let n = 7; ");
    p.key("Enter");
    p.type_text("spin()");
    p.key("Enter");
    std::thread::sleep(Duration::from_millis(300)); // it is running now
    p.key("C-c");
    p.wait_for("interrupted");
    // the session is alive and kept what it had
    p.type_text("n + 1");
    p.wait_for("↳ 8");
    p.key("Enter");

    // a cell waiting for input() is interrupted too, and typing there is echoed and can be corrected
    p.type_text("let w = input(); ");
    p.key("Enter");
    p.type_text("abx");
    p.key("BSpace");
    p.type_text("c");
    p.key("Enter");
    p.wait_for("abc");
    p.type_text("let v = input(); ");
    p.key("Enter");
    std::thread::sleep(Duration::from_millis(300));
    p.key("C-c");
    p.wait_for_count("interrupted", 2);

    // errors point into what was typed, not into the machinery around it
    p.type_text("n + missing");
    p.key("Enter");
    let s = p.wait_for("not found in this scope");
    assert!(s.contains("--> <repl>:1:5") && s.contains("1 | n + missing"), "{s}");
    assert!(!s.contains("let value") && !s.contains("aborting"), "{s}");
    // a forgotten semicolon is not an error
    p.type_text("var k = 5");
    p.key("Enter");
    p.type_text("k * 2");
    p.key("Enter");
    p.wait_for("\n10");
    p.key("C-d");
    p.wait_for("EXIT_CODE=0");
}
