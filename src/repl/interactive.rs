//! The interactive prompt: a colored, editable input line with history, completion and a gray live
//! preview of the value of what is being typed.
//!
//! Drawing is the simple kind: after every key the whole input is redrawn from its first row. That is
//! cheap for what people type at a prompt and avoids tracking what changed; [`frame`] is the pure
//! function that computes the bytes (and where the cursor ends up), so it can be tested.

use std::io::{self, BufRead, Write};

use super::complete::{self, Candidate};
use super::editor::{Editor, Event, Key, Mode};
use super::highlight::{self, Class, Names, Theme};
use super::terminal::{self, char_width, Decoder, RawMode};
use super::{dispatch, history, Flow, ReplConfig, Session};
use crate::vfs::Vfs;

const PROMPT_WIDTH: usize = 4;
const MAX_MENU_ROWS: usize = 5;
const MAX_PREVIEW_LINES: usize = 3;

// ---- drawing -----------------------------------------------------------------------------------------

/// Everything one redraw needs.
#[derive(Debug)]
pub struct View<'a> {
    pub text: &'a str,
    /// The cursor, as a character index into `text`.
    pub cursor: usize,
    /// The first line's prompt (already styled) and its width on screen.
    pub prompt: &'a str,
    pub prompt_width: usize,
    /// The prompt of the other lines (styled).
    pub continuation: &'a str,
    /// Hint lines below the input, styled and short enough not to wrap.
    pub hints: &'a [String],
    /// Terminal width in columns.
    pub width: usize,
}

/// What a redraw produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub bytes: String,
    /// The row (from the first row of the input) the cursor is left on.
    pub cursor_row: usize,
    /// The last row drawn.
    pub end_row: usize,
}

fn text_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// The bytes that redraw the input over the previous drawing, whose cursor was left on row
/// `prev_cursor_row` (counted from the input's first row).
pub fn frame(prev_cursor_row: usize, view: &View<'_>, classes: &[Class], theme: &Theme) -> Frame {
    let tw = view.width.max(8);
    let mut out = String::new();
    if prev_cursor_row > 0 {
        out.push_str(&format!("\x1b[{prev_cursor_row}A"));
    }
    out.push_str("\r\x1b[J");

    let styled = highlight::render_lines(view.text, classes, theme);
    let plain: Vec<&str> = view.text.split('\n').collect();
    let mut line_row: Vec<usize> = Vec::new();
    let mut line_prompt: Vec<usize> = Vec::new();
    let mut row = 0usize;
    let mut fresh = false; // the cursor is at column 0 of a row nothing was written to
    let mut end_row = 0usize;
    for (i, (s, p)) in styled.iter().zip(&plain).enumerate() {
        if i > 0 && !fresh {
            out.push_str("\r\n");
        }
        fresh = false;
        let (prompt, pw) = if i == 0 { (view.prompt, view.prompt_width) } else { (view.continuation, PROMPT_WIDTH) };
        out.push_str(prompt);
        out.push_str(s);
        let w = pw + text_width(p);
        if w > 0 && w.is_multiple_of(tw) {
            // the line fills the row exactly: force the wrap so the cursor row is well defined
            out.push_str(" \r");
            fresh = true;
        }
        line_row.push(row);
        line_prompt.push(pw);
        end_row = row + w / tw;
        row += w / tw + 1;
    }
    for h in view.hints {
        if !fresh {
            out.push_str("\r\n");
        }
        fresh = false;
        out.push_str(h);
        end_row += 1;
    }

    // where the cursor belongs
    let before: String = view.text.chars().take(view.cursor).collect();
    let cl = before.matches('\n').count();
    let in_line = before.rsplit('\n').next().unwrap_or("");
    let cells = line_prompt[cl.min(line_prompt.len() - 1)] + text_width(in_line);
    let target_row = line_row[cl.min(line_row.len() - 1)] + cells / tw;
    let target_col = cells % tw;
    if end_row > target_row {
        out.push_str(&format!("\x1b[{}A", end_row - target_row));
    }
    out.push('\r');
    if target_col > 0 {
        out.push_str(&format!("\x1b[{target_col}C"));
    }
    Frame { bytes: out, cursor_row: target_row, end_row }
}

