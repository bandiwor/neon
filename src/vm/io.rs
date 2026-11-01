//! The VM's window to the outside world. The VM never touches stdin or
//! stdout itself, so it runs unchanged in tests, in a terminal, or in a
//! browser through WebAssembly.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::time::Duration;

use crate::host::{self, MemFiles};
use crate::term::{self, Key, Next, Term};

/// What the file functions say when there is no file system to talk to.
pub const NO_FS: &str = "the file system is not available here";

/// Everything the running program can observe or affect outside the VM.
pub trait VmIo {
    /// `print(x)`: text without a trailing newline.
    fn print(&mut self, s: &str);

    /// `println(x)`: text followed by a newline.
    fn println(&mut self, s: &str) {
        self.print(s);
        self.print("\n");
    }

    /// `input()`: the next line without its terminator; `Ok(None)` at end of
    /// input (the program then sees an empty string).
    fn input(&mut self) -> Result<Option<String>, String>;

    /// `sys.args()`: the command-line arguments of the program (without the program itself).
    fn args(&self) -> Vec<String> {
        Vec::new()
    }

    // ---- the standard modules: `sys`, `fs`, `term` ---------------------------------------------
    // A host that has nothing to offer says so with these defaults; the terminal gives the real thing.

    fn env(&self, _name: &str) -> Option<String> {
        None
    }

    fn platform(&self) -> String {
        "unknown".to_string()
    }

    /// Milliseconds since 1970-01-01 UTC.
    fn now_ms(&mut self) -> i64 {
        0
    }

    /// Milliseconds on a clock that only goes forward.
    fn ticks_ms(&mut self) -> i64 {
        0
    }

    fn sleep_ms(&mut self, _ms: u64) {}

    fn fs_read(&mut self, _path: &str) -> Result<String, String> {
        Err(NO_FS.to_string())
    }

    fn fs_write(&mut self, _path: &str, _text: &str) -> Result<(), String> {
        Err(NO_FS.to_string())
    }

    fn fs_append(&mut self, _path: &str, _text: &str) -> Result<(), String> {
        Err(NO_FS.to_string())
    }

    fn fs_exists(&mut self, _path: &str) -> bool {
        false
    }

    fn fs_is_file(&mut self, _path: &str) -> bool {
        false
    }

    fn fs_is_dir(&mut self, _path: &str) -> bool {
        false
    }

    fn fs_list(&mut self, _path: &str) -> Result<Vec<String>, String> {
        Err(NO_FS.to_string())
    }

    fn fs_create_dir(&mut self, _path: &str) -> Result<(), String> {
        Err(NO_FS.to_string())
    }

    fn fs_remove(&mut self, _path: &str) -> Result<(), String> {
        Err(NO_FS.to_string())
    }

    fn fs_rename(&mut self, _from: &str, _to: &str) -> Result<(), String> {
        Err(NO_FS.to_string())
    }

    /// Raw mode: keys arrive one at a time, without Enter and without echo. True if it is on afterwards.
    fn term_raw(&mut self, _on: bool) -> bool {
        false
    }

    /// The next key, waiting at most `wait` (`None`: as long as it takes).
    fn term_key(&mut self, _wait: Option<Duration>) -> Key {
        Key::Eof
    }

    /// `(columns, rows)`.
    fn term_size(&self) -> (i64, i64) {
        (80, 24)
    }

    fn term_is_tty(&self) -> bool {
        false
    }
}

/// The bytes of a string as a source of keys.
fn bytes_of(queue: &mut VecDeque<u8>) -> impl FnMut(Option<Duration>) -> Next + '_ {
    move |_| queue.pop_front().map_or(Next::Eof, Next::Byte)
}

/// Collects output in a string and serves input from a queue of lines.
#[derive(Debug, Default)]
pub struct BufferIo {
    pub output: String,
    input: VecDeque<String>,
    args: Vec<String>,
    /// The keys `term.read_key()` hands out, as the bytes a terminal would send.
    keys: VecDeque<u8>,
    /// The files of the program; `None`: there is no file system.
    pub files: Option<MemFiles>,
    env: Vec<(String, String)>,
    raw: bool,
}

