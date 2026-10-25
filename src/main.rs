//! Neon command-line entry point.

#![warn(missing_debug_implementations, rust_2018_idioms)]

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use neon::vfs::OsVfs;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdin = io::stdin();
    let interactive = stdin.is_terminal();
    // returning (rather than `process::exit`) lets every local be dropped
    let code = neon::cli::main_with(&args, &OsVfs::new(), stdin.lock(), io::stdout(), &mut io::stderr(), interactive);
    ExitCode::from(code as u8)
}