/// `s` cut to `max` columns, with an ellipsis if something was lost.
fn fit(s: &str, max: usize) -> String {
    if text_width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// The completion menu as hint lines.
fn menu_lines(items: &[Candidate], selected: Option<usize>, tw: usize, theme: &Theme) -> Vec<String> {
    let widest = items.iter().map(|c| text_width(&c.label)).max().unwrap_or(1);
    let col_w = (widest + 2).min(tw.saturating_sub(1)).max(2);
    let cols = ((tw.saturating_sub(1)) / col_w).max(1);
    let rows_total = items.len().div_ceil(cols);
    // keep the selected row in view
    let sel_row = selected.map_or(0, |s| s / cols);
    let first_row = if sel_row >= MAX_MENU_ROWS { sel_row + 1 - MAX_MENU_ROWS } else { 0 };
    let (dim, reset) = (theme.dim(), theme.reset());
    let mut lines = Vec::new();
    for r in first_row..rows_total.min(first_row + MAX_MENU_ROWS) {
        let mut line = String::new();
        for c in 0..cols {
            let Some(item) = items.get(r * cols + c) else { break };
            let label = fit(&item.label, col_w - 1);
            let pad = " ".repeat(col_w - text_width(&label));
            if Some(r * cols + c) == selected && theme.enabled() {
                line.push_str(&format!("\x1b[0m\x1b[7m{label}{reset}{dim}{pad}"));
            } else {
                line.push_str(&format!("{dim}{label}{pad}"));
            }
        }
        line.push_str(reset);
        if !theme.enabled() {
            line = line.trim_end().to_string();
        }
        lines.push(line);
    }
    if rows_total > first_row + MAX_MENU_ROWS {
        lines.push(format!("{dim}… {} more{reset}", items.len() - (first_row + MAX_MENU_ROWS) * cols));
    }
    if let Some(item) = selected.and_then(|s| items.get(s)).filter(|c| !c.detail.is_empty()) {
        lines.push(format!("{dim}{}{reset}", fit(&item.detail, tw - 1)));
    }
    lines
}

/// The preview as hint lines.
fn preview_lines(value: &str, tw: usize, theme: &Theme) -> Vec<String> {
    let all: Vec<&str> = value.lines().collect();
    let mut lines: Vec<String> = Vec::new();
    for (i, l) in all.iter().take(MAX_PREVIEW_LINES).enumerate() {
        let lead = if i == 0 { "↳ " } else { "  " };
        lines.push(format!("{}{}{}", theme.dim(), fit(&format!("{lead}{l}"), tw - 1), theme.reset()));
    }
    if all.len() > MAX_PREVIEW_LINES {
        lines.push(format!("{}  … {} more lines{}", theme.dim(), all.len() - MAX_PREVIEW_LINES, theme.reset()));
    }
    lines
}

// ---- the loop ----------------------------------------------------------------------------------------

/// Completion menu state while the user cycles through candidates with Tab.
struct Menu {
    start: usize,
    items: Vec<Candidate>,
    selected: Option<usize>,
}

struct Prompt<'a> {
    editor: Editor,
    session: Session,
    names: Names,
    theme: Theme,
    width: usize,
    menu: Option<Menu>,
    /// The text the preview was computed for, and the value it printed.
    preview: Option<(String, Option<String>)>,
    /// Ticks since the last edit (the preview waits for a pause).
    idle: u32,
    last_cursor_row: usize,
    last_end_row: usize,
    vfs: &'a dyn Vfs,
    cfg: &'a ReplConfig,
    history_path: Option<std::path::PathBuf>,
    mode_shown: Mode,
}