impl BufferIo {
    pub fn new() -> BufferIo {
        BufferIo::default()
    }

    pub fn with_input<S: AsRef<str>>(lines: &[S]) -> BufferIo {
        BufferIo { input: lines.iter().map(|l| l.as_ref().to_string()).collect(), ..BufferIo::default() }
    }

    /// The keys that are "pressed", as the bytes a terminal would send (`b"a\x1b[A"`).
    pub fn with_keys(mut self, keys: &[u8]) -> BufferIo {
        self.keys = keys.iter().copied().collect();
        self
    }

    /// Gives the program a file system in memory.
    pub fn with_files(mut self, files: MemFiles) -> BufferIo {
        self.files = Some(files);
        self
    }

    pub fn with_env(mut self, name: &str, value: &str) -> BufferIo {
        self.env.push((name.to_string(), value.to_string()));
        self
    }

    /// The program's command-line arguments (`sys.args`).
    pub fn with_args<S: AsRef<str>>(mut self, args: &[S]) -> BufferIo {
        self.args = args.iter().map(|a| a.as_ref().to_string()).collect();
        self
    }
}

impl VmIo for BufferIo {
    fn print(&mut self, s: &str) {
        self.output.push_str(s);
    }

    fn input(&mut self) -> Result<Option<String>, String> {
        Ok(self.input.pop_front())
    }

    fn args(&self) -> Vec<String> {
        self.args.clone()
    }

    fn env(&self, name: &str) -> Option<String> {
        self.env.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    }

    fn platform(&self) -> String {
        "test".to_string()
    }

    fn fs_read(&mut self, path: &str) -> Result<String, String> {
        self.files.as_ref().ok_or(NO_FS.to_string())?.read(path)
    }

    fn fs_write(&mut self, path: &str, text: &str) -> Result<(), String> {
        self.files.as_mut().ok_or(NO_FS.to_string())?.write(path, text, false)
    }

    fn fs_append(&mut self, path: &str, text: &str) -> Result<(), String> {
        self.files.as_mut().ok_or(NO_FS.to_string())?.write(path, text, true)
    }

    fn fs_exists(&mut self, path: &str) -> bool {
        self.files.as_ref().is_some_and(|f| f.exists(path))
    }

    fn fs_is_file(&mut self, path: &str) -> bool {
        self.files.as_ref().is_some_and(|f| f.is_file(path))
    }

    fn fs_is_dir(&mut self, path: &str) -> bool {
        self.files.as_ref().is_some_and(|f| f.is_dir(path))
    }

    fn fs_list(&mut self, path: &str) -> Result<Vec<String>, String> {
        self.files.as_ref().ok_or(NO_FS.to_string())?.list(path)
    }

    fn fs_create_dir(&mut self, path: &str) -> Result<(), String> {
        self.files.as_mut().ok_or(NO_FS.to_string())?.create_dir(path)
    }

    fn fs_remove(&mut self, path: &str) -> Result<(), String> {
        self.files.as_mut().ok_or(NO_FS.to_string())?.remove(path)
    }

    fn fs_rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        self.files.as_mut().ok_or(NO_FS.to_string())?.rename(from, to)
    }

    fn term_raw(&mut self, on: bool) -> bool {
        self.raw = on;
        true
    }

    fn term_key(&mut self, wait: Option<Duration>) -> Key {
        term::decode(&mut bytes_of(&mut self.keys), wait)
    }

    fn term_is_tty(&self) -> bool {
        true
    }
}

/// Adapts any reader / writer pair (stdin and stdout for the CLI).
#[derive(Debug)]
pub struct StreamIo<R: BufRead, W: Write> {
    reader: R,
    writer: W,
    args: Vec<String>,
    /// The keyboard and the terminal mode.
    term: Term,
    /// Where the files are: `None` is the disk; a program in a sandbox (the browser) gets a memory.
    files: Option<MemFiles>,
    /// Set when the program stopped for want of a key (rather than a line): the browser asks the user.
    key_wait: Option<std::rc::Rc<std::cell::Cell<bool>>>,
    /// The first write error, kept so output problems are not silently lost.
    pub write_error: Option<String>,
}

