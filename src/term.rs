//! Keys without Enter: what `term.read_key()` and `term.poll_key()` of the `term` module are made of.
//!
//! A key comes back as a name: the character itself for a printable one (`"a"`, `"A"`, `" "`, `"я"`),
//! otherwise `"enter"`, `"esc"`, `"tab"`, `"backspace"`, `"delete"`, `"insert"`, `"up"`, `"down"`,
//! `"left"`, `"right"`, `"home"`, `"end"`, `"pageup"`, `"pagedown"`, `"f1"` ... `"f12"`, and a
//! modifier in front: `"ctrl+c"`, `"alt+x"`, `"shift+tab"`, `"ctrl+up"`.
//!
//! [`decode`] turns bytes into names (escape sequences of ANSI terminals included); it is the same
//! whether the bytes come from a terminal in raw mode or from a pipe, so a program that is driven by
//! keys can be driven by a file in a test. [`Term`] owns the terminal: raw mode through `stty` (the
//! crate has no `unsafe`, so no termios), and a thread that reads the keyboard so a key can be waited
//! for with a timeout. On Windows a PowerShell child reads the console.

use std::collections::VecDeque;
use std::io::IsTerminal;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use crate::repl::terminal::RawMode;

/// What the source of bytes answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Byte(u8),
    Timeout,
    Eof,
    /// The source cannot go on (a browser that has to ask the user for more).
    Failed,
}

/// What a wait for a key ends with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Key(String),
    Timeout,
    Eof,
    /// Waiting for a key is not possible here; the program stops.
    Failed(String),
}

/// How long after an `Esc` byte the rest of an escape sequence may take to arrive.
const SEQUENCE: Duration = Duration::from_millis(30);

fn modifier(m: u32) -> String {
    let m = m.saturating_sub(1);
    let mut s = String::new();
    if m & 4 != 0 {
        s.push_str("ctrl+");
    }
    if m & 2 != 0 {
        s.push_str("alt+");
    }
    if m & 1 != 0 {
        s.push_str("shift+");
    }
    s
}

fn function_key(n: u32) -> Option<String> {
    match n {
        11..=15 => Some(format!("f{}", n - 10)),
        17..=21 => Some(format!("f{}", n - 11)),
        23 | 24 => Some(format!("f{}", n - 12)),
        _ => None,
    }
}

/// `ESC [ params final`
fn csi(next: &mut dyn FnMut(Option<Duration>) -> Next) -> String {
    let mut params = String::new();
    let final_byte = loop {
        match next(Some(SEQUENCE)) {
            Next::Byte(b) if (0x20..=0x3f).contains(&b) => params.push(b as char),
            Next::Byte(b) => break b,
            _ => return String::new(),
        }
    };
    if params.starts_with(['<', '?', '>', '=']) {
        return String::new(); // a mouse report, a device answer: not a key
    }
    let nums: Vec<u32> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
    let mods = nums.get(1).copied().filter(|&m| m > 1).map(modifier).unwrap_or_default();
    let first = nums.first().copied().unwrap_or(0);
    let name = match final_byte {
        b'A' => "up".to_string(),
        b'B' => "down".into(),
        b'C' => "right".into(),
        b'D' => "left".into(),
        b'H' => "home".into(),
        b'F' => "end".into(),
        b'Z' => return "shift+tab".into(),
        b'P'..=b'S' => format!("f{}", final_byte - b'P' + 1),
        b'~' => match first {
            1 | 7 => "home".into(),
            2 => "insert".into(),
            3 => "delete".into(),
            4 | 8 => "end".into(),
            5 => "pageup".into(),
            6 => "pagedown".into(),
            n => match function_key(n) {
                Some(f) => f,
                None => return String::new(),
            },
        },
        _ => return String::new(),
    };
    format!("{mods}{name}")
}

fn escape(next: &mut dyn FnMut(Option<Duration>) -> Next) -> String {
    match next(Some(SEQUENCE)) {
        Next::Timeout | Next::Eof | Next::Failed => "esc".into(),
        Next::Byte(b'[') => csi(next),
        Next::Byte(b'O') => match next(Some(SEQUENCE)) {
            Next::Byte(b @ b'P'..=b'S') => format!("f{}", b - b'P' + 1),
            Next::Byte(b'A') => "up".into(),
            Next::Byte(b'B') => "down".into(),
            Next::Byte(b'C') => "right".into(),
            Next::Byte(b'D') => "left".into(),
            Next::Byte(b'H') => "home".into(),
            Next::Byte(b'F') => "end".into(),
            _ => String::new(),
        },
        Next::Byte(b @ 0x21..=0x7e) => format!("alt+{}", b as char),
        Next::Byte(b' ') => "alt+ ".into(),
        Next::Byte(0x1b) => "esc".into(),
        Next::Byte(_) => String::new(),
    }
}