impl Prompt<'_> {
    fn hints(&self) -> Vec<String> {
        let tw = self.width;
        if let Some(m) = &self.menu {
            return menu_lines(&m.items, m.selected, tw, &self.theme);
        }
        if self.editor.search.is_some() {
            return Vec::new();
        }
        match &self.preview {
            Some((text, Some(value))) if *text == self.editor.text() => preview_lines(value, tw, &self.theme),
            _ => Vec::new(),
        }
    }

    fn prompt_strings(&self) -> (String, usize, String) {
        let t = &self.theme;
        let style = |code: &str, text: &str| {
            if t.enabled() {
                format!("{code}{text}{}", t.reset())
            } else {
                text.to_string()
            }
        };
        let continuation = style("\x1b[38;5;240m", "... ");
        if let Some(s) = &self.editor.search {
            let label = format!("(search `{}`) ", s.query);
            let w = text_width(&label);
            return (style("\x1b[38;5;214m", &label), w, continuation);
        }
        match self.editor.mode {
            Mode::Insert => (style("\x1b[1;32m", ">>> "), PROMPT_WIDTH, continuation),
            Mode::Normal => (style("\x1b[1;33m", "[n] "), PROMPT_WIDTH, continuation),
        }
    }

    /// Redraws the input and leaves the cursor where it belongs.
    fn draw<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        let hits = self.editor.search_hit().map(str::to_string);
        let text = match (&self.editor.search, &hits) {
            (Some(_), Some(h)) => h.clone(),
            (Some(_), None) => String::new(),
            _ => self.editor.text(),
        };
        let cursor = if self.editor.search.is_some() { text.chars().count() } else { self.editor.cursor() };
        let classes = highlight::highlight(&text, &self.names);
        let (prompt, pw, continuation) = self.prompt_strings();
        let hints = self.hints();
        let view = View {
            text: &text,
            cursor,
            prompt: &prompt,
            prompt_width: pw,
            continuation: &continuation,
            hints: &hints,
            width: self.width,
        };
        let f = frame(self.last_cursor_row, &view, &classes, &self.theme);
        let mut bytes = String::new();
        // block cursor in normal mode, bar in insert mode
        if self.mode_shown != self.editor.mode {
            bytes.push_str(if self.editor.mode == Mode::Normal { "\x1b[2 q" } else { "\x1b[6 q" });
            self.mode_shown = self.editor.mode;
        }
        bytes.push_str(&f.bytes);
        out.write_all(bytes.as_bytes())?;
        out.flush()?;
        self.last_cursor_row = f.cursor_row;
        self.last_end_row = f.end_row;
        Ok(())
    }

    /// Leaves the finished input on screen (without hints) and starts a fresh line below it.
    fn finish_line<W: Write>(&mut self, out: &mut W, suffix: &str) -> io::Result<()> {
        self.menu = None;
        self.preview = None;
        self.draw(out)?;
        let down = self.last_end_row - self.last_cursor_row;
        if down > 0 {
            write!(out, "\x1b[{down}B")?;
        }
        write!(out, "\r\n{suffix}")?;
        out.flush()?;
        self.last_cursor_row = 0;
        self.last_end_row = 0;
        Ok(())
    }

    fn complete<W: Write>(&mut self, backward: bool, out: &mut W) -> io::Result<()> {
        let _ = out;
        if let Some(m) = &mut self.menu {
            // cycle through the candidates
            let n = m.items.len();
            let next = match (m.selected, backward) {
                (None, false) => 0,
                (None, true) => n - 1,
                (Some(i), false) => (i + 1) % n,
                (Some(i), true) => (i + n - 1) % n,
            };
            m.selected = Some(next);
            let (start, label) = (m.start, m.items[next].label.clone());
            self.editor.replace_before_cursor(start, &label);
            return Ok(());
        }
        let text = self.editor.text();
        let Some(c) = complete::complete(self.session.cells(), &text, self.editor.cursor(), self.vfs) else {
            return Ok(());
        };
        let typed: String = self.editor.chars()[c.start..self.editor.cursor()].iter().collect();
        if c.items.len() == 1 {
            self.editor.replace_before_cursor(c.start, &c.items[0].label);
            return Ok(());
        }
        let common = complete::common_prefix(&c.items);
        let grew = common.chars().count() > typed.chars().count();
        if grew {
            self.editor.replace_before_cursor(c.start, &common);
        }
        // the first Tab that cannot extend the word shows the menu with the first entry chosen
        let selected = if grew { None } else { Some(0) };
        if let Some(i) = selected {
            let label = c.items[i].label.clone();
            self.editor.replace_before_cursor(c.start, &label);
        }
        self.menu = Some(Menu { start: c.start, items: c.items, selected });
        Ok(())
    }

    fn refresh_names(&mut self) {
        self.names = Names::from_cells(self.session.cells());
    }

    fn maybe_preview(&mut self) {
        let text = self.editor.text();
        if text.trim().is_empty() || self.editor.search.is_some() || self.menu.is_some() {
            return;
        }
        if self.preview.as_ref().is_some_and(|(t, _)| *t == text) {
            return;
        }
        let value =
            if text.trim_start().starts_with(':') { None } else { self.session.preview(&text, self.vfs, self.cfg) };
        self.preview = Some((text, value));
    }
}

