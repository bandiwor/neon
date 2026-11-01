//! The terminal side of the interactive prompt: raw mode, key decoding and character widths.
//!
//! The standard library has no termios (and the crate has no `unsafe`), so the mode is switched with
//! `stty` (it acts on the terminal that is standard input). A thread of its own reads the terminal
//! ([`TtyInput`]) and hands the bytes over through a channel with a timeout: a lone `Esc` can be told
//! from the start of an escape sequence, the prompt gets idle moments to compute its preview, and
//! `Ctrl-C` can be noticed while a cell is running, when nobody else is reading.

use std::collections::VecDeque;
use std::io::{self, BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use super::editor::Key;

pub(crate) fn stty(args: &[&str]) -> io::Result<String> {
    let out = Command::new("stty").args(args).stdin(Stdio::inherit()).stderr(Stdio::null()).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!("stty {} failed", args.join(" "))));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The terminal in raw mode until dropped.
#[derive(Debug)]
pub struct RawMode {
    saved: String,
}

const RAW: &[&str] =
    &["-icanon", "-echo", "-isig", "-ixon", "-icrnl", "-inlcr", "-igncr", "-iexten", "min", "1", "time", "0"];

impl RawMode {
    pub fn enter() -> io::Result<RawMode> {
        let saved = stty(&["-g"])?.trim().to_string();
        if saved.is_empty() {
            return Err(io::Error::other("not a terminal"));
        }
        stty(RAW)?;
        Ok(RawMode { saved })
    }

    /// The terminal as it was before: for the time a program runs and reads its own input.
    pub fn suspend(&self) {
        let _ = stty(&[&self.saved]);
    }

    pub fn resume(&self) {
        let _ = stty(RAW);
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        self.suspend();
    }
}

/// `(rows, columns)` of the terminal.
pub fn size() -> (usize, usize) {
    if let Ok(s) = stty(&["size"]) {
        let mut it = s.split_whitespace().filter_map(|v| v.parse::<usize>().ok());
        if let (Some(r), Some(c)) = (it.next(), it.next()) {
            if r > 0 && c > 0 {
                return (r, c);
            }
        }
    }
    let cols = std::env::var("COLUMNS").ok().and_then(|v| v.parse().ok()).unwrap_or(80);
    let rows = std::env::var("LINES").ok().and_then(|v| v.parse().ok()).unwrap_or(24);
    (rows, cols)
}

/// What the terminal sends, read by a thread of its own. It is a [`BufRead`]: when nothing arrives for a
/// tenth of a second the buffer is empty (a pause, not the end of input).
#[derive(Debug)]
pub struct TtyInput {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
    running: Arc<AtomicBool>,
}