/// The next key from `next`, waiting at most `wait` for its first byte (`None`: as long as it takes).
pub fn decode(next: &mut dyn FnMut(Option<Duration>) -> Next, wait: Option<Duration>) -> Key {
    loop {
        let first = match next(wait) {
            Next::Byte(b) => b,
            Next::Timeout => return Key::Timeout,
            Next::Eof => return Key::Eof,
            Next::Failed => return Key::Failed("no more input".to_string()),
        };
        let name = match first {
            0x1b => escape(next),
            b'\r' | b'\n' => "enter".into(),
            b'\t' => "tab".into(),
            0x7f | 0x08 => "backspace".into(),
            0 => "ctrl+space".into(),
            1..=26 => format!("ctrl+{}", (b'a' + first - 1) as char),
            0x1c..=0x1f => format!("ctrl+{}", ['\\', ']', '^', '_'][(first - 0x1c) as usize]),
            0x20..=0x7e => (first as char).to_string(),
            0x80.. => {
                let len = match first {
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf7 => 4,
                    _ => 1,
                };
                let mut bytes = vec![first];
                while bytes.len() < len {
                    match next(Some(SEQUENCE)) {
                        Next::Byte(b) => bytes.push(b),
                        _ => break,
                    }
                }
                String::from_utf8(bytes).unwrap_or_default()
            }
        };
        if !name.is_empty() {
            return Key::Key(name);
        }
        // not a key (a report from the terminal, a stray byte): look for the next one
    }
}

/// `(columns, rows)` of the terminal, or the usual 80 x 24.
pub fn size() -> (i64, i64) {
    let (rows, cols) = crate::repl::terminal::size();
    (cols as i64, rows as i64)
}

/// The keyboard of a program: raw mode, and the bytes of the keys that are pressed.
#[derive(Debug)]
pub struct Term {
    /// Standard input is a terminal.
    tty: bool,
    raw: Option<RawMode>,
    /// Bytes (Unix) or finished key names (Windows) from the keyboard thread.
    bytes: Option<Receiver<u8>>,
    keys: Option<Receiver<String>>,
    backlog: VecDeque<u8>,
}

impl Term {
    pub fn new(tty: bool) -> Term {
        Term { tty, raw: None, bytes: None, keys: None, backlog: VecDeque::new() }
    }

    pub fn is_tty(&self) -> bool {
        self.tty && std::io::stdout().is_terminal()
    }

    /// Switches raw mode on or off; true if the terminal is in the requested mode afterwards.
    pub fn set_raw(&mut self, on: bool) -> bool {
        if !self.tty {
            return false;
        }
        if !on {
            self.raw = None; // dropping restores the terminal
            return true;
        }
        if self.raw.is_some() {
            return true;
        }
        #[cfg(windows)]
        {
            if self.keys.is_none() {
                self.keys = console::spawn();
            }
            return self.keys.is_some();
        }
        #[cfg(not(windows))]
        {
            match RawMode::enter() {
                Ok(mode) => {
                    self.raw = Some(mode);
                    if self.bytes.is_none() {
                        self.bytes = keyboard_thread();
                    }
                    true
                }
                Err(_) => false,
            }
        }
    }

    /// Whether keys come from the keyboard (raw mode) rather than from the program's input stream.
    fn live(&self) -> bool {
        self.raw.is_some() || self.keys.is_some()
    }

    /// The next key. `stream` supplies bytes when the keyboard is not being read directly (input that
    /// is piped, or a terminal that is not in raw mode).
    pub fn key(&mut self, wait: Option<Duration>, stream: &mut dyn FnMut(Option<Duration>) -> Next) -> Key {
        if !self.live() {
            return decode(stream, wait);
        }
        if let Some(rx) = &self.keys {
            return match wait {
                None => rx.recv().map_or(Key::Eof, Key::Key),
                Some(d) => match rx.recv_timeout(d) {
                    Ok(k) => Key::Key(k),
                    Err(RecvTimeoutError::Timeout) => Key::Timeout,
                    Err(RecvTimeoutError::Disconnected) => Key::Eof,
                },
            };
        }
        let Some(rx) = &self.bytes else { return decode(stream, wait) };
        let backlog = &mut self.backlog;
        let mut next = |timeout: Option<Duration>| -> Next {
            if let Some(b) = backlog.pop_front() {
                return Next::Byte(b);
            }
            let got = match timeout {
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some(d) => rx.recv_timeout(d),
            };
            match got {
                Ok(b) => Next::Byte(b),
                Err(RecvTimeoutError::Timeout) => Next::Timeout,
                Err(RecvTimeoutError::Disconnected) => Next::Eof,
            }
        };
        decode(&mut next, wait)
    }
}