/// Whether the prompt should be the line editor: standard output is a terminal that understands escape
/// sequences, and `NEON_REPL=plain` has not asked for the simple loop.
pub fn wanted() -> bool {
    use std::io::IsTerminal;
    io::stdout().is_terminal()
        && std::env::var("TERM").is_ok_and(|t| !t.is_empty() && t != "dumb")
        && std::env::var("NEON_REPL").map_or(true, |v| v != "plain")
}

/// Runs the prompt until `:quit`, `Ctrl-D` or the end of input. `Unsupported` means the terminal could
/// not be switched to raw mode (nothing was drawn): use the plain loop instead.
pub fn run<R: BufRead, W: Write>(input: &mut R, out: &mut W, vfs: &dyn Vfs, cfg: &ReplConfig) -> io::Result<()> {
    let raw = RawMode::enter().map_err(|e| io::Error::new(io::ErrorKind::Unsupported, e.to_string()))?;
    let history_path = history::default_path();
    let entries = history_path.as_ref().map(history::load).unwrap_or_default();
    let (_, width) = terminal::size();
    let mut p = Prompt {
        editor: Editor::new(entries),
        session: Session::new(),
        names: Names::default(),
        theme: Theme::detect(),
        width,
        menu: None,
        preview: None,
        idle: 0,
        last_cursor_row: 0,
        last_end_row: 0,
        vfs,
        cfg,
        history_path,
        mode_shown: Mode::Insert,
    };
    // bracketed paste on, bar cursor
    write!(out, "\x1b[?2004h\x1b[6 q")?;
    writeln!(out, "Neon {} — :help for help, Ctrl-D to leave\r", env!("CARGO_PKG_VERSION"))?;
    let result = event_loop(&mut p, &raw, input, out);
    // put the terminal back whatever happened
    let _ = write!(out, "\x1b[?2004l\x1b[0 q");
    let _ = out.flush();
    drop(raw);
    result
}

