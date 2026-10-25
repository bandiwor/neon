use std::fmt;

/// A run-time error. Execution stops and the trap is reported to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trap {
    /// Integer arithmetic result out of range for its type.
    Overflow,
    DivideByZero,
    /// Shift amount negative or not smaller than the bit width.
    ShiftOverflow,
    IndexOutOfBounds {
        index: i128,
        len: usize,
    },
    KeyNotFound,
    /// `pop` on an empty array.
    EmptyArray,
    /// `parse_i64` / `parse_f64` on malformed text.
    ParseError(String),
    /// `panic(..)` or `assert(..)`.
    Panic(String),
    /// A heap limit was exceeded even after collecting garbage.
    OutOfMemory,
    /// The call stack is too deep.
    StackOverflow,
    /// Input exhausted or an I/O error from the host.
    Io(String),
}

impl fmt::Display for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trap::Overflow => write!(f, "integer overflow"),
            Trap::DivideByZero => write!(f, "division by zero"),
            Trap::ShiftOverflow => write!(f, "shift amount out of range"),
            Trap::IndexOutOfBounds { index, len } => write!(f, "index {index} out of bounds (length {len})"),
            Trap::KeyNotFound => write!(f, "key not found in map"),
            Trap::EmptyArray => write!(f, "pop from an empty array"),
            Trap::ParseError(s) => write!(f, "cannot parse `{s}`"),
            Trap::Panic(m) => write!(f, "panic: {m}"),
            Trap::OutOfMemory => write!(f, "out of memory"),
            Trap::StackOverflow => write!(f, "stack overflow"),
            Trap::Io(m) => write!(f, "i/o error: {m}"),
        }
    }
}

impl std::error::Error for Trap {}