impl<R: BufRead, W: Write> StreamIo<R, W> {
    pub fn new(reader: R, writer: W) -> StreamIo<R, W> {
        StreamIo {
            reader,
            writer,
            args: Vec::new(),
            term: Term::new(false),
            files: None,
            key_wait: None,
            write_error: None,
        }
    }

    /// Standard input is a terminal: `term.raw(true)` can read the keyboard directly.
    pub fn with_tty(mut self, tty: bool) -> StreamIo<R, W> {
        self.term = Term::new(tty);
        self
    }

    /// A sandbox: the files are these, the clock stands still and the environment is empty.
    pub fn with_files(mut self, files: MemFiles) -> StreamIo<R, W> {
        self.files = Some(files);
        self
    }

    /// A flag that tells whether the input that ran out was wanted as a key (`true`) or as a line.
    pub fn with_key_wait(mut self, flag: std::rc::Rc<std::cell::Cell<bool>>) -> StreamIo<R, W> {
        self.key_wait = Some(flag);
        self
    }

    /// The files of a sandbox, as the program left them.
    pub fn files(&self) -> Option<&MemFiles> {
        self.files.as_ref()
    }

    /// The program's command-line arguments (`sys.args`).
    pub fn with_args(mut self, args: Vec<String>) -> StreamIo<R, W> {
        self.args = args;
        self
    }

    pub fn into_parts(self) -> (R, W) {
        (self.reader, self.writer)
    }
}

impl<R: BufRead, W: Write> VmIo for StreamIo<R, W> {
    fn args(&self) -> Vec<String> {
        self.args.clone()
    }

    fn env(&self, name: &str) -> Option<String> {
        if self.files.is_some() {
            None
        } else {
            host::env(name)
        }
    }

    fn platform(&self) -> String {
        if self.files.is_some() {
            "web".to_string()
        } else {
            host::platform().to_string()
        }
    }

    fn now_ms(&mut self) -> i64 {
        if self.files.is_some() {
            0
        } else {
            host::now_ms()
        }
    }

    fn ticks_ms(&mut self) -> i64 {
        if self.files.is_some() {
            0
        } else {
            host::ticks_ms()
        }
    }

    fn sleep_ms(&mut self, ms: u64) {
        if self.files.is_none() {
            let _ = self.writer.flush();
            host::sleep_ms(ms);
        }
    }

    fn fs_read(&mut self, path: &str) -> Result<String, String> {
        match &self.files {
            Some(m) => m.read(path),
            None => host::fs_read(path),
        }
    }

    fn fs_write(&mut self, path: &str, text: &str) -> Result<(), String> {
        match &mut self.files {
            Some(m) => m.write(path, text, false),
            None => host::fs_write(path, text),
        }
    }

    fn fs_append(&mut self, path: &str, text: &str) -> Result<(), String> {
        match &mut self.files {
            Some(m) => m.write(path, text, true),
            None => host::fs_append(path, text),
        }
    }

    fn fs_exists(&mut self, path: &str) -> bool {
        self.files.as_ref().map_or_else(|| host::fs_exists(path), |m| m.exists(path))
    }

    fn fs_is_file(&mut self, path: &str) -> bool {
        self.files.as_ref().map_or_else(|| host::fs_is_file(path), |m| m.is_file(path))
    }

    fn fs_is_dir(&mut self, path: &str) -> bool {
        self.files.as_ref().map_or_else(|| host::fs_is_dir(path), |m| m.is_dir(path))
    }

    fn fs_list(&mut self, path: &str) -> Result<Vec<String>, String> {
        match &self.files {
            Some(m) => m.list(path),
            None => host::fs_list(path),
        }
    }

    fn fs_create_dir(&mut self, path: &str) -> Result<(), String> {
        match &mut self.files {
            Some(m) => m.create_dir(path),
            None => host::fs_create_dir(path),
        }
    }

