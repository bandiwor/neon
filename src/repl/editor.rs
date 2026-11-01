//! The line editor behind the interactive prompt: a buffer that may span several lines, a cursor, a
//! history, an undo stack, a kill ring, an emacs-style key set and a small vi normal mode.
//!
//! It knows nothing about terminals: keys go in, a buffer and a cursor come out, so all of it can be
//! tested by feeding keys.

use super::needs_more;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    /// Insert a line break even if the input is complete (`Alt-Enter`, `Ctrl-J`).
    NewLine,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    WordLeft,
    WordRight,
    Ctrl(char),
    Esc,
    /// Text pasted in one piece (bracketed paste).
    Paste(String),
    /// Nothing arrived for a moment.
    Tick,
}

/// What the owner of the editor has to do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    None,
    /// The buffer is a complete cell: take it with [`Editor::take`].
    Submit,
    /// `Ctrl-D` on an empty buffer.
    Eof,
    /// `Ctrl-C`: the buffer was thrown away.
    Interrupt,
    /// `Tab`: offer completions for the word before the cursor (`true`: backwards).
    Complete(bool),
    /// `Ctrl-L`.
    ClearScreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Insert,
    Normal,
}

/// An incremental search through the history (`Ctrl-R`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    pub query: String,
    /// Index in the history of the entry shown.
    pub found: Option<usize>,
}

const INDENT: &str = "    ";
const UNDO_LIMIT: usize = 200;

#[derive(Debug, Clone)]
pub struct Editor {
    text: Vec<char>,
    cursor: usize,
    pub mode: Mode,
    history: Vec<String>,
    /// The history entry on show, if the user is browsing.
    browsing: Option<usize>,
    /// What was being typed when browsing started.
    stash: String,
    undo: Vec<(Vec<char>, usize)>,
    kill: Vec<char>,
    /// A vi operator waiting for its motion (`d`, `c`, `r`).
    pending: Option<char>,
    /// The column vertical movement tries to keep.
    want_col: Option<usize>,
    pub search: Option<Search>,
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new(Vec::new())
    }
}