fn event_loop<R: BufRead, W: Write>(p: &mut Prompt<'_>, raw: &RawMode, input: &mut R, out: &mut W) -> io::Result<()> {
    let mut decoder = Decoder::new();
    let mut ticks = 0u32;
    p.draw(out)?;
    loop {
        let key = decoder.key(input)?;
        if key == Key::Tick {
            ticks += 1;
            p.idle += 1;
            let mut dirty = false;
            if ticks.is_multiple_of(10) {
                let (_, w) = terminal::size();
                if w != p.width {
                    p.width = w;
                    // the old drawing may have wrapped differently: start over on a clean screen
                    write!(out, "\x1b[H\x1b[2J")?;
                    p.last_cursor_row = 0;
                    dirty = true;
                }
            }
            if p.idle == 2 {
                let before = p.preview.clone();
                p.maybe_preview();
                dirty |= before != p.preview;
            }
            if dirty {
                p.draw(out)?;
            }
            continue;
        }
        p.idle = 0;
        // Esc only closes an open completion menu
        if key == Key::Esc && p.menu.is_some() {
            p.menu = None;
            p.draw(out)?;
            continue;
        }
        let was_menu = p.menu.is_some();
        let before = p.editor.text();
        let event = p.editor.handle(key.clone());
        match event {
            Event::Complete(back) => {
                p.complete(back, out)?;
            }
            _ => {
                // any other key closes the menu (Esc, Enter and the rest still do their job)
                if was_menu {
                    p.menu = None;
                }
            }
        }
        match event {
            Event::None | Event::Complete(_) => {}
            Event::ClearScreen => {
                write!(out, "\x1b[H\x1b[2J")?;
                p.last_cursor_row = 0;
            }
            Event::Interrupt => {
                p.finish_line_with(&before, out)?;
                write!(out, "{}(interrupted){}\r\n", p.theme.dim(), p.theme.reset())?;
            }
            Event::Eof => {
                p.finish_line(out, "")?;
                return Ok(());
            }
            Event::Submit => {
                let text = p.editor.take();
                p.finish_line_with(&text, out)?;
                if let Some(path) = &p.history_path {
                    if !text.trim().is_empty() {
                        history::append(path, &text);
                    }
                }
                // the program runs with an ordinary terminal: echo, line input, Ctrl-C
                raw.suspend();
                let flow = dispatch(&mut p.session, &text, p.vfs, p.cfg, input, out);
                raw.resume();
                if flow? == Flow::Quit {
                    return Ok(());
                }
                p.refresh_names();
                p.mode_shown = Mode::Insert;
                write!(out, "\x1b[6 q")?;
            }
        }
        p.draw(out)?;
    }
}

