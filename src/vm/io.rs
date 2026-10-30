//! The VM's window to the outside world. The VM never touches stdin or
//! stdout itself, so it runs unchanged in tests, in a terminal, or in a
//! browser through WebAssembly.

use std::collections::VecDeque;
use std::io::{BufRead, Write};

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

    /// `sys.args`: the command-line arguments of the program (without the program itself).
    fn args(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Collects output in a string and serves input from a queue of lines.
#[derive(Debug, Default)]
pub struct BufferIo {
    pub output: String,
    input: VecDeque<String>,
    args: Vec<String>,
}

impl BufferIo {
    pub fn new() -> BufferIo {
        BufferIo::default()
    }

    pub fn with_input<S: AsRef<str>>(lines: &[S]) -> BufferIo {
        BufferIo {
            output: String::new(),
            input: lines.iter().map(|l| l.as_ref().to_string()).collect(),
            args: Vec::new(),
        }
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
}

/// Adapts any reader / writer pair (stdin and stdout for the CLI).
#[derive(Debug)]
pub struct StreamIo<R: BufRead, W: Write> {
    reader: R,
    writer: W,
    args: Vec<String>,
    /// The first write error, kept so output problems are not silently lost.
    pub write_error: Option<String>,
}

impl<R: BufRead, W: Write> StreamIo<R, W> {
    pub fn new(reader: R, writer: W) -> StreamIo<R, W> {
        StreamIo { reader, writer, args: Vec::new(), write_error: None }
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
            Err(e) => Err(e.to_string()),
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