    fn fs_remove(&mut self, path: &str) -> Result<(), String> {
        match &mut self.files {
            Some(m) => m.remove(path),
            None => host::fs_remove(path),
        }
    }

    fn fs_rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        match &mut self.files {
            Some(m) => m.rename(from, to),
            None => host::fs_rename(from, to),
        }
    }

    fn term_raw(&mut self, on: bool) -> bool {
        let _ = self.writer.flush();
        self.term.set_raw(on) || self.files.is_some() || !self.term.is_tty()
    }

    fn term_key(&mut self, wait: Option<Duration>) -> Key {
        // what is drawn so far must be on the screen before waiting for a key
        let _ = self.writer.flush();
        let reader = &mut self.reader;
        let mut first = true;
        let mut stream = |_: Option<Duration>| {
            let was_first = std::mem::replace(&mut first, false);
            match reader.fill_buf() {
                Ok([]) => Next::Eof,
                // the input is not there yet: for the first byte that stops the program (the browser asks
                // the user and runs it again), for the rest of an escape sequence it is a pause
                Err(_) if was_first => Next::Failed,
                Err(_) => Next::Timeout,
                Ok(buf) => {
                    let b = buf[0];
                    reader.consume(1);
                    Next::Byte(b)
                }
            }
        };
        let key = self.term.key(wait, &mut stream);
        if let (Key::Failed(_), Some(flag)) = (&key, &self.key_wait) {
            flag.set(true);
        }
        key
    }

    fn term_size(&self) -> (i64, i64) {
        if self.term.is_tty() {
            term::size()
        } else {
            (80, 24)
        }
    }

    fn term_is_tty(&self) -> bool {
        self.term.is_tty()
    }

    fn print(&mut self, s: &str) {
        if let Err(e) = self.writer.write_all(s.as_bytes()) {
            self.write_error.get_or_insert_with(|| e.to_string());
        }
    }

    fn input(&mut self) -> Result<Option<String>, String> {
        // make prompts written with `print` visible before blocking on input
        let _ = self.writer.flush();
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Ok(None),
            Ok(_) => {
                while line.ends_with('\n') || line.ends_with('\r') {
                    line.pop();
                }
                Ok(Some(line))
            }
            Err(e) => {
                if let Some(flag) = &self.key_wait {
                    flag.set(false);
                }
                Err(e.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_served_to_the_program() {
        let io = BufferIo::with_input::<&str>(&[]).with_args(&["a", "b"]);
        assert_eq!(io.args(), ["a", "b"]);
        assert!(BufferIo::new().args().is_empty());
        let io = StreamIo::new(std::io::Cursor::new(Vec::new()), Vec::new()).with_args(vec!["x".into()]);
        assert_eq!(io.args(), ["x"]);
    }

    #[test]
    fn buffer_io_collects_output_and_serves_lines() {
        let mut io = BufferIo::with_input(&["a", "b"]);
        io.print("x");
        io.println("y");
        assert_eq!(io.output, "xy\n");
        assert_eq!(io.input().unwrap(), Some("a".into()));
        assert_eq!(io.input().unwrap(), Some("b".into()));
        assert_eq!(io.input().unwrap(), None);
    }

    #[test]
    fn stream_io_reads_lines_and_writes_bytes() {
        let input = std::io::Cursor::new(b"first\r\nsecond\nlast".to_vec());
        let mut out = Vec::new();
        {
            let mut io = StreamIo::new(input, &mut out);
            io.println("hello");
            io.print("p");
            assert_eq!(io.input().unwrap(), Some("first".into()));
            assert_eq!(io.input().unwrap(), Some("second".into()));
            assert_eq!(io.input().unwrap(), Some("last".into()));
            assert_eq!(io.input().unwrap(), None);
            assert!(io.write_error.is_none());
        }
        assert_eq!(String::from_utf8(out).unwrap(), "hello\np");
    }

    #[test]
    fn stream_io_remembers_write_errors() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk on fire"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut io = StreamIo::new(std::io::Cursor::new(Vec::new()), Broken);
        io.print("x");
        assert!(io.write_error.as_deref().unwrap().contains("disk on fire"));
    }
}