impl Prompt<'_> {
    /// Redraws the submitted text once more (so it stays on screen) and moves below it.
    fn finish_line_with<W: Write>(&mut self, text: &str, out: &mut W) -> io::Result<()> {
        // the editor has been emptied by `take`: draw what was submitted
        self.menu = None;
        self.preview = None;
        let classes = highlight::highlight(text, &self.names);
        let (prompt, pw, continuation) = (
            if self.theme.enabled() { format!("\x1b[1;32m>>> {}", self.theme.reset()) } else { ">>> ".to_string() },
            PROMPT_WIDTH,
            if self.theme.enabled() { format!("\x1b[38;5;240m... {}", self.theme.reset()) } else { "... ".to_string() },
        );
        let view = View {
            text,
            cursor: text.chars().count(),
            prompt: &prompt,
            prompt_width: pw,
            continuation: &continuation,
            hints: &[],
            width: self.width,
        };
        let f = frame(self.last_cursor_row, &view, &classes, &self.theme);
        out.write_all(f.bytes.as_bytes())?;
        write!(out, "\r\n")?;
        out.flush()?;
        self.last_cursor_row = 0;
        self.last_end_row = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_view<'a>(text: &'a str, cursor: usize, width: usize, hints: &'a [String]) -> View<'a> {
        View { text, cursor, prompt: ">>> ", prompt_width: 4, continuation: "... ", hints, width }
    }

    fn draw(text: &str, cursor: usize, width: usize, hints: &[String]) -> Frame {
        let cl = highlight::highlight(text, &Names::default());
        frame(0, &plain_view(text, cursor, width, hints), &cl, &Theme::plain())
    }

    #[test]
    fn a_short_line_stays_on_one_row() {
        let f = draw("1 + 2", 5, 80, &[]);
        assert_eq!((f.cursor_row, f.end_row), (0, 0));
        assert!(f.bytes.starts_with("\r\x1b[J>>> 1 + 2"), "{:?}", f.bytes);
        assert!(f.bytes.ends_with("\r\x1b[9C"), "cursor after the prompt (4) and the text (5): {:?}", f.bytes);
    }

    #[test]
    fn the_cursor_can_sit_in_the_middle_of_the_text() {
        let f = draw("abcdef", 2, 80, &[]);
        assert!(f.bytes.ends_with("\r\x1b[6C"), "{:?}", f.bytes);
    }

    #[test]
    fn long_lines_wrap_and_the_cursor_row_follows() {
        // 4 + 20 characters in a 10-column terminal: rows of 10, 10, 4
        let text = "a".repeat(20);
        let f = draw(&text, 20, 10, &[]);
        assert_eq!((f.cursor_row, f.end_row), (2, 2));
        let f = draw(&text, 8, 10, &[]);
        assert_eq!((f.cursor_row, f.end_row), (1, 2), "cursor at column 12 of the line: row 1");
        assert!(f.bytes.contains("\x1b[1A"), "back up from the last row: {:?}", f.bytes);
    }

    #[test]
    fn a_line_that_fills_the_row_exactly_gets_a_row_of_its_own_for_the_cursor() {
        let text = "a".repeat(6); // 4 + 6 = 10 columns
        let f = draw(&text, 6, 10, &[]);
        assert_eq!((f.cursor_row, f.end_row), (1, 1));
        assert!(f.bytes.contains(" \r"), "{:?}", f.bytes);
    }

    #[test]
    fn several_lines_and_hints_add_rows_and_the_cursor_returns_to_its_line() {
        let hints = ["↳ 3".to_string()];
        let f = draw("fn f() {\n    1\n}", 4, 80, &hints);
        assert_eq!((f.cursor_row, f.end_row), (0, 3), "three lines and a hint");
        assert!(f.bytes.contains("... "), "{:?}", f.bytes);
        assert!(f.bytes.contains("\x1b[3A"), "{:?}", f.bytes);
        let f = draw("fn f() {\n    1\n}", 15, 80, &hints);
        assert_eq!(f.cursor_row, 2);
    }

    #[test]
    fn redrawing_goes_back_up_to_where_the_input_began() {
        let cl = highlight::highlight("ab", &Names::default());
        let f = frame(3, &plain_view("ab", 2, 80, &[]), &cl, &Theme::plain());
        assert!(f.bytes.starts_with("\x1b[3A\r\x1b[J"), "{:?}", f.bytes);
    }

    #[test]
    fn colors_appear_only_with_a_color_theme() {
        let cl = highlight::highlight("let x = 1;", &Names::default());
        let colored = frame(0, &plain_view("let x = 1;", 10, 80, &[]), &cl, &Theme::new(true));
        assert!(colored.bytes.contains("\x1b[38;2;"), "{:?}", colored.bytes);
        let plain = frame(0, &plain_view("let x = 1;", 10, 80, &[]), &cl, &Theme::plain());
        assert!(!plain.bytes.contains("\x1b[3"), "{:?}", plain.bytes);
    }

    #[test]
    fn the_completion_menu_is_a_grid_with_the_selection_marked() {
        let items: Vec<Candidate> = ["print", "println", "panic", "pop"]
            .iter()
            .map(|l| Candidate { label: l.to_string(), detail: String::new() })
            .collect();
        let lines = menu_lines(&items, Some(1), 40, &Theme::plain());
        assert_eq!(lines, ["print    println  panic    pop"], "columns are as wide as the longest label plus two");
        let lines = menu_lines(&items, None, 24, &Theme::plain());
        assert_eq!(lines.len(), 2, "{lines:?}");
        let many: Vec<Candidate> =
            (0..60).map(|i| Candidate { label: format!("item{i:02}"), detail: String::new() }).collect();
        let lines = menu_lines(&many, None, 20, &Theme::plain());
        assert!(lines.last().unwrap().contains("more"), "{lines:?}");
        assert_eq!(lines.len(), MAX_MENU_ROWS + 1);
    }

    #[test]
    fn previews_are_truncated_to_the_width() {
        let lines = preview_lines("123456789012345678901234567890", 20, &Theme::plain());
        assert_eq!(lines, ["↳ 1234567890123456…"]);
        let lines = preview_lines("a\nb\nc\nd\ne", 40, &Theme::plain());
        assert_eq!(lines, ["↳ a", "  b", "  c", "  … 2 more lines"]);
    }
}