#[cfg(not(windows))]
fn keyboard_thread() -> Option<Receiver<u8>> {
    use std::io::Read;
    let mut tty = std::fs::File::open("/dev/tty").ok()?;
    let (tx, rx) = std::sync::mpsc::channel::<u8>();
    std::thread::Builder::new()
        .name("neon-keys".into())
        .spawn(move || {
            let mut chunk = [0u8; 256];
            loop {
                match tty.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for &b in &chunk[..n] {
                            if tx.send(b).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        })
        .ok()?;
    Some(rx)
}

#[cfg(windows)]
mod console {
    //! The Windows console has no `stty`; a PowerShell child reads the keys and prints one line per key.
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::{self, Receiver};

    const SCRIPT: &str = "[Console]::TreatControlCAsInput=$true;while($true){$k=[Console]::ReadKey($true);\
        [Console]::Out.WriteLine(('{0} {1} {2}' -f $k.Key,[int]$k.KeyChar,[int]$k.Modifiers));[Console]::Out.Flush()}";

    pub fn spawn() -> Option<Receiver<String>> {
        let mut child = Command::new("powershell")
            .args(["-NoProfile", "-NoLogo", "-Command", SCRIPT])
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let out = child.stdout.take()?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("neon-keys".into())
            .spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(key) = name(&line) {
                        if tx.send(key).is_err() {
                            break;
                        }
                    }
                }
                let _ = child.kill();
            })
            .ok()?;
        Some(rx)
    }

    /// `"UpArrow 0 0"` (key, character code, modifiers: Alt 1, Shift 2, Control 4) to a key name.
    pub fn name(line: &str) -> Option<String> {
        let mut it = line.split_whitespace();
        let (key, ch, mods) = (it.next()?, it.next()?.parse::<u32>().ok()?, it.next()?.parse::<u32>().ok()?);
        let base = match key {
            "UpArrow" => "up".to_string(),
            "DownArrow" => "down".into(),
            "LeftArrow" => "left".into(),
            "RightArrow" => "right".into(),
            "Enter" => "enter".into(),
            "Escape" => "esc".into(),
            "Tab" => "tab".into(),
            "Backspace" => "backspace".into(),
            "Delete" => "delete".into(),
            "Insert" => "insert".into(),
            "Home" => "home".into(),
            "End" => "end".into(),
            "PageUp" => "pageup".into(),
            "PageDown" => "pagedown".into(),
            f if f.len() <= 3 && f.starts_with('F') && f[1..].parse::<u32>().is_ok() => f.to_lowercase(),
            _ if ch >= 32 => char::from_u32(ch)?.to_string(),
            k if k.len() == 1 && mods & 4 != 0 => k.to_lowercase(),
            _ => return None,
        };
        let mut prefix = String::new();
        if mods & 4 != 0 {
            prefix.push_str("ctrl+");
        }
        if mods & 1 != 0 {
            prefix.push_str("alt+");
        }
        if mods & 2 != 0 && base.len() > 1 {
            prefix.push_str("shift+");
        }
        Some(format!("{prefix}{base}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(input: &[u8]) -> Vec<String> {
        let mut at = 0;
        let mut next = |_: Option<Duration>| match input.get(at) {
            Some(&b) => {
                at += 1;
                Next::Byte(b)
            }
            None => Next::Eof,
        };
        let mut out = Vec::new();
        while let Key::Key(k) = decode(&mut next, None) {
            out.push(k);
        }
        out
    }

    #[test]
    fn plain_keys() {
        assert_eq!(keys(b"aZ 9"), ["a", "Z", " ", "9"]);
        assert_eq!(keys("яЖ→".as_bytes()), ["я", "Ж", "→"]);
        assert_eq!(keys(b"\r\n\t\x7f"), ["enter", "enter", "tab", "backspace"]);
        assert_eq!(keys(&[3, 1, 26, 0]), ["ctrl+c", "ctrl+a", "ctrl+z", "ctrl+space"]);
    }

    #[test]
    fn escape_sequences() {
        assert_eq!(keys(b"\x1b[A\x1b[B\x1b[C\x1b[D"), ["up", "down", "right", "left"]);
        assert_eq!(keys(b"\x1bOA\x1bOP\x1b[H\x1b[F"), ["up", "f1", "home", "end"]);
        assert_eq!(keys(b"\x1b[3~\x1b[5~\x1b[6~\x1b[2~"), ["delete", "pageup", "pagedown", "insert"]);
        assert_eq!(keys(b"\x1b[15~\x1b[24~\x1b[11~"), ["f5", "f12", "f1"]);
        assert_eq!(keys(b"\x1b[1;5A\x1b[1;2C\x1b[1;3D"), ["ctrl+up", "shift+right", "alt+left"]);
        assert_eq!(keys(b"\x1b[Z"), ["shift+tab"]);
        assert_eq!(keys(b"\x1bx"), ["alt+x"]);
        // a lone Esc, and keys after it
        assert_eq!(keys(b"\x1b"), ["esc"]);
        // reports that are not keys are skipped
        assert_eq!(keys(b"\x1b[<0;5;5Mq"), ["q"]);
    }

    #[test]
    fn waiting_ends_with_a_timeout_or_the_end() {
        let mut next = |_: Option<Duration>| Next::Timeout;
        assert_eq!(decode(&mut next, Some(Duration::from_millis(1))), Key::Timeout);
        let mut eof = |_: Option<Duration>| Next::Eof;
        assert_eq!(decode(&mut eof, None), Key::Eof);
    }
}