impl TtyInput {
    /// Starts reading `/dev/tty` (the terminal must already be in raw mode). While a cell runs (see
    /// [`set_running`](Self::set_running)) a `Ctrl-C` byte sets `interrupt` instead of being delivered.
    pub fn open(interrupt: &'static AtomicBool) -> io::Result<TtyInput> {
        let mut tty = std::fs::File::open("/dev/tty")?;
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let running = Arc::new(AtomicBool::new(false));
        let flag = running.clone();
        std::thread::Builder::new().name("neon-tty".into()).spawn(move || {
            let mut chunk = [0u8; 512];
            loop {
                match tty.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut data = chunk[..n].to_vec();
                        if flag.load(Ordering::SeqCst) && data.contains(&3) {
                            interrupt.store(true, Ordering::SeqCst);
                            data.retain(|&b| b != 3);
                        }
                        if !data.is_empty() && tx.send(data).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        })?;
        Ok(TtyInput { rx, buf: Vec::new(), pos: 0, running })
    }

    /// Whether a cell is running: its `Ctrl-C` is an interrupt, not a key.
    pub fn set_running(&self, on: bool) {
        self.running.store(on, Ordering::SeqCst);
    }

    /// The next byte, waiting at most `timeout`.
    fn byte(&mut self, timeout: Duration) -> io::Result<Option<u8>> {
        if self.pos >= self.buf.len() {
            match self.rx.recv_timeout(timeout) {
                Ok(v) => {
                    self.buf = v;
                    self.pos = 0;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(io::ErrorKind::UnexpectedEof.into()),
            }
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        Ok(Some(b))
    }
}

impl Read for TtyInput {
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

impl BufRead for TtyInput {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos >= self.buf.len() {
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(v) => {
                    self.buf = v;
                    self.pos = 0;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.buf.clear();
                    self.pos = 0;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(io::ErrorKind::UnexpectedEof.into()),
            }
        }
        Ok(&self.buf[self.pos..])
    }

    fn consume(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.buf.len());
    }
}

/// Lines for the program that runs in a cell (`input()`), typed at a terminal that is in raw mode: the
/// typed characters are echoed, Backspace edits the line, `Ctrl-D` on an empty line ends the input and
/// `Ctrl-C` (seen by the reading thread) makes the read fail so the cell stops.
#[derive(Debug)]
pub struct LineInput<'a> {
    tty: &'a mut TtyInput,
    interrupt: &'static AtomicBool,
    line: Vec<u8>,
    pos: usize,
    eof: bool,
}

impl<'a> LineInput<'a> {
    pub fn new(tty: &'a mut TtyInput, interrupt: &'static AtomicBool) -> LineInput<'a> {
        LineInput { tty, interrupt, line: Vec::new(), pos: 0, eof: false }
    }
}

fn echo(s: &str) {
    let mut out = io::stdout();
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

impl Read for LineInput<'_> {
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

impl BufRead for LineInput<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos < self.line.len() {
            return Ok(&self.line[self.pos..]);
        }
        if self.eof {
            return Ok(&[]);
        }
        self.line.clear();
        self.pos = 0;
        let mut typed: Vec<u8> = Vec::new();
        let mut partial: Vec<u8> = Vec::new();
        loop {
            if self.interrupt.load(Ordering::SeqCst) {
                return Err(io::Error::other("interrupted"));
            }
            let Some(b) = self.tty.byte(Duration::from_millis(50))? else { continue };
            match b {
                b'\r' | b'\n' => {
                    echo("\r\n");
                    typed.push(b'\n');
                    self.line = typed;
                    return Ok(&self.line);
                }
                0x7f | 0x08 => {
                    // one character, which may be several bytes
                    while let Some(last) = typed.pop() {
                        if last & 0xc0 != 0x80 {
                            echo("\x08 \x08");
                            break;
                        }
                    }
                }
                0x04 => {
                    if typed.is_empty() {
                        self.eof = true;
                        return Ok(&[]);
                    }
                }
                0x15 => {
                    // Ctrl-U: forget the line
                    let n = String::from_utf8_lossy(&typed).chars().count();
                    echo(&"\x08 \x08".repeat(n));
                    typed.clear();
                }
                0x1b => {
                    // an escape sequence (arrows and the like) is of no use here
                    while let Some(c) = self.tty.byte(Duration::from_millis(5))? {
                        if (0x40..=0x7e).contains(&c) && c != b'[' && c != b'O' {
                            break;
                        }
                    }
                }
                0x20.. => {
                    // a character may arrive in several bytes: echo it once it is whole
                    partial.push(b);
                    if let Ok(text) = std::str::from_utf8(&partial) {
                        echo(text);
                        typed.append(&mut partial);
                    } else if partial.len() >= 4 {
                        partial.clear();
                    }
                }
                _ => {}
            }
        }
    }

    fn consume(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.line.len());
    }
}

/// Columns a character occupies: 0 for combining marks, 2 for wide (East Asian, emoji), else 1.
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    match u {
        0x0300..=0x036f | 0x200b..=0x200f | 0x20d0..=0x20ff | 0xfe00..=0xfe0f | 0xfe20..=0xfe2f => 0,
        0x1100..=0x115f
        | 0x2e80..=0xa4cf
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe30..=0xfe6f
        | 0xff00..=0xff60
        | 0xffe0..=0xffe6
        | 0x1f300..=0x1f64f
        | 0x1f900..=0x1f9ff
        | 0x20000..=0x3fffd => 2,
        _ => 1,
    }
}