fn is_word(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

impl Editor {
    pub fn new(history: Vec<String>) -> Editor {
        Editor {
            text: Vec::new(),
            cursor: 0,
            mode: Mode::Insert,
            history,
            browsing: None,
            stash: String::new(),
            undo: Vec::new(),
            kill: Vec::new(),
            pending: None,
            want_col: None,
            search: None,
        }
    }

    // ---- state -------------------------------------------------------------------------------

    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    pub fn chars(&self) -> &[char] {
        &self.text
    }

    /// The cursor, as an index into [`chars`](Self::chars).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// Replaces the buffer (the cursor goes to the end).
    pub fn set_text(&mut self, s: &str) {
        self.text = s.chars().collect();
        self.cursor = self.text.len();
    }

    /// Starts a new line of input: returns the buffer and clears it. A non-blank buffer joins the
    /// history (unless it repeats the previous entry).
    pub fn take(&mut self) -> String {
        let s = self.text();
        if !s.trim().is_empty() && self.history.last() != Some(&s) {
            self.history.push(s.clone());
        }
        self.reset();
        s
    }

    fn reset(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.mode = Mode::Insert;
        self.browsing = None;
        self.stash.clear();
        self.undo.clear();
        self.pending = None;
        self.want_col = None;
        self.search = None;
    }

    // ---- lines -------------------------------------------------------------------------------

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].iter().rposition(|&c| c == '\n').map_or(0, |p| p + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..].iter().position(|&c| c == '\n').map_or(self.text.len(), |p| at + p)
    }

    /// `(line, column)` of the cursor.
    pub fn position(&self) -> (usize, usize) {
        let line = self.text[..self.cursor].iter().filter(|&&c| c == '\n').count();
        (line, self.cursor - self.line_start(self.cursor))
    }

    pub fn line_count(&self) -> usize {
        self.text.iter().filter(|&&c| c == '\n').count() + 1
    }

    fn indent_of(&self, at: usize) -> usize {
        let s = self.line_start(at);
        self.text[s..].iter().take_while(|&&c| c == ' ').count()
    }

    // ---- editing primitives -------------------------------------------------------------------

    fn snapshot(&mut self) {
        if self.undo.last().is_some_and(|(t, _)| *t == self.text) {
            return;
        }
        if self.undo.len() >= UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push((self.text.clone(), self.cursor));
    }

    fn insert(&mut self, s: &str) {
        self.snapshot();
        let new: Vec<char> = s.chars().collect();
        let n = new.len();
        self.text.splice(self.cursor..self.cursor, new);
        self.cursor += n;
        self.want_col = None;
    }

    fn remove(&mut self, lo: usize, hi: usize) -> Vec<char> {
        if lo >= hi {
            return Vec::new();
        }
        self.snapshot();
        let gone: Vec<char> = self.text.drain(lo..hi).collect();
        self.cursor = lo.min(self.text.len());
        self.want_col = None;
        gone
    }

    fn word_left(&self, from: usize) -> usize {
        let mut i = from;
        while i > 0 && !is_word(self.text[i - 1]) && self.text[i - 1] != '\n' {
            i -= 1;
        }
        while i > 0 && is_word(self.text[i - 1]) {
            i -= 1;
        }
        i
    }

    fn word_right(&self, from: usize) -> usize {
        let n = self.text.len();
        let mut i = from;
        while i < n && is_word(self.text[i]) {
            i += 1;
        }
        while i < n && !is_word(self.text[i]) && self.text[i] != '\n' {
            i += 1;
        }
        i
    }

    // ---- vertical movement and history -----------------------------------------------------------

    /// Moves the cursor one line up or down inside the buffer; `false` at the first / last line.
    fn move_line(&mut self, up: bool) -> bool {
        let (line, col) = self.position();
        let want = self.want_col.unwrap_or(col);
        let start = self.line_start(self.cursor);
        let target_start = if up {
            if line == 0 {
                return false;
            }
            self.line_start(start - 1)
        } else {
            let end = self.line_end(self.cursor);
            if end >= self.text.len() {
                return false;
            }
            end + 1
        };
        let target_end = self.line_end(target_start);
        self.cursor = (target_start + want).min(target_end);
        self.want_col = Some(want);
        true
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let at = match self.browsing {
            None => {
                self.stash = self.text();
                self.history.len() - 1
            }
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.browsing = Some(at);
        let s = self.history[at].clone();
        self.set_text(&s);
    }

    fn history_next(&mut self) {
        let Some(i) = self.browsing else { return };
        if i + 1 < self.history.len() {
            self.browsing = Some(i + 1);
            let s = self.history[i + 1].clone();
            self.set_text(&s);
        } else {
            self.browsing = None;
            let s = std::mem::take(&mut self.stash);
            self.set_text(&s);
        }
    }

    fn up(&mut self) {
        if !self.move_line(true) {
            self.history_prev();
        }
    }

    fn down(&mut self) {
        if !self.move_line(false) {
            self.history_next();
        }
    }

    // ---- keys ----------------------------------------------------------------------------------------

    /// Applies one key.
    pub fn handle(&mut self, key: Key) -> Event {
        if self.search.is_some() {
            return self.handle_search(key);
        }
        match key {
            Key::Tick => Event::None,
            Key::Ctrl('c') => {
                self.reset();
                Event::Interrupt
            }
            Key::Ctrl('d') => {
                if self.text.is_empty() {
                    Event::Eof
                } else {
                    if self.mode == Mode::Insert {
                        self.delete_forward();
                    }
                    Event::None
                }
            }
            Key::Ctrl('l') => Event::ClearScreen,
            Key::Ctrl('r') => {
                self.search = Some(Search::default());
                Event::None
            }
            Key::Enter => self.enter(),
            _ => match self.mode {
                Mode::Insert => self.insert_mode(key),
                Mode::Normal => self.normal_mode(key),
            },
        }
    }

    fn enter(&mut self) -> Event {
        if self.mode == Mode::Insert {
            let between = matches!(
                (self.cursor.checked_sub(1).map(|i| self.text[i]), self.text.get(self.cursor)),
                (Some('{'), Some('}')) | (Some('('), Some(')')) | (Some('['), Some(']'))
            );
            if between || needs_more(&self.text()) {
                return self.newline();
            }
        }
        Event::Submit
    }

    /// A line break; it keeps the indentation of the line and adds one level after an opening bracket.
    fn newline(&mut self) -> Event {
        let indent = self.indent_of(self.cursor);
        let before = self.text[..self.cursor].iter().rev().find(|c| !c.is_whitespace()).copied();
        let after = self.text.get(self.cursor).copied();
        let mut s = String::from("\n");
        s.push_str(&" ".repeat(indent));
        if matches!(before, Some('{' | '(' | '[')) {
            s.push_str(INDENT);
            // `{|}` -> the closing bracket moves to its own line
            if matches!((before, after), (Some('{'), Some('}')) | (Some('('), Some(')')) | (Some('['), Some(']'))) {
                self.insert(&s);
                let at = self.cursor;
                self.insert(&format!("\n{}", " ".repeat(indent)));
                self.cursor = at;
                return Event::None;
            }
        }
        self.insert(&s);
        Event::None
    }

    fn delete_forward(&mut self) {
        if self.cursor < self.text.len() {
            self.remove(self.cursor, self.cursor + 1);
        }
    }

    fn insert_mode(&mut self, key: Key) -> Event {
        match key {
            Key::Char('}') if self.only_blanks_before_cursor() && self.cursor > 0 => {
                // a closing bracket on its own line steps back one level
                let s = self.line_start(self.cursor);
                let drop = INDENT.len().min(self.cursor - s);
                self.remove(self.cursor - drop, self.cursor);
                self.insert("}");
            }
            Key::Char(c) => self.insert(&c.to_string()),
            Key::Paste(s) => self.insert(&s.replace("\r\n", "\n").replace('\r', "\n")),
            Key::NewLine | Key::Ctrl('j') => {
                return self.newline();
            }
            Key::Tab => {
                // at the start of a line (or after blanks) Tab indents, elsewhere it completes
                if self.only_blanks_before_cursor() {
                    self.insert(INDENT);
                } else {
                    return Event::Complete(false);
                }
            }
            Key::BackTab => return Event::Complete(true),
            Key::Backspace | Key::Ctrl('h') => {
                if self.cursor > 0 {
                    // a whole indentation level at once
                    let s = self.line_start(self.cursor);
                    let col = self.cursor - s;
                    let blanks = self.only_blanks_before_cursor();
                    let n = if blanks && col >= INDENT.len() && col.is_multiple_of(INDENT.len()) {
                        INDENT.len()
                    } else {
                        1
                    };
                    self.remove(self.cursor - n, self.cursor);
                }
            }
            Key::Delete => self.delete_forward(),
            Key::Left | Key::Ctrl('b') => self.cursor = self.cursor.saturating_sub(1),
            Key::Right | Key::Ctrl('f') => self.cursor = (self.cursor + 1).min(self.text.len()),
            Key::Up | Key::Ctrl('p') => self.up(),
            Key::Down | Key::Ctrl('n') => self.down(),
            Key::Home | Key::Ctrl('a') => self.cursor = self.line_start(self.cursor),
            Key::End | Key::Ctrl('e') => self.cursor = self.line_end(self.cursor),
            Key::WordLeft => self.cursor = self.word_left(self.cursor),
            Key::WordRight => self.cursor = self.word_right(self.cursor),
            Key::Ctrl('k') => {
                let end = self.line_end(self.cursor);
                // at the end of a line it joins the next one
                let end = if end == self.cursor && end < self.text.len() { end + 1 } else { end };
                self.kill = self.remove(self.cursor, end);
            }
            Key::Ctrl('u') => {
                let s = self.line_start(self.cursor);
                self.kill = self.remove(s, self.cursor);
            }
            Key::Ctrl('w') => {
                let w = self.word_left(self.cursor);
                self.kill = self.remove(w, self.cursor);
            }
            Key::Ctrl('y') => {
                let k: String = self.kill.iter().collect();
                self.insert(&k);
            }
            Key::Ctrl('t') => {
                // swap the two characters around the cursor
                let n = self.text.len();
                if n >= 2 {
                    let at = if self.cursor == n { n - 1 } else { self.cursor };
                    if at > 0 && self.text[at] != '\n' && self.text[at - 1] != '\n' {
                        self.snapshot();
                        self.text.swap(at - 1, at);
                        self.cursor = at + 1;
                    }
                }
            }
            Key::Ctrl('_') | Key::Ctrl('z') | Key::Ctrl('/') => self.undo_last(),
            Key::Esc => {
                self.mode = Mode::Normal;
                self.pending = None;
                if self.cursor > self.line_start(self.cursor) {
                    self.cursor -= 1;
                }
            }
            _ => {}
        }
        Event::None
    }

    fn only_blanks_before_cursor(&self) -> bool {
        let s = self.line_start(self.cursor);
        self.text[s..self.cursor].iter().all(|&c| c == ' ')
    }

    fn undo_last(&mut self) {
        if let Some((t, c)) = self.undo.pop() {
            self.text = t;
            self.cursor = c.min(self.text.len());
        }
    }

    fn normal_mode(&mut self, key: Key) -> Event {
        // an operator waiting for its motion
        if let Some(op) = self.pending.take() {
            if let Key::Char(c) = key {
                self.operator(op, c);
            }
            return Event::None;
        }
        let last = |s: &Editor| {
            let e = s.line_end(s.cursor);
            if e > s.line_start(s.cursor) {
                e - 1
            } else {
                e
            }
        };
        match key {
            Key::Char('i') => self.mode = Mode::Insert,
            Key::Char('a') => {
                if self.cursor < self.line_end(self.cursor) {
                    self.cursor += 1;
                }
                self.mode = Mode::Insert;
            }
            Key::Char('I') => {
                self.cursor = self.line_start(self.cursor);
                self.mode = Mode::Insert;
            }
            Key::Char('A') => {
                self.cursor = self.line_end(self.cursor);
                self.mode = Mode::Insert;
            }
            Key::Char('o') => {
                self.cursor = self.line_end(self.cursor);
                self.mode = Mode::Insert;
                return self.newline();
            }
            Key::Char('h') | Key::Left | Key::Backspace => {
                if self.cursor > self.line_start(self.cursor) {
                    self.cursor -= 1;
                }
            }
            Key::Char('l') | Key::Right | Key::Char(' ') => {
                if self.cursor < last(self) {
                    self.cursor += 1;
                }
            }
            Key::Char('k') | Key::Up => self.up(),
            Key::Char('j') | Key::Down => self.down(),
            Key::Char('0') | Key::Home => self.cursor = self.line_start(self.cursor),
            Key::Char('^') => {
                let s = self.line_start(self.cursor);
                self.cursor = s + self.indent_of(self.cursor);
            }
            Key::Char('$') | Key::End => self.cursor = last(self),
            Key::Char('w') | Key::WordRight => {
                self.cursor = self.word_right(self.cursor).min(self.text.len().saturating_sub(1))
            }
            Key::Char('b') | Key::WordLeft => self.cursor = self.word_left(self.cursor),
            Key::Char('e') => {
                let n = self.text.len();
                let mut i = (self.cursor + 1).min(n);
                while i < n && !is_word(self.text[i]) {
                    i += 1;
                }
                while i + 1 < n && is_word(self.text[i + 1]) {
                    i += 1;
                }
                self.cursor = i.min(n.saturating_sub(1));
            }
            Key::Char('x') | Key::Delete => {
                if self.cursor < self.line_end(self.cursor) {
                    self.kill = self.remove(self.cursor, self.cursor + 1);
                }
            }
            Key::Char('X') => {
                if self.cursor > self.line_start(self.cursor) {
                    self.kill = self.remove(self.cursor - 1, self.cursor);
                }
            }
            Key::Char('D') => {
                let e = self.line_end(self.cursor);
                self.kill = self.remove(self.cursor, e);
                self.cursor = self.cursor.saturating_sub(1).max(self.line_start(self.cursor.saturating_sub(1)));
            }
            Key::Char('C') => {
                let e = self.line_end(self.cursor);
                self.kill = self.remove(self.cursor, e);
                self.mode = Mode::Insert;
            }
            Key::Char('p') => {
                let at = (self.cursor + 1).min(self.line_end(self.cursor));
                self.cursor = at;
                let k: String = self.kill.iter().collect();
                self.insert(&k);
                self.cursor = self.cursor.saturating_sub(1);
            }
            Key::Char('P') => {
                let k: String = self.kill.iter().collect();
                self.insert(&k);
                self.cursor = self.cursor.saturating_sub(1);
            }
            Key::Char('u') | Key::Ctrl('_') | Key::Ctrl('z') => self.undo_last(),
            Key::Char(c @ ('d' | 'c' | 'r')) => self.pending = Some(c),
            Key::Char('G') => {
                self.cursor = self.text.len().saturating_sub(1);
            }
            Key::Char('g') => self.cursor = 0,
            Key::Paste(s) => {
                self.mode = Mode::Insert;
                self.insert(&s.replace("\r\n", "\n").replace('\r', "\n"));
            }
            Key::Tab => return Event::Complete(false),
            Key::BackTab => return Event::Complete(true),
            Key::Esc => self.pending = None,
            Key::NewLine => return self.newline(),
            _ => {}
        }
        Event::None
    }

    /// `d`, `c` and `r` with the key that follows.
    fn operator(&mut self, op: char, c: char) {
        if op == 'r' {
            if self.cursor < self.line_end(self.cursor) {
                self.snapshot();
                self.text[self.cursor] = c;
            }
            return;
        }
        let (lo, hi) = match c {
            c if c == op => {
                // the whole line (with its break)
                let s = self.line_start(self.cursor);
                let e = self.line_end(self.cursor);
                if s > 0 && e >= self.text.len() {
                    (s - 1, e)
                } else if e < self.text.len() {
                    (s, e + 1)
                } else {
                    (s, e)
                }
            }
            // `cw` changes up to the end of the word, like `ce`, not the blanks after it
            'w' if op == 'c' => {
                let mut e = self.cursor;
                while e < self.text.len() && is_word(self.text[e]) {
                    e += 1;
                }
                (self.cursor, e.max(self.cursor + 1).min(self.text.len()))
            }
            'w' => (self.cursor, self.word_right(self.cursor)),
            'b' => (self.word_left(self.cursor), self.cursor),
            '$' => (self.cursor, self.line_end(self.cursor)),
            '0' | '^' => (self.line_start(self.cursor), self.cursor),
            'l' => (self.cursor, (self.cursor + 1).min(self.line_end(self.cursor))),
            'h' => (self.cursor.saturating_sub(1).max(self.line_start(self.cursor)), self.cursor),
            _ => return,
        };
        self.kill = self.remove(lo, hi);
        if op == 'c' {
            self.mode = Mode::Insert;
        } else {
            let end = self.line_end(self.cursor);
            if self.cursor >= end && self.cursor > self.line_start(self.cursor) {
                self.cursor = end.saturating_sub(1);
            }
        }
    }

    // ---- incremental search -----------------------------------------------------------------------

    fn find_from(&self, query: &str, from: usize) -> Option<usize> {
        if query.is_empty() {
            return None;
        }
        (0..=from.min(self.history.len().saturating_sub(1)))
            .rev()
            .find(|&i| !self.history.is_empty() && self.history[i].contains(query))
    }

    fn handle_search(&mut self, key: Key) -> Event {
        let mut search = self.search.take().expect("searching");
        let last = self.history.len().saturating_sub(1);
        match key {
            Key::Char(c) => {
                search.query.push(c);
                search.found = self.find_from(&search.query, last);
            }
            Key::Backspace => {
                search.query.pop();
                search.found = self.find_from(&search.query, last);
            }
            Key::Ctrl('r') => {
                // the next older match
                if let Some(f) = search.found {
                    if f > 0 {
                        search.found = self.find_from(&search.query, f - 1).or(Some(f));
                    }
                }
            }
            Key::Ctrl('c') | Key::Ctrl('g') | Key::Esc => {
                return Event::None; // leave the buffer as it was
            }
            Key::Tick => {}
            other => {
                // accept the match and carry on with the key
                if let Some(f) = search.found {
                    let s = self.history[f].clone();
                    self.set_text(&s);
                }
                return if other == Key::Enter { Event::None } else { self.handle(other) };
            }
        }
        self.search = Some(search);
        Event::None
    }

    /// The history entry the search currently shows.
    pub fn search_hit(&self) -> Option<&str> {
        let s = self.search.as_ref()?;
        s.found.map(|i| self.history[i].as_str())
    }

    // ---- completion support ---------------------------------------------------------------------

    /// Replaces `from..cursor` by `with` (used to insert a completion).
    pub fn replace_before_cursor(&mut self, from: usize, with: &str) {
        let from = from.min(self.cursor);
        self.snapshot();
        self.text.splice(from..self.cursor, with.chars());
        self.cursor = from + with.chars().count();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_str(ed: &mut Editor, s: &str) {
        for c in s.chars() {
            ed.handle(if c == '\n' { Key::NewLine } else { Key::Char(c) });
        }
    }

    fn ed_with(history: &[&str]) -> Editor {
        Editor::new(history.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn typing_and_arrow_keys_edit_in_place() {
        let mut e = ed_with(&[]);
        type_str(&mut e, "helo");
        e.handle(Key::Left);
        e.handle(Key::Char('l'));
        assert_eq!((e.text().as_str(), e.cursor()), ("hello", 4));
        e.handle(Key::Home);
        e.handle(Key::Delete);
        assert_eq!(e.text(), "ello");
        e.handle(Key::End);
        e.handle(Key::Backspace);
        assert_eq!(e.text(), "ell");
        assert_eq!(e.handle(Key::Enter), Event::Submit);
        assert_eq!(e.take(), "ell");
        assert!(e.is_empty());
    }

    #[test]
    fn up_and_down_walk_the_history_and_restore_the_draft() {
        let mut e = ed_with(&["one", "two", "three"]);
        type_str(&mut e, "dra");
        e.handle(Key::Up);
        assert_eq!(e.text(), "three");
        e.handle(Key::Up);
        e.handle(Key::Up);
        assert_eq!(e.text(), "one");
        e.handle(Key::Up);
        assert_eq!(e.text(), "one", "the oldest entry stays");
        e.handle(Key::Down);
        assert_eq!(e.text(), "two");
        e.handle(Key::Down);
        e.handle(Key::Down);
        assert_eq!(e.text(), "dra", "the draft comes back");
    }

    #[test]
    fn unfinished_input_continues_on_a_new_indented_line() {
        let mut e = ed_with(&[]);
        type_str(&mut e, "fn f() {");
        assert_eq!(e.handle(Key::Enter), Event::None, "an open brace continues the cell");
        assert_eq!(e.text(), "fn f() {\n    ");
        type_str(&mut e, "1");
        e.handle(Key::Enter);
        e.handle(Key::Char('}'));
        assert_eq!(e.text(), "fn f() {\n    1\n    }".replace("    }", "}"), "the closing brace steps back");
        assert_eq!(e.handle(Key::Enter), Event::Submit);
    }

    #[test]
    fn enter_between_brackets_opens_a_block() {
        let mut e = ed_with(&[]);
        type_str(&mut e, "if x {}");
        e.handle(Key::Left);
        e.handle(Key::Enter);
        assert_eq!(e.text(), "if x {\n    \n}");
        assert_eq!(e.position(), (1, 4));
    }

    #[test]
    fn up_and_down_move_between_the_lines_of_a_multi_line_buffer() {
        let mut e = ed_with(&["old"]);
        type_str(&mut e, "abc\nde\nfghij");
        assert_eq!(e.position(), (2, 5));
        e.handle(Key::Up);
        assert_eq!(e.position(), (1, 2), "clamped to the shorter line");
        e.handle(Key::Up);
        assert_eq!(e.position(), (0, 3));
        e.handle(Key::Down);
        e.handle(Key::Down);
        assert_eq!(e.position(), (2, 5), "the wanted column is remembered");
        e.handle(Key::Down);
        assert_eq!(e.text(), "abc\nde\nfghij", "no history ahead: nothing happens");
        e.handle(Key::Up);
        e.handle(Key::Up);
        e.handle(Key::Up);
        assert_eq!(e.text(), "old", "at the first line, Up browses the history");
    }

    #[test]
    fn emacs_keys() {
        let mut e = ed_with(&[]);
        type_str(&mut e, "let value = 12;");
        e.handle(Key::Ctrl('a'));
        e.handle(Key::WordRight);
        e.handle(Key::Ctrl('k'));
        assert_eq!(e.text(), "let ");
        e.handle(Key::Ctrl('y'));
        assert_eq!(e.text(), "let value = 12;");
        e.handle(Key::Ctrl('w'));
        assert_eq!(e.text(), "let value = ");
        e.handle(Key::Ctrl('u'));
        assert_eq!(e.text(), "");
        e.handle(Key::Ctrl('y'));
        assert_eq!(e.text(), "let value = ", "kills are remembered");
        e.handle(Key::Ctrl('_'));
        assert_eq!(e.text(), "");
    }

    #[test]
    fn control_c_and_control_d() {
        let mut e = ed_with(&[]);
        type_str(&mut e, "abc");
        assert_eq!(e.handle(Key::Ctrl('d')), Event::None, "deletes at the cursor");
        e.handle(Key::Left);
        e.handle(Key::Ctrl('d'));
        assert_eq!(e.text(), "ab");
        assert_eq!(e.handle(Key::Ctrl('c')), Event::Interrupt);
        assert!(e.is_empty());
        assert_eq!(e.handle(Key::Ctrl('d')), Event::Eof);
    }

    #[test]
    fn vi_normal_mode_moves_edits_and_browses_the_history() {
        let mut e = ed_with(&["first", "second"]);
        type_str(&mut e, "alpha beta gamma");
        e.handle(Key::Esc);
        assert_eq!(e.mode, Mode::Normal);
        e.handle(Key::Char('0'));
        e.handle(Key::Char('w'));
        assert_eq!(e.cursor(), 6);
        e.handle(Key::Char('d'));
        e.handle(Key::Char('w'));
        assert_eq!(e.text(), "alpha gamma");
        e.handle(Key::Char('$'));
        assert_eq!(e.cursor(), 10);
        e.handle(Key::Char('x'));
        assert_eq!(e.text(), "alpha gamm");
        e.handle(Key::Char('u'));
        assert_eq!(e.text(), "alpha gamma");
        // k and j browse the history, like the arrows
        e.handle(Key::Char('k'));
        assert_eq!(e.text(), "second");
        e.handle(Key::Char('k'));
        assert_eq!(e.text(), "first");
        e.handle(Key::Char('j'));
        assert_eq!(e.text(), "second");
        e.handle(Key::Char('j'));
        assert_eq!(e.text(), "alpha gamma");
        // insert again
        e.handle(Key::Char('A'));
        assert_eq!(e.mode, Mode::Insert);
        type_str(&mut e, "!");
        assert_eq!(e.text(), "alpha gamma!");
    }

    #[test]
    fn vi_dd_cc_and_paste() {
        let mut e = ed_with(&[]);
        type_str(&mut e, "one\ntwo\nthree");
        e.handle(Key::Esc);
        e.handle(Key::Char('k'));
        e.handle(Key::Char('d'));
        e.handle(Key::Char('d'));
        assert_eq!(e.text(), "one\nthree");
        e.handle(Key::Char('p'));
        assert!(e.text().contains("two"), "{}", e.text());
        let mut e = ed_with(&[]);
        type_str(&mut e, "abc def");
        e.handle(Key::Esc);
        e.handle(Key::Char('0'));
        e.handle(Key::Char('c'));
        e.handle(Key::Char('w'));
        type_str(&mut e, "X");
        assert_eq!(e.text(), "X def");
    }

    #[test]
    fn reverse_search_finds_older_entries_and_accepts() {
        let mut e = ed_with(&["let a = 1;", "println(a)", "let b = 2;"]);
        e.handle(Key::Ctrl('r'));
        assert!(e.search.is_some());
        e.handle(Key::Char('l'));
        e.handle(Key::Char('e'));
        assert_eq!(e.search_hit(), Some("let b = 2;"));
        e.handle(Key::Ctrl('r'));
        assert_eq!(e.search_hit(), Some("let a = 1;"));
        e.handle(Key::Enter);
        assert!(e.search.is_none());
        assert_eq!(e.text(), "let a = 1;");
        // Escape leaves the buffer alone
        let mut e = ed_with(&["xyz"]);
        type_str(&mut e, "keep");
        e.handle(Key::Ctrl('r'));
        e.handle(Key::Char('x'));
        e.handle(Key::Esc);
        assert_eq!(e.text(), "keep");
    }

    #[test]
    fn tab_indents_at_the_line_start_and_asks_for_completion_elsewhere() {
        let mut e = ed_with(&[]);
        assert_eq!(e.handle(Key::Tab), Event::None);
        assert_eq!(e.text(), "    ");
        type_str(&mut e, "pri");
        assert_eq!(e.handle(Key::Tab), Event::Complete(false));
        assert_eq!(e.handle(Key::BackTab), Event::Complete(true));
        e.replace_before_cursor(4, "println");
        assert_eq!(e.text(), "    println");
    }

    #[test]
    fn a_paste_keeps_its_line_breaks_and_does_not_submit() {
        let mut e = ed_with(&[]);
        e.handle(Key::Paste("let a = 1;\r\nlet b = 2;".into()));
        assert_eq!(e.text(), "let a = 1;\nlet b = 2;");
        assert_eq!(e.line_count(), 2);
    }

    #[test]
    fn taking_the_buffer_records_history_without_repeats() {
        let mut e = ed_with(&[]);
        type_str(&mut e, "x");
        e.take();
        type_str(&mut e, "x");
        e.take();
        type_str(&mut e, "   ");
        e.take();
        assert_eq!(e.history(), ["x"]);
    }
}