/// Turns bytes into keys. A read that returns nothing is a pause, not the end of input.
#[derive(Debug, Default)]
pub struct Decoder {
    pushback: VecDeque<u8>,
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder::default()
    }

    /// The next byte, or `None` if nothing arrived in time.
    fn byte(&mut self, r: &mut dyn BufRead) -> io::Result<Option<u8>> {
        if let Some(b) = self.pushback.pop_front() {
            return Ok(Some(b));
        }
        loop {
            match r.fill_buf() {
                Ok([]) => return Ok(None),
                Ok(buf) => {
                    let b = buf[0];
                    r.consume(1);
                    return Ok(Some(b));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// The next key; [`Key::Tick`] when the input paused.
    pub fn key(&mut self, r: &mut dyn BufRead) -> io::Result<Key> {
        let Some(b) = self.byte(r)? else { return Ok(Key::Tick) };
        Ok(match b {
            0x0d => Key::Enter,
            0x0a => Key::NewLine,
            0x09 => Key::Tab,
            0x7f | 0x08 => Key::Backspace,
            0x1b => self.escape(r)?,
            0x1f => Key::Ctrl('_'),
            0x00 => Key::Ctrl(' '),
            0x01..=0x1a => Key::Ctrl((b'a' + b - 1) as char),
            0x20..=0x7e => Key::Char(b as char),
            0x80.. => Key::Char(self.utf8(b, r)?),
            _ => Key::Tick,
        })
    }

    fn utf8(&mut self, first: u8, r: &mut dyn BufRead) -> io::Result<char> {
        let extra = match first {
            0xc0..=0xdf => 1,
            0xe0..=0xef => 2,
            0xf0..=0xf7 => 3,
            _ => return Ok('\u{fffd}'),
        };
        let mut bytes = vec![first];
        for _ in 0..extra {
            match self.byte(r)? {
                Some(b) if b & 0xc0 == 0x80 => bytes.push(b),
                Some(b) => {
                    self.pushback.push_front(b);
                    break;
                }
                None => break,
            }
        }
        Ok(std::str::from_utf8(&bytes).ok().and_then(|s| s.chars().next()).unwrap_or('\u{fffd}'))
    }

    fn escape(&mut self, r: &mut dyn BufRead) -> io::Result<Key> {
        let Some(b) = self.byte(r)? else { return Ok(Key::Esc) };
        match b {
            b'[' => self.csi(r),
            b'O' => Ok(match self.byte(r)? {
                Some(b'A') => Key::Up,
                Some(b'B') => Key::Down,
                Some(b'C') => Key::Right,
                Some(b'D') => Key::Left,
                Some(b'H') => Key::Home,
                Some(b'F') => Key::End,
                _ => Key::Esc,
            }),
            0x0d | 0x0a => Ok(Key::NewLine),
            b'b' => Ok(Key::WordLeft),
            b'f' => Ok(Key::WordRight),
            0x7f => Ok(Key::Ctrl('w')),
            other => {
                // not an Alt combination we know: the Esc stands alone, the byte is typed next
                self.pushback.push_front(other);
                Ok(Key::Esc)
            }
        }
    }

    /// After `ESC [`: parameters, then a final byte.
    fn csi(&mut self, r: &mut dyn BufRead) -> io::Result<Key> {
        let mut params = String::new();
        let fin = loop {
            match self.byte(r)? {
                Some(b @ (b'0'..=b'9' | b';' | b'?' | b'<' | b'=' | b'>')) => params.push(b as char),
                Some(b) => break b,
                None => return Ok(Key::Esc),
            }
        };
        let modified = params.split(';').nth(1).is_some_and(|m| m == "5" || m == "3" || m == "7");
        Ok(match (fin, params.as_str()) {
            (b'A', _) => Key::Up,
            (b'B', _) => Key::Down,
            (b'C', _) if modified => Key::WordRight,
            (b'D', _) if modified => Key::WordLeft,
            (b'C', _) => Key::Right,
            (b'D', _) => Key::Left,
            (b'H', _) => Key::Home,
            (b'F', _) => Key::End,
            (b'Z', _) => Key::BackTab,
            (b'~', p) => match p.split(';').next().unwrap_or("") {
                "1" | "7" => Key::Home,
                "3" => Key::Delete,
                "4" | "8" => Key::End,
                "200" => Key::Paste(self.paste(r)?),
                _ => Key::Tick,
            },
            _ => Key::Tick,
        })
    }

    /// The text of a bracketed paste, up to `ESC [ 2 0 1 ~`.
    fn paste(&mut self, r: &mut dyn BufRead) -> io::Result<String> {
        const END: &[u8] = b"\x1b[201~";
        let mut bytes: Vec<u8> = Vec::new();
        // a paste cut short, or a terminal that stalled, ends with what came
        while let Some(b) = self.byte(r)? {
            bytes.push(b);
            if bytes.ends_with(END) {
                bytes.truncate(bytes.len() - END.len());
                break;
            }
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn keys(input: &[u8]) -> Vec<Key> {
        let mut r = Cursor::new(input.to_vec());
        let mut d = Decoder::new();
        let mut out = Vec::new();
        loop {
            match d.key(&mut r).unwrap() {
                Key::Tick => break,
                k => out.push(k),
            }
        }
        out
    }

    #[test]
    fn plain_text_and_control_keys() {
        assert_eq!(keys(b"ab\r"), [Key::Char('a'), Key::Char('b'), Key::Enter]);
        assert_eq!(keys(b"\x01\x05\x0b\x03"), [Key::Ctrl('a'), Key::Ctrl('e'), Key::Ctrl('k'), Key::Ctrl('c')]);
        assert_eq!(keys(b"\x7f\t\n"), [Key::Backspace, Key::Tab, Key::NewLine]);
        assert_eq!(keys("é€😀".as_bytes()), [Key::Char('é'), Key::Char('€'), Key::Char('😀')]);
    }

    #[test]
    fn arrows_and_editing_keys() {
        assert_eq!(keys(b"\x1b[A\x1b[B\x1b[C\x1b[D"), [Key::Up, Key::Down, Key::Right, Key::Left]);
        assert_eq!(keys(b"\x1bOA\x1bOH\x1bOF"), [Key::Up, Key::Home, Key::End]);
        assert_eq!(keys(b"\x1b[H\x1b[F\x1b[3~\x1b[1~\x1b[4~"), [Key::Home, Key::End, Key::Delete, Key::Home, Key::End]);
        assert_eq!(keys(b"\x1b[Z"), [Key::BackTab]);
        assert_eq!(keys(b"\x1b[1;5C\x1b[1;5D"), [Key::WordRight, Key::WordLeft]);
        assert_eq!(keys(b"\x1b\r"), [Key::NewLine], "Alt-Enter");
    }

    #[test]
    fn a_lone_escape_and_an_escape_before_a_letter() {
        assert_eq!(keys(b"\x1b"), [Key::Esc]);
        // `Esc k` typed fast is Esc then k, not an unknown Alt combination
        assert_eq!(keys(b"\x1bk"), [Key::Esc, Key::Char('k')]);
        assert_eq!(keys(b"\x1b\x1b[A"), [Key::Esc, Key::Up]);
    }

    #[test]
    fn bracketed_paste_is_one_key() {
        assert_eq!(
            keys(b"\x1b[200~let a = 1;\nlet b = 2;\x1b[201~x"),
            [Key::Paste("let a = 1;\nlet b = 2;".into()), Key::Char('x')]
        );
        // cut short
        assert_eq!(keys(b"\x1b[200~abc"), [Key::Paste("abc".into())]);
    }

    #[test]
    fn widths() {
        assert_eq!(
            (char_width('a'), char_width('é'), char_width('語'), char_width('😀'), char_width('\u{301}')),
            (1, 1, 2, 2, 0)
        );
    }
}
